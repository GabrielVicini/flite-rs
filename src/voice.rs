//! Diphone voice: unit selection and waveform generation.
//!
//! The voice is a database of *diphones*, recorded transitions from the
//! middle of one phone to the middle of the next. Concatenating diphones
//! rather than whole phones puts every join in the steady middle of a sound,
//! where a splice is least audible.
//!
//! Each diphone is stored as a run of pitch periods; a period holds ten LPC
//! coefficients and an ADPCM-compressed excitation residual. Synthesis:
//!
//! 1. pick the diphone for each pair of adjacent segments;
//! 2. turn the pitch contour into a sequence of *pitch marks*, one per period
//!    of the output, where spacing them further apart lowers the pitch;
//! 3. for each output period, take the coefficients and residual from the
//!    nearest period of the source unit, resampling the unit in time to hit
//!    the target duration;
//! 4. run the whole thing through the LPC filter.
//!
//! Steps 2 and 3 are where pitch and duration are imposed: the recorded
//! material supplies timbre, the prosody model supplies everything else.

use crate::data::{u16_at, u32_at, Container, DataError, Reader};
use crate::dsp;
use crate::utterance::{ItemId, Utterance};

/// One entry of the diphone index: where a diphone's pitch periods live.
struct Diphone {
    name: &'static str,
    /// Index of the first pitch period.
    start: u32,
    /// Periods belonging to the first half (up to the phone boundary).
    first_half: u8,
    /// Periods belonging to the second half.
    second_half: u8,
}

/// A diphone database plus its pitch-period store.
pub struct Voice {
    index: Vec<Diphone>,
    /// Quantised LPC coefficients, `order` per period.
    lpc: &'static [u8],
    /// Byte offset of each period's residual within `residual`.
    residual_offsets: &'static [u8],
    /// Decoded length in samples of each period's residual.
    residual_sizes: &'static [u8],
    residual: &'static [u8],
    pub order: usize,
    pub sample_rate: u32,
    coeff_min: f32,
    coeff_range: f32,
}

impl Voice {
    pub fn parse(bytes: &'static [u8]) -> Result<Voice, DataError> {
        let container = Container::parse(bytes)?;
        let mut header = Reader::new(container.section("sts.header")?);
        let frames = header.u32()? as usize;
        let order = header.u32()? as usize;
        let sample_rate = header.u32()?;
        let coeff_min = header.f32()?;
        let coeff_range = header.f32()?;

        let lpc = container.section("sts.lpc")?;
        let residual_offsets = container.section("sts.resoffs")?;
        let residual_sizes = container.section("sts.ressize")?;
        let residual = container.section("sts.res")?;
        if lpc.len() < frames * order * 2
            || residual_sizes.len() < frames
            || residual_offsets.len() < (frames + 1) * 4
        {
            return Err(DataError("voice tables inconsistent with header"));
        }

        let mut r = Reader::new(container.section("dip.index")?);
        let count = r.u32()? as usize;
        let mut index = Vec::with_capacity(count);
        for _ in 0..count {
            let name = r.short_str()?;
            index.push(Diphone {
                name,
                start: r.u16()? as u32,
                first_half: r.u8()?,
                second_half: r.u8()?,
            });
        }
        index.sort_by_key(|d| d.name);

        Ok(Voice {
            index,
            lpc,
            residual_offsets,
            residual_sizes,
            residual,
            order,
            sample_rate,
            coeff_min,
            coeff_range,
        })
    }

    fn find(&self, name: &str) -> Option<&Diphone> {
        let i = self.index.binary_search_by(|d| d.name.cmp(name)).ok()?;
        Some(&self.index[i])
    }

    /// Decoded length in samples of one pitch period.
    fn period_samples(&self, period: usize) -> usize {
        self.residual_sizes[period] as usize
    }

    /// The packed ADPCM residual for one pitch period.
    fn period_residual(&self, period: usize) -> &'static [u8] {
        let start = u32_at(self.residual_offsets, period) as usize;
        let end = u32_at(self.residual_offsets, period + 1) as usize;
        &self.residual[start..end]
    }

    fn period_coefficients(&self, period: usize, out: &mut Vec<u16>) {
        let base = period * self.order;
        out.extend((0..self.order).map(|k| u16_at(self.lpc, base + k)));
    }
}

/// One selected unit: half a diphone, stretched to a target end time.
struct Unit {
    first_period: usize,
    last_period: usize,
    /// Where this unit should end, in output samples.
    target_end: usize,
}

/// Synthesised audio.
#[derive(Clone, Debug)]
pub struct Audio {
    pub samples: Vec<i16>,
    pub sample_rate: u32,
}

impl Audio {
    pub fn duration_seconds(&self) -> f32 {
        self.samples.len() as f32 / self.sample_rate as f32
    }
}

/// Generate the waveform for a fully analysed utterance.
pub fn synthesise(voice: &Voice, utt: &Utterance) -> Audio {
    let units = select_units(voice, utt);
    let pitch_marks = pitch_marks(voice, utt);
    if units.is_empty() || pitch_marks.len() < 2 {
        return Audio {
            samples: Vec::new(),
            sample_rate: voice.sample_rate,
        };
    }
    let (coefficients, sizes, residual) = build_excitation(voice, &units, &pitch_marks);
    let samples = dsp::lpc_resynthesise(
        &coefficients,
        &sizes,
        &residual,
        voice.coeff_min,
        voice.coeff_range,
        voice.order,
    );
    Audio {
        samples,
        sample_rate: voice.sample_rate,
    }
}

/// Choose a diphone for each adjacent pair of segments, and split it into the
/// two halves that belong to each segment.
fn select_units(voice: &Voice, utt: &Utterance) -> Vec<Unit> {
    let segments: Vec<ItemId> = utt.iter_relation("Segment").collect();
    let mut units = Vec::with_capacity(segments.len() * 2);
    let rate = voice.sample_rate as f32;

    for pair in segments.windows(2) {
        let (left, right) = (pair[0], pair[1]);
        // Consonant clusters inside a syllable onset ("kw" in "question")
        // have their own recordings, because the transition between them
        // differs from the same two phones across a syllable boundary.
        let diphone = in_consonant_cluster(utt, left)
            .then(|| voice.find(&format!("{}_-_{}", utt.name(left), utt.name(right))))
            .flatten()
            .or_else(|| voice.find(&format!("{}-{}", utt.name(left), utt.name(right))))
            // Falling back to the first entry keeps synthesis total: a missing
            // diphone costs one wrong sound, not a failed utterance.
            .unwrap_or(&voice.index[0]);

        let start = diphone.start as usize;
        let middle = start + diphone.first_half as usize;
        let end = middle + diphone.second_half as usize;

        let left_end = utt.feature_f32(left, "end");
        let right_end = utt.feature_f32(right, "end");

        // The first half runs to the boundary between the two phones; the
        // second half runs to the midpoint of the following phone, where the
        // next diphone will pick up.
        units.push(Unit {
            first_period: start,
            last_period: middle,
            target_end: (left_end * rate) as usize,
        });
        units.push(Unit {
            first_period: middle,
            last_period: end,
            target_end: ((left_end + right_end) / 2.0 * rate) as usize,
        });
    }
    units
}

/// Whether this segment and the next one in its syllable are both consonants.
fn in_consonant_cluster(utt: &Utterance, segment: ItemId) -> bool {
    if crate::phoneset::is_vowel(utt.name(segment)) {
        return false;
    }
    // Deliberately the next segment *within the syllable*: a consonant pair
    // spanning a syllable boundary is not a cluster.
    match utt
        .item_as(segment, "SylStructure")
        .and_then(|view| utt.next(view))
    {
        Some(next) => crate::phoneset::feature(utt.name(next), "vc") == "-",
        None => false,
    }
}

/// Turn the pitch-target contour into output pitch marks.
///
/// Walking forward in time and stepping by `1/f0` at each point places one
/// mark per pitch period, with F0 interpolated linearly between targets. The
/// result is the sample position of every period boundary in the output.
fn pitch_marks(voice: &Voice, utt: &Utterance) -> Vec<usize> {
    /// Pitch assumed before the first target, in Hz. Only reached when the
    /// first target sits at a non-zero time, which the F0 model prevents.
    const INITIAL_F0: f64 = 120.0;

    let mut marks = Vec::new();
    let mut time = 0.0f64;
    let mut last_position = 0.0f64;
    let mut last_f0 = INITIAL_F0;
    let rate = voice.sample_rate as f64;

    for target in utt.iter_relation("Target") {
        let position = utt.feature_f32(target, "pos") as f64;
        let f0 = utt.feature_f32(target, "f0") as f64;
        if time != position && position > last_position {
            let slope = (f0 - last_f0) / (position - last_position);
            while time < position {
                let instantaneous = last_f0 + (time - last_position) * slope;
                if instantaneous <= 0.0 {
                    break; // guard against a contour that would stall
                }
                time += 1.0 / instantaneous;
                marks.push((rate * time) as usize);
            }
        }
        last_f0 = f0;
        last_position = position;
    }
    marks
}

/// Assemble the excitation and coefficients for every output pitch period.
///
/// Each output period draws from the unit period whose position within the
/// unit best matches its position within the target span, so a unit that has
/// to fill more time repeats periods and one that has less skips them.
fn build_excitation(
    voice: &Voice,
    units: &[Unit],
    pitch_marks: &[usize],
) -> (Vec<u16>, Vec<usize>, Vec<u8>) {
    let total_samples = *pitch_marks.last().expect("caller checked non-empty");
    let mut residual = vec![dsp::ULAW_SILENCE; total_samples];
    let mut coefficients = Vec::with_capacity(pitch_marks.len() * voice.order);
    let mut sizes = Vec::with_capacity(pitch_marks.len());

    let mut decoded = Vec::new();
    let mut mark = 0usize;
    let mut written = 0usize;
    let mut target_start = 0usize;

    for unit in units {
        let unit_samples: usize = (unit.first_period..unit.last_period)
            .map(|p| voice.period_samples(p))
            .sum();
        // How fast to advance through the unit per output sample.
        let span = unit.target_end.saturating_sub(target_start).max(1);
        let rate = unit_samples as f32 / span as f32;
        let mut unit_position = 0.0f32;

        while mark < pitch_marks.len() && pitch_marks[mark] <= unit.target_end {
            let period = nearest_period(voice, unit, unit_position);
            voice.period_coefficients(period, &mut coefficients);

            let size = pitch_marks[mark] - if mark > 0 { pitch_marks[mark - 1] } else { 0 };
            sizes.push(size);

            copy_residual(voice, period, &mut decoded, &mut residual[written..], size);
            written += size;
            unit_position += size as f32 * rate;
            mark += 1;
        }
        target_start = unit.target_end;
    }

    // Trailing pitch marks past the last unit have no excitation; drop them
    // rather than synthesising from stale coefficients.
    residual.truncate(written);
    (coefficients, sizes, residual)
}

/// The pitch period of `unit` closest to `position` samples into it.
fn nearest_period(voice: &Voice, unit: &Unit, position: f32) -> usize {
    let mut offset = 0f32;
    for period in unit.first_period..unit.last_period {
        let next = offset + voice.period_samples(period) as f32;
        if (position - offset).abs() < (position - next).abs() {
            return period;
        }
        offset = next;
    }
    unit.last_period.saturating_sub(1)
}

/// Copy one period's excitation into the output, centring it in the target
/// window so that the energy peak stays put when periods are resized.
fn copy_residual(
    voice: &Voice,
    period: usize,
    decoded: &mut Vec<u8>,
    target: &mut [u8],
    target_size: usize,
) {
    dsp::decode_adpcm(voice.period_residual(period), decoded);
    let source = &decoded[dsp::ADPCM_LEAD_IN.min(decoded.len())..];
    let source_size = voice.period_samples(period).min(source.len());
    let target_size = target_size.min(target.len());

    if source_size < target_size {
        let offset = (target_size - source_size) / 2;
        target[offset..offset + source_size].copy_from_slice(&source[..source_size]);
    } else {
        let offset = (source_size - target_size) / 2;
        target[..target_size].copy_from_slice(&source[offset..offset + target_size]);
    }
}
