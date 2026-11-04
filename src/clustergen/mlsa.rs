//! The MLSA filter: mel-cepstral coefficients in, samples out.
//!
//! Clustergen predicts a mel-cepstrum per frame and this turns it into sound.
//! A Mel Log Spectrum Approximation filter realises the spectral envelope
//! those coefficients describe directly, without ever forming an impulse
//! response, by cascading two all-pass sections whose exponential is
//! approximated by a Padé expansion. Excitation is a pulse train at the
//! predicted F0 for voiced frames and noise for unvoiced ones, and with mixed
//! excitation both at once, shaped by five band filters.
//!
//! Coefficients are interpolated across each frame rather than switched at the
//! boundary, which is why the filter state has a current set, a target set and
//! a per-sample increment.
//!
//! The arithmetic follows upstream exactly, including where a `float` voice
//! parameter is combined with another `float` before being promoted: it is
//! `(beta * alpha) as f64`, not `beta as f64 * alpha as f64`, and the two
//! differ in the last bits. Upstream's own lineage for this file is HTS by way
//! of Festvox's voice-conversion code; only the algorithm is taken.

use super::random::GlibcRandom;
use crate::dsp::Flow;

/// Order of the Padé approximation to the exponential. Upstream uses 5 except
/// on Android and Windows CE, where it drops to 4 for speed.
const PADE_ORDER: usize = 5;

/// The Padé coefficient table, indexed by `order * (order + 1) / 2`.
const PADE: [f64; 21] = [
    1.0,
    1.0,
    0.0,
    1.0,
    0.0,
    0.0,
    1.0,
    0.0,
    0.0,
    0.0,
    1.0,
    0.4999273,
    0.1067005,
    0.011_702_21,
    0.000_565_627_9,
    1.0,
    0.4999391,
    0.1107098,
    0.013_699_84,
    0.000_956_485_3,
    0.000_030_417_21,
];

/// The Box-Muller generator used when there is no mixed excitation.
///
/// Deterministic, unlike the `rand()` that the mixed-excitation path reaches
/// for, so a voice that takes this branch needs nothing pinned. None of the
/// three Clustergen voices upstream ships in-tree takes it — they all set
/// `mixed_excitation` — so it is written for completeness and is not
/// exercised by the reference comparison.
///
/// `next` is `unsigned long` upstream, which is 64 bits on Unix and 32 on
/// Windows, so this generator is itself not portable across those two. 64 bits
/// is used here, matching the platform the reference is built on in CI.
struct Gaussian {
    next: u64,
    /// Box-Muller yields two values per pass; this says which is owed.
    second_pending: bool,
    r1: f64,
    r2: f64,
    s: f64,
}

impl Gaussian {
    fn new() -> Gaussian {
        Gaussian {
            next: 1,
            second_pending: false,
            r1: 0.0,
            r2: 0.0,
            s: 0.0,
        }
    }

    /// `rnd()`: a value in [0, 1) from a 64-bit linear congruential step.
    fn uniform(&mut self) -> f64 {
        const RAND_MAX: f64 = 32767.0;
        self.next = self.next.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let r = (self.next / 65536) % 32768;
        r as f64 / RAND_MAX
    }

    fn next_normal(&mut self) -> f64 {
        if !self.second_pending {
            self.second_pending = true;
            loop {
                self.r1 = 2.0 * self.uniform() - 1.0;
                self.r2 = 2.0 * self.uniform() - 1.0;
                self.s = self.r1 * self.r1 + self.r2 * self.r2;
                if self.s <= 1.0 && self.s != 0.0 {
                    break;
                }
            }
            self.s = (-2.0 * self.s.ln() / self.s).sqrt();
            self.r1 * self.s
        } else {
            self.second_pending = false;
            self.r2 * self.s
        }
    }
}

/// What the vocoder needs to know about the voice.
pub struct MlsaSettings {
    pub sample_rate: u32,
    pub frame_advance: f32,
    /// All-pass coefficient, the warping that makes the scale mel rather than
    /// linear. Held as `f32` because that is the width upstream stores it at
    /// and some of the arithmetic happens before promotion.
    pub alpha: f32,
    /// Post-filter strength. Zero disables the spectral sharpening.
    pub beta: f32,
    pub gain: f32,
    /// Five band filters for mixed excitation, `me_num` of `me_order` taps.
    /// Empty when the voice does not ask for mixed excitation.
    pub me_filters: Vec<Vec<f64>>,
}

impl MlsaSettings {
    fn me_num(&self) -> usize {
        self.me_filters.len()
    }

    fn me_order(&self) -> usize {
        self.me_filters.first().map_or(0, Vec::len)
    }
}

/// The running state of the filter.
///
/// Public and reusable because it must outlive one utterance: its
/// pseudo-random source is what upstream keeps in process-global state, so
/// restarting it between the sentences of one synthesis call would not match.
/// The filter's own delay lines are reset per utterance; only the random
/// source carries over, which is exactly what upstream carries.
pub struct Vocoder {
    /// Samples per frame, upstream's `fprd`.
    frame_length: usize,
    /// Current filter coefficients, `b` in the filter equations.
    c: Vec<f64>,
    /// The next frame's coefficients, interpolated towards.
    cc: Vec<f64>,
    /// Per-sample step from `c` towards `cc`.
    cinc: Vec<f64>,
    /// All-pass section delay lines for both halves of the cascade.
    d1: Vec<f64>,
    /// Pitch period in samples, or 0.0 when unvoiced. Negative until the first
    /// frame has been seen, which is how upstream marks "no previous frame".
    p1: f64,
    /// Where in the current pitch period the next pulse falls.
    pc: f64,
    order: usize,
    /// True when excitation comes from the Gaussian generator, i.e. when the
    /// voice has no mixed excitation.
    gauss: bool,
    gaussian: Gaussian,
    sign: GlibcRandom,
    /// Per-band impulse responses for this frame.
    hpulse: Vec<f64>,
    hnoise: Vec<f64>,
    /// Delay lines for the two shaped excitation sources.
    xpulsesig: Vec<f64>,
    xnoisesig: Vec<f64>,
    /// Scratch for the post-filter's energy calculation.
    postfilter: PostFilter,
    /// Whether the filter has been sized for an utterance yet.
    started: bool,
}

/// Scratch buffers for `b2en`, which measures the energy of a coefficient set
/// so the post-filter can restore it after sharpening.
struct PostFilter {
    /// Length of the impulse response the energy is measured over.
    ir_length: usize,
    mc: Vec<f64>,
    cep: Vec<f64>,
    ir: Vec<f64>,
    /// `freqt`'s two working rows.
    d: Vec<f64>,
    g: Vec<f64>,
}

impl PostFilter {
    fn new(order: usize) -> PostFilter {
        let ir_length = 64;
        PostFilter {
            ir_length,
            mc: vec![0.0; order + 1],
            cep: vec![0.0; ir_length],
            ir: vec![0.0; ir_length],
            d: vec![0.0; ir_length],
            g: vec![0.0; ir_length],
        }
    }
}

impl Vocoder {
    /// A fresh filter for a new run of speech.
    ///
    /// The order and frame length are not known until the first parameter
    /// track arrives, so they are filled in then.
    pub fn new_run() -> Vocoder {
        Vocoder {
            frame_length: 0,
            c: Vec::new(),
            cc: Vec::new(),
            cinc: Vec::new(),
            d1: Vec::new(),
            p1: -1.0,
            pc: 0.0,
            order: 0,
            gauss: true,
            gaussian: Gaussian::new(),
            sign: GlibcRandom::new(),
            hpulse: Vec::new(),
            hnoise: Vec::new(),
            xpulsesig: Vec::new(),
            xnoisesig: Vec::new(),
            postfilter: PostFilter::new(0),
            started: false,
        }
    }

    /// Set the filter up for one utterance, keeping the random source.
    ///
    /// Everything but the two generators is reset: upstream builds a fresh
    /// `VocoderSetup` per utterance and the generators are the only state it
    /// does *not* put there, because they live in the C library instead.
    fn begin(&mut self, settings: &MlsaSettings, order: usize, frame_length: usize) {
        let fresh = Vocoder::new(settings, order, frame_length);
        let Vocoder {
            frame_length,
            c,
            cc,
            cinc,
            d1,
            p1,
            pc,
            order,
            gauss,
            hpulse,
            hnoise,
            xpulsesig,
            xnoisesig,
            postfilter,
            ..
        } = fresh;
        self.frame_length = frame_length;
        self.c = c;
        self.cc = cc;
        self.cinc = cinc;
        self.d1 = d1;
        self.p1 = p1;
        self.pc = pc;
        self.order = order;
        self.gauss = gauss;
        self.hpulse = hpulse;
        self.hnoise = hnoise;
        self.xpulsesig = xpulsesig;
        self.xnoisesig = xnoisesig;
        self.postfilter = postfilter;
        self.started = true;
    }

    fn new(settings: &MlsaSettings, order: usize, frame_length: usize) -> Vocoder {
        let mixed = settings.me_num() > 0;
        let me_order = settings.me_order();
        Vocoder {
            frame_length,
            c: vec![0.0; order + 1],
            cc: vec![0.0; order + 1],
            cinc: vec![0.0; order + 1],
            // mlsadf1 takes 2 * (pd + 1); mlsadf2 takes pd * (order + 2) + pd
            // + 1. Allocated as one block upstream, kept separate here.
            d1: vec![0.0; 2 * (PADE_ORDER + 1) + (PADE_ORDER + 1) * (order + 2)],
            p1: -1.0,
            pc: 0.0,
            order,
            // Mixed excitation turns the Gaussian source off, as upstream's
            // `if (str != NULL) vs.gauss = MFALSE` does.
            gauss: !mixed,
            gaussian: Gaussian::new(),
            sign: GlibcRandom::new(),
            hpulse: vec![0.0; me_order],
            hnoise: vec![0.0; me_order],
            xpulsesig: vec![0.0; me_order],
            xnoisesig: vec![0.0; me_order],
            postfilter: PostFilter::new(order),
            started: true,
        }
    }
}

/// Run the filter over a parameter track, handing each frame's samples to
/// `sink`.
///
/// One frame of output is `frame_advance` seconds long, so the sink is called
/// about two hundred times a second and nothing longer is held.
pub fn resynthesise(
    settings: &MlsaSettings,
    vocoder: &mut Vocoder,
    track: &[Vec<f32>],
    strengths: &[Vec<f32>],
    sink: &mut dyn FnMut(&[i16]) -> Flow,
) {
    let Some(first) = track.first() else { return };
    let rate = settings.sample_rate as f64;
    // Upstream derives the frame length from the track's own timestamps, which
    // are `i * frame_advance`, so their difference is the advance.
    let frame_length = (0.5 + (1000.0 * settings.frame_advance as f64) * rate / 1000.0) as usize;

    // Channel 0 is F0 and the rest are the cepstrum, with one trailing zero
    // coefficient that upstream appends after the copy.
    let order = first.len() - 1;
    vocoder.begin(settings, order, frame_length);
    let mut mcep = vec![0.0f64; order + 1];
    let mut block = Vec::with_capacity(frame_length);

    for (t, frame) in track.iter().enumerate() {
        for k in 0..order {
            mcep[k] = frame[k + 1] as f64;
        }
        mcep[order] = 0.0;
        block.clear();
        generate(
            vocoder,
            settings,
            frame[0],
            &mcep,
            strengths.get(t).map(Vec::as_slice),
            &mut block,
        );
        if !block.is_empty() && sink(&block) == Flow::Stop {
            return;
        }
    }
}

/// One frame: set up the interpolation, then emit `frame_length` samples.
fn generate(
    vs: &mut Vocoder,
    settings: &MlsaSettings,
    f0: f32,
    mcep: &[f64],
    strengths: Option<&[f32]>,
    out: &mut Vec<i16>,
) {
    let m = vs.order;
    let alpha = settings.alpha as f64;
    let gain = if settings.gain != 0.0 {
        settings.gain
    } else {
        1.0
    };

    if let Some(strengths) = strengths {
        // Build this frame's pulse and noise shaping filters: each band's
        // strength says how much of that band is periodic and how much is
        // noise, and the two must sum to the band filter.
        for i in 0..settings.me_order() {
            let mut pulse = 0.0;
            let mut noise = 0.0;
            for (j, filter) in settings.me_filters.iter().enumerate() {
                let strength = strengths[j];
                pulse += strength as f64 * filter[i];
                // `1 - str[j]` is computed in f32 upstream, the int 1 being
                // converted to float rather than the strength to double.
                noise += (1.0f32 - strength) as f64 * filter[i];
            }
            vs.hpulse[i] = pulse;
            vs.hnoise[i] = noise;
        }
    }

    // F0 in Hz becomes a period in samples, which is what the pulse train is
    // spaced by. Zero stays zero and means unvoiced.
    let mut p = f0 as f64;
    if p != 0.0 {
        p = settings.sample_rate as f64 / p;
    }

    if vs.p1 < 0.0 {
        // The very first frame only primes the filter: there is nothing to
        // interpolate from yet, so no samples come out of it. Upstream returns
        // here too, which is why a Clustergen utterance is one frame shorter
        // than its parameter track.
        vs.p1 = p;
        vs.pc = vs.p1;
        mc2b(mcep, &mut vs.c, m, alpha);
        if settings.beta > 0.0 && m > 1 {
            post_filter(&mut vs.c, mcep, m, settings, &mut vs.postfilter);
        }
        return;
    }

    mc2b(mcep, &mut vs.cc, m, alpha);
    if settings.beta > 0.0 && m > 1 {
        post_filter(&mut vs.cc, mcep, m, settings, &mut vs.postfilter);
    }

    let frame_length = vs.frame_length as f64;
    for k in 0..=m {
        vs.cinc[k] = (vs.cc[k] - vs.c[k]) / frame_length;
    }

    // A transition into or out of silence is a step, not a glide: there is no
    // sensible interpolation between "no pitch" and a pitch.
    let inc = if vs.p1 != 0.0 && p != 0.0 {
        (p - vs.p1) / frame_length
    } else {
        vs.pc = p;
        vs.p1 = 0.0;
        0.0
    };

    for _ in 0..vs.frame_length {
        let mut x;
        let mut xpulse = 0.0;
        let mut xnoise = 0.0;

        if vs.p1 == 0.0 {
            // Unvoiced: noise only.
            x = if vs.gauss {
                vs.gaussian.next_normal()
            } else {
                vs.sign.plus_or_minus_one()
            };
            if strengths.is_some() {
                xnoise = x;
                xpulse = 0.0;
            }
        } else {
            // Voiced: one pulse per period, scaled so that its energy does not
            // depend on how far apart the pulses are.
            vs.pc += 1.0;
            if vs.pc >= vs.p1 {
                x = vs.p1.sqrt();
                vs.pc -= vs.p1;
            } else {
                x = 0.0;
            }
            if strengths.is_some() {
                xpulse = x;
                xnoise = vs.sign.plus_or_minus_one();
            }
        }

        if strengths.is_some() {
            // Shape pulse and noise through their band filters and add them.
            // The delay lines shift as the convolution is accumulated, which
            // is why this is one loop rather than two.
            let mut fxpulse = 0.0;
            let mut fxnoise = 0.0;
            for k in (1..settings.me_order()).rev() {
                fxpulse += vs.hpulse[k] * vs.xpulsesig[k];
                fxnoise += vs.hnoise[k] * vs.xnoisesig[k];
                vs.xpulsesig[k] = vs.xpulsesig[k - 1];
                vs.xnoisesig[k] = vs.xnoisesig[k - 1];
            }
            fxpulse += vs.hpulse[0] * xpulse;
            fxnoise += vs.hnoise[0] * xnoise;
            vs.xpulsesig[0] = xpulse;
            vs.xnoisesig[0] = xnoise;
            x = fxpulse + fxnoise;
        }

        // c[0] is the log gain of the frame, so the excitation is scaled by
        // its exponential before the filter shapes it.
        if settings.sample_rate == 8000 {
            // Upstream's comment: "8KHz voices are too quiet: this is probably
            // not general". Reproduced because it is what the output is.
            x *= vs.c[0].exp() * 2.0;
        } else {
            x *= vs.c[0].exp() * gain as f64;
        }

        x = mlsadf(x, m, alpha, vs);
        out.push(to_i16(x));

        // iprd is 1 upstream, so the coefficients and the pitch advance every
        // sample rather than every few.
        vs.p1 += inc;
        for k in 0..=m {
            vs.c[k] += vs.cinc[k];
        }
    }

    vs.p1 = p;
    vs.c[..=m].copy_from_slice(&vs.cc[..=m]);
}

/// C's `(short)` cast of a double.
///
/// Truncation toward zero to an `int`, then the low sixteen bits. Rust's `as`
/// saturates where C is undefined, and on x86 an out-of-range conversion
/// answers `INT_MIN`, so that case is spelled out rather than left to differ.
fn to_i16(x: f64) -> i16 {
    let truncated = if x.is_nan() || x < i32::MIN as f64 || x > i32::MAX as f64 {
        i32::MIN
    } else {
        x as i32
    };
    truncated as i16
}

/// The post-filter: sharpen the spectrum, then put the energy back.
///
/// Raising the higher coefficients by `1 + beta` deepens the formant valleys,
/// which counteracts the over-smoothing that averaging cluster means causes.
/// That changes the overall gain, so the energy is measured before and after
/// and the difference folded into `b[0]`.
fn post_filter(
    b: &mut [f64],
    mcep: &[f64],
    m: usize,
    settings: &MlsaSettings,
    scratch: &mut PostFilter,
) {
    let alpha = settings.alpha as f64;
    let e1 = b2en(b, m, alpha, scratch);
    // Both operands are `float` upstream, so this product is computed at
    // single precision and only then promoted.
    b[1] -= (settings.beta * settings.alpha) as f64 * mcep[2];
    let scale = 1.0 + settings.beta as f64;
    for coefficient in b[2..=m].iter_mut() {
        *coefficient *= scale;
    }
    let e2 = b2en(b, m, alpha, scratch);
    b[0] += (e1 / e2).ln() / 2.0;
}

/// Mel-cepstrum to MLSA filter coefficients.
fn mc2b(mc: &[f64], b: &mut [f64], m: usize, a: f64) {
    b[m] = mc[m];
    for k in (0..m).rev() {
        b[k] = mc[k] - a * b[k + 1];
    }
}

/// MLSA filter coefficients back to a mel-cepstrum.
fn b2mc(b: &[f64], mc: &mut [f64], m: usize, a: f64) {
    let mut d = b[m];
    mc[m] = d;
    for k in (0..m).rev() {
        let o = b[k] + a * d;
        d = b[k];
        mc[k] = o;
    }
}

/// The energy of the impulse response the coefficients describe.
fn b2en(b: &[f64], m: usize, a: f64, scratch: &mut PostFilter) -> f64 {
    if scratch.mc.len() < m + 1 {
        scratch.mc = vec![0.0; m + 1];
    }
    b2mc(b, &mut scratch.mc, m, a);
    let ir_length = scratch.ir_length;
    freqt(m, ir_length - 1, -a, scratch);
    c2ir(ir_length, scratch);
    scratch.ir[..ir_length].iter().map(|v| v * v).sum()
}

/// Frequency transformation: rewarp a cepstrum onto a different all-pass
/// coefficient, here from the mel scale back to linear.
fn freqt(m1: usize, m2: usize, a: f64, scratch: &mut PostFilter) {
    if scratch.d.len() < m2 + 1 {
        scratch.d = vec![0.0; m2 + 1];
        scratch.g = vec![0.0; m2 + 1];
    }
    let b = 1.0 - a * a;
    for v in scratch.g[..=m2].iter_mut() {
        *v = 0.0;
    }

    // Walking the input backwards accumulates the recursion in place, which is
    // what lets this run in two rows rather than a full matrix.
    for i in (0..=m1).rev() {
        scratch.d[0] = scratch.g[0];
        scratch.g[0] = scratch.mc[i] + a * scratch.d[0];
        if m2 >= 1 {
            scratch.d[1] = scratch.g[1];
            scratch.g[1] = b * scratch.d[0] + a * scratch.d[1];
        }
        for j in 2..=m2 {
            scratch.d[j] = scratch.g[j];
            scratch.g[j] = scratch.d[j - 1] + a * (scratch.d[j] - scratch.g[j - 1]);
        }
    }
    scratch.cep[..=m2].copy_from_slice(&scratch.g[..=m2]);
}

/// Cepstrum to minimum-phase impulse response.
fn c2ir(length: usize, scratch: &mut PostFilter) {
    let nc = scratch.cep.len();
    scratch.ir[0] = scratch.cep[0].exp();
    for n in 1..length {
        let upl = if n >= nc { nc - 1 } else { n };
        let mut d = 0.0;
        for k in 1..=upl {
            d += k as f64 * scratch.cep[k] * scratch.ir[n - k];
        }
        scratch.ir[n] = d / n as f64;
    }
}

/// One sample through the whole filter: the two cascaded sections.
fn mlsadf(x: f64, m: usize, a: f64, vs: &mut Vocoder) -> f64 {
    let pade = &PADE[PADE_ORDER * (PADE_ORDER + 1) / 2..];
    // The first section realises the b[1] term alone and the second the rest,
    // which is the factorisation that makes the Padé expansion cheap.
    let (first, second) = vs.d1.split_at_mut(2 * (PADE_ORDER + 1));
    let x = mlsadf1(x, &vs.c, a, pade, first);
    mlsadf2(x, &vs.c, m, a, pade, second)
}

/// The first section: a single all-pass stage driven by `b[1]`.
fn mlsadf1(mut x: f64, b: &[f64], a: f64, pade: &[f64], d: &mut [f64]) -> f64 {
    let aa = 1.0 - a * a;
    let (d, pt) = d.split_at_mut(PADE_ORDER + 1);
    let mut out = 0.0;
    for i in (1..=PADE_ORDER).rev() {
        d[i] = aa * pt[i - 1] + a * d[i];
        pt[i] = d[i] * b[1];
        let v = pt[i] * pade[i];
        // The Padé numerator alternates sign; the denominator does not.
        if i & 1 == 1 {
            x += v;
        } else {
            x -= v;
        }
        out += v;
    }
    pt[0] = x;
    out + x
}

/// The second section: the all-pass cascade over `b[2..=m]`.
fn mlsadf2(mut x: f64, b: &[f64], m: usize, a: f64, pade: &[f64], d: &mut [f64]) -> f64 {
    let stride = m + 2;
    let mut out = 0.0;
    for i in (1..=PADE_ORDER).rev() {
        // Each Padé stage filters the previous one's output. The stages share
        // one buffer, with the stage outputs living past the delay lines.
        let input = d[PADE_ORDER * stride + i - 1];
        let (lines, pt) = d.split_at_mut(PADE_ORDER * stride);
        let filtered = mlsafir(input, b, m, a, &mut lines[(i - 1) * stride..i * stride]);
        pt[i] = filtered;
        let v = pt[i] * pade[i];
        if i & 1 == 1 {
            x += v;
        } else {
            x -= v;
        }
        out += v;
    }
    d[PADE_ORDER * stride] = x;
    out + x
}

/// The all-pass FIR that the second section cascades.
fn mlsafir(x: f64, b: &[f64], m: usize, a: f64, d: &mut [f64]) -> f64 {
    let aa = 1.0 - a * a;
    let mut y = 0.0;

    d[0] = x;
    d[1] = aa * d[0] + a * d[1];
    for i in 2..=m {
        d[i] += a * (d[i + 1] - d[i - 1]);
        y += d[i] * b[i];
    }
    // Shift the delay line up by one, which is the unit delay of the section.
    for i in (2..=m + 1).rev() {
        d[i] = d[i - 1];
    }
    y
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mc2b_and_b2mc_are_inverses() {
        let mc = [0.5, -0.25, 0.125, -0.0625, 0.03125];
        let mut b = [0.0; 5];
        let mut back = [0.0; 5];
        mc2b(&mc, &mut b, 4, 0.42);
        b2mc(&b, &mut back, 4, 0.42);
        for (a, b) in mc.iter().zip(&back) {
            assert!((a - b).abs() < 1e-12, "{a} vs {b}");
        }
    }

    #[test]
    fn a_short_cast_takes_the_low_sixteen_bits() {
        assert_eq!(to_i16(1.9), 1);
        assert_eq!(to_i16(-1.9), -1);
        assert_eq!(to_i16(32767.0), 32767);
        // 32768 does not fit and wraps, as C's narrowing conversion does.
        assert_eq!(to_i16(32768.0), -32768);
        assert_eq!(to_i16(f64::NAN), 0);
    }
}
