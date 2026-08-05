//! Signal processing: µ-law companding, ADPCM residual decoding, and LPC
//! resynthesis.
//!
//! The voice stores each pitch period as ten LPC coefficients plus an
//! excitation residual. Resynthesis runs the residual back through the
//! all-pole filter the coefficients describe; stretching or compressing the
//! spacing of the pitch periods is what changes pitch and duration.
//!
//! The arithmetic here is deliberately integer-only. That is not nostalgia:
//! it makes output bit-identical on every target regardless of floating-point
//! settings, and it is measurably faster than the float path on machines
//! without fast FPUs, which is the portability promise this crate inherits.

/// µ-law byte to 16-bit linear sample (ITU-T G.711).
pub fn ulaw_to_i16(byte: u8) -> i16 {
    const EXPONENT_BIAS: [i32; 8] = [0, 132, 396, 924, 1980, 4092, 8316, 16764];
    let b = !byte;
    let sign = b & 0x80;
    let exponent = ((b >> 4) & 0x07) as usize;
    let mantissa = (b & 0x0F) as i32;
    let sample = EXPONENT_BIAS[exponent] + (mantissa << (exponent + 3));
    if sign != 0 {
        -sample as i16
    } else {
        sample as i16
    }
}

/// 16-bit linear sample to µ-law byte (ITU-T G.711).
pub fn i16_to_ulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    let sign = ((sample >> 8) & 0x80) as i32;
    let mut magnitude = if sign != 0 {
        -(sample as i32)
    } else {
        sample as i32
    };
    if magnitude > CLIP {
        magnitude = CLIP;
    }
    magnitude += BIAS;
    // Position of the highest set bit above bit 7, i.e. the µ-law exponent.
    // The exponent is the position of the highest set bit of the biased
    // magnitude above bit 7, i.e. which µ-law segment the sample falls in.
    let exponent = (((magnitude >> 7) & 0xFF) as u32)
        .checked_ilog2()
        .unwrap_or(0)
        .min(7) as i32;
    let mantissa = (magnitude >> (exponent + 3)) & 0x0F;
    !((sign | (exponent << 4) | mantissa) as u8)
}

/// µ-law code for silence. The residual buffer is pre-filled with this so that
/// any sample a unit does not cover is silent rather than full-scale negative.
pub const ULAW_SILENCE: u8 = 255;

/// Samples of ADPCM lead-in decoded and discarded before each residual.
///
/// The encoder's adaptive state is not stored with each frame, so the decoder
/// needs a short run-up to converge before its output is usable.
pub const ADPCM_LEAD_IN: usize = 8;

// G.721 ADPCM
//
// A 32 kbit/s adaptive differential codec (ITU-T G.721): each 4-bit code is a
// quantised prediction error, with the quantiser step size and the predictor
// coefficients both adapting sample by sample. Widths below (`i16` locals,
// `i32` intermediates) are part of the algorithm's definition, not an
// implementation detail. The fixed-point truncation is what the encoder
// assumed.

#[rustfmt::skip]
const POWER2: [i16; 15] = [1, 2, 4, 8, 0x10, 0x20, 0x40, 0x80, 0x100, 0x200, 0x400, 0x800, 0x1000, 0x2000, 0x4000];
#[rustfmt::skip]
const DQLN: [i16; 16] = [-2048, 4, 135, 213, 273, 323, 373, 425, 425, 373, 323, 273, 213, 135, 4, -2048];
#[rustfmt::skip]
const WI: [i16; 16] = [-12, 18, 41, 64, 112, 198, 355, 1122, 1122, 355, 198, 112, 64, 41, 18, -12];
#[rustfmt::skip]
const FI: [i16; 16] = [0, 0, 0, 0x200, 0x200, 0x200, 0x600, 0xE00, 0xE00, 0x600, 0x200, 0x200, 0x200, 0, 0, 0];

/// Adaptive decoder state, reset at the start of every residual.
struct AdpcmState {
    /// Locked (steady-state) step size multiplier.
    yl: i32,
    /// Unlocked step size multiplier.
    yu: i16,
    /// Short- and long-term energy estimates.
    dms: i16,
    dml: i16,
    /// Weighting between `yl` and `yu`.
    ap: i16,
    /// Pole and zero predictor coefficients.
    a: [i16; 2],
    b: [i16; 6],
    /// Signs of the last two partially reconstructed samples.
    pk: [i16; 2],
    /// Recent quantised differences and reconstructed samples, in the codec's
    /// internal 4-bit-exponent/6-bit-mantissa format.
    dq: [i16; 6],
    sr: [i16; 2],
    /// Delayed tone detector.
    td: bool,
}

impl AdpcmState {
    fn new() -> AdpcmState {
        AdpcmState {
            yl: 34816,
            yu: 544,
            dms: 0,
            dml: 0,
            ap: 0,
            a: [0; 2],
            b: [0; 6],
            pk: [0; 2],
            dq: [32; 6],
            sr: [32; 2],
            td: false,
        }
    }

    fn predictor_zero(&self) -> i32 {
        (0..6)
            .map(|i| fmult((self.b[i] >> 2) as i32, self.dq[i] as i32))
            .sum()
    }

    fn predictor_pole(&self) -> i32 {
        fmult((self.a[1] >> 2) as i32, self.sr[1] as i32)
            + fmult((self.a[0] >> 2) as i32, self.sr[0] as i32)
    }

    /// Blend the locked and unlocked step sizes according to how stationary
    /// the signal looks.
    fn step_size(&self) -> i32 {
        if self.ap >= 256 {
            return self.yu as i32;
        }
        let y = self.yl >> 6;
        let diff = self.yu as i32 - y;
        let al = (self.ap >> 2) as i32;
        if diff > 0 {
            y + ((diff * al) >> 6)
        } else if diff < 0 {
            y + ((diff * al + 0x3F) >> 6)
        } else {
            y
        }
    }

    fn decode(&mut self, code: u8) -> i16 {
        let i = (code & 0x0F) as usize;
        let sezi = self.predictor_zero() as i16;
        let sez = sezi >> 1;
        let sei = (sezi as i32 + self.predictor_pole()) as i16;
        let se = sei >> 1;

        let y = self.step_size();
        let dq = reconstruct(i & 0x08 != 0, DQLN[i] as i32, y) as i16;
        let sr = if dq < 0 {
            (se as i32 - (dq as i32 & 0x3FFF)) as i16
        } else {
            (se as i32 + dq as i32) as i16
        };
        let dqsez = (sr as i32 - se as i32 + sez as i32) as i16;

        self.update(
            y,
            (WI[i] as i32) << 5,
            FI[i] as i32,
            dq as i32,
            sr as i32,
            dqsez as i32,
        );
        // The codec works in a 14-bit dynamic range; shift back up to 16 bits.
        ((sr as i32) << 2) as i16
    }

    fn update(&mut self, y: i32, wi: i32, fi: i32, dq: i32, sr: i32, dqsez: i32) {
        let pk0: i16 = if dqsez < 0 { 1 } else { 0 };
        let mag = (dq & 0x7FFF) as i16;

        // Tone/transition detection: a large prediction error relative to the
        // long-term step size suggests non-speech (modem) input.
        let ylint = (self.yl >> 15).clamp(0, 15) as i16;
        let ylfrac = ((self.yl >> 10) & 0x1F) as i16;
        let thr1 = ((32 + ylfrac as i32) << ylint) as i16;
        let thr2 = if ylint > 9 { 31 << 10 } else { thr1 };
        let dqthr = (thr2 as i32 + (thr2 as i32 >> 1)) >> 1;
        let transition = self.td && (mag as i32 > dqthr);

        // Quantiser step size adaptation.
        self.yu = (y + ((wi - y) >> 5)).clamp(544, 5120) as i16;
        self.yl += self.yu as i32 + ((-self.yl) >> 6);

        let mut a2p: i16 = 0;
        if transition {
            self.a = [0; 2];
            self.b = [0; 6];
        } else {
            let pks1 = pk0 ^ self.pk[0];

            a2p = self.a[1] - (self.a[1] >> 7);
            if dqsez != 0 {
                let fa1 = if pks1 != 0 { self.a[0] } else { -self.a[0] };
                a2p = if fa1 < -8191 {
                    (a2p as i32 - 0x100) as i16
                } else if fa1 > 8191 {
                    (a2p as i32 + 0xFF) as i16
                } else {
                    (a2p as i32 + (fa1 >> 5) as i32) as i16
                };

                if pk0 ^ self.pk[1] != 0 {
                    a2p = if a2p <= -12160 {
                        -12288
                    } else if a2p >= 12416 {
                        12288
                    } else {
                        (a2p as i32 - 0x80) as i16
                    };
                } else {
                    a2p = if a2p <= -12416 {
                        -12288
                    } else if a2p >= 12160 {
                        12288
                    } else {
                        (a2p as i32 + 0x80) as i16
                    };
                }
            }
            self.a[1] = a2p;

            self.a[0] -= self.a[0] >> 8;
            if dqsez != 0 {
                if pks1 == 0 {
                    self.a[0] = (self.a[0] as i32 + 192) as i16;
                } else {
                    self.a[0] = (self.a[0] as i32 - 192) as i16;
                }
            }
            let a1_limit = (15360 - a2p as i32) as i16;
            self.a[0] = self.a[0].clamp(-a1_limit, a1_limit);

            for cnt in 0..6 {
                self.b[cnt] -= self.b[cnt] >> 8;
                if dq & 0x7FFF != 0 {
                    if (dq ^ self.dq[cnt] as i32) >= 0 {
                        self.b[cnt] = (self.b[cnt] as i32 + 128) as i16;
                    } else {
                        self.b[cnt] = (self.b[cnt] as i32 - 128) as i16;
                    }
                }
            }
        }

        self.dq.copy_within(0..5, 1);
        self.dq[0] = float_encode(dq, mag as i32);

        self.sr[1] = self.sr[0];
        self.sr[0] = if sr == 0 {
            0x20
        } else if sr > 0 {
            let exp = quan(sr, &POWER2);
            ((exp << 6) + ((sr << 6) >> exp)) as i16
        } else if sr > -32768 {
            let mag = -sr;
            let exp = quan(mag, &POWER2);
            ((exp << 6) + ((mag << 6) >> exp) - 0x400) as i16
        } else {
            0xFC20u16 as i16
        };

        self.pk[1] = self.pk[0];
        self.pk[0] = pk0;

        // A sample treated as data resets the detector; strong negative
        // sample-to-sample correlation arms it.
        self.td = !transition && a2p < -11776;

        // Adaptation speed control: `ap` moves towards 256 when the signal is
        // changing quickly, which unlocks the step size.
        self.dms = (self.dms as i32 + ((fi - self.dms as i32) >> 5)) as i16;
        self.dml = (self.dml as i32 + (((fi << 2) - self.dml as i32) >> 7)) as i16;
        let unsteady = ((self.dms as i32) << 2) - self.dml as i32;
        if transition {
            self.ap = 256;
        } else if y < 1536 || self.td || unsteady.abs() >= (self.dml as i32) >> 3 {
            self.ap = (self.ap as i32 + ((0x200 - self.ap as i32) >> 4)) as i16;
        } else {
            self.ap = (self.ap as i32 + ((-(self.ap as i32)) >> 4)) as i16;
        }
    }
}

/// Convert a quantised difference into the codec's internal floating format.
fn float_encode(dq: i32, mag: i32) -> i16 {
    if mag == 0 {
        return if dq >= 0 { 0x20 } else { 0xFC20u16 as i16 };
    }
    let exp = quan(mag, &POWER2);
    let base = (exp << 6) + ((mag << 6) >> exp);
    if dq >= 0 {
        base as i16
    } else {
        (base - 0x400) as i16
    }
}

/// Index of the first table entry greater than `val`.
fn quan(val: i32, table: &[i16]) -> i32 {
    table
        .iter()
        .position(|t| val < *t as i32)
        .unwrap_or(table.len()) as i32
}

/// Multiply a 14-bit integer by a value in the codec's 4-bit-exponent,
/// 6-bit-mantissa format.
fn fmult(an: i32, srn: i32) -> i32 {
    let anmag = if an > 0 { an } else { (-an) & 0x1FFF } as i16 as i32;
    let anexp = quan(anmag, &POWER2) - 6;
    let anmant = if anmag == 0 {
        32
    } else if anexp >= 0 {
        anmag >> anexp
    } else {
        anmag << -anexp
    } as i16 as i32;
    let wanexp = anexp + ((srn >> 6) & 0xF) - 13;
    let wanmant = ((anmant * (srn & 0o77) + 0x30) >> 4) as i16 as i32;
    let retval = if wanexp >= 0 {
        ((wanmant << wanexp) & 0x7FFF) as i16 as i32
    } else {
        (wanmant >> -wanexp) as i16 as i32
    };
    if (an ^ srn) < 0 {
        -retval
    } else {
        retval
    }
}

/// Reconstruct a difference sample from its code and the step size.
fn reconstruct(negative: bool, dqln: i32, y: i32) -> i32 {
    let dql = dqln + (y >> 2);
    if dql < 0 {
        return if negative { -0x8000 } else { 0 };
    }
    let dex = (dql >> 7) & 15;
    let dqt = 128 + (dql & 127);
    let dq = (dqt << 7) >> (14 - dex);
    if negative {
        dq - 0x8000
    } else {
        dq
    }
}

/// Decode `packed` (two 4-bit codes per byte) into µ-law samples.
///
/// Output length is always `2 * packed.len()`; the first [`ADPCM_LEAD_IN`]
/// samples are the decoder's run-up and should be discarded.
pub fn decode_adpcm(packed: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(packed.len() * 2);
    let mut state = AdpcmState::new();
    for byte in packed {
        for code in [byte >> 4, byte & 0x0F] {
            let sample = state.decode(code);
            out.push(i16_to_ulaw(sample));
        }
    }
}

/// Fixed-point scale for LPC coefficients and the filter accumulator (Q14).
const LPC_SCALE: i32 = 16384;

/// Run the LPC synthesis filter over a residual, one pitch period at a time.
///
/// `quantised` holds `order` coefficients per pitch period, `sizes[i]` gives
/// period `i`'s length in samples, and `residual` is the µ-law excitation for
/// the whole utterance. `coeff_min` and `coeff_range` undo the quantisation.
///
/// The filter state deliberately carries across period boundaries: resetting
/// it at each pitch mark produces an audible buzz.
pub fn lpc_resynthesise(
    quantised: &[u16],
    sizes: &[usize],
    residual: &[u8],
    coeff_min: f32,
    coeff_range: f32,
    order: usize,
) -> Vec<i16> {
    let total: usize = sizes.iter().sum();
    let mut samples = Vec::with_capacity(total);

    let min_q = (coeff_min * 32768.0) as i32;
    // The range is known to stay well inside ±16, so 2048 is a safe scale.
    let range_q = (coeff_range * 2048.0) as i32;

    // Circular buffer of the previous `order` output samples.
    let mut history = vec![0i32; order + 1];
    let mut head = order;
    let mut coefficients = vec![0i32; order];
    let mut position = 0usize;

    for (i, &size) in sizes.iter().enumerate() {
        let frame = &quantised[i * order..(i + 1) * order];
        for (k, coefficient) in coefficients.iter_mut().enumerate() {
            let q = frame[k] as i32;
            *coefficient = ((q / 2 * range_q) / 2048 + min_q) / 2;
        }

        for _ in 0..size {
            if position >= residual.len() {
                break;
            }
            let mut acc = (ulaw_to_i16(residual[position]) as i32).wrapping_mul(LPC_SCALE);
            let mut tap = if head == 0 { order } else { head - 1 };
            for coefficient in &coefficients {
                acc = acc.wrapping_add(coefficient.wrapping_mul(history[tap]));
                tap = if tap == 0 { order } else { tap - 1 };
            }
            acc /= LPC_SCALE;
            history[head] = acc;
            samples.push(acc as i16);
            head = if head == order { 0 } else { head + 1 };
            position += 1;
        }
    }
    samples
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ulaw_round_trips_at_the_extremes() {
        assert_eq!(ulaw_to_i16(255), 0);
        assert_eq!(ulaw_to_i16(0), -32124);
        assert_eq!(ulaw_to_i16(128), 32124);
        assert_eq!(i16_to_ulaw(0), 255);
        assert_eq!(i16_to_ulaw(-32124), 0);
        assert_eq!(i16_to_ulaw(32124), 128);
    }

    #[test]
    fn ulaw_encoding_is_stable_under_a_decode_encode_cycle() {
        // Byte 127 is the negative zero of the encoding and maps back to 255,
        // which is the only value that does not round-trip.
        for byte in 0..=255u8 {
            if byte == 127 {
                continue;
            }
            let decoded = ulaw_to_i16(byte);
            assert_eq!(i16_to_ulaw(decoded), byte, "byte {byte}");
        }
    }

    #[test]
    fn adpcm_decodes_to_the_expected_length() {
        let mut out = Vec::new();
        decode_adpcm(&[0x12, 0x34, 0x56], &mut out);
        assert_eq!(out.len(), 6);
    }
}
