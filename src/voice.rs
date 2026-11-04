//! The seam between linguistic analysis and waveform generation.
//!
//! Everything upstream of here turns text into phones with durations and a
//! pitch contour; everything downstream turns that into samples. The two
//! halves meet at [`Backend`], and a voice is a [`VoiceProfile`] (what any
//! synthesiser needs to know) plus one of those.
//!
//! [`Backend::synthesise_streaming`] is the primitive, not
//! [`Backend::synthesise`]. That is deliberate: the crate's constant-memory
//! guarantee comes from never holding more than one utterance, so the
//! collecting method is a provided method built on the streaming one and a
//! backend that cannot stream cannot be written by accident.
//!
//! Dispatch is one virtual call per utterance — the backend fills a buffer and
//! hands it over, so nothing is dispatched per sample.

use crate::dsp::Flow;
use crate::utterance::Utterance;

/// What a synthesiser needs to know about a voice regardless of how it makes
/// sound.
///
/// Split out of the diphone database because none of it is about diphones:
/// every backend needs the rate it runs at and the prosody the voice asks for,
/// and none of them should have to know about LPC frames to get it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceProfile {
    pub sample_rate: u32,
    /// The rate and pitch this voice registers itself with, which differ per
    /// voice and so travel in the voice file rather than in the code.
    pub duration_stretch: f32,
    pub f0_mean: f32,
    pub f0_stddev: f32,
    /// Whether this voice's postlexical rules rewrite `ah` as `aa`.
    ///
    /// A property of the recordings, not of the language: the 8 kHz diphone
    /// database records the vowel only as `aa` while the 16 kHz one has both.
    pub fold_ah_to_aa: bool,
    /// Whether the language's segment-duration model runs.
    ///
    /// Clustergen predicts durations per HMM state from its own trees, so it
    /// sets this false and does the work itself. Upstream carries the same
    /// decision as the voice feature `no_segment_duration_model`.
    pub segment_duration_model: bool,
    /// Whether the language's F0 target model runs.
    ///
    /// As above: Clustergen predicts F0 per frame and sets this false.
    /// Upstream's voice feature is `no_f0_target_model`.
    pub f0_target_model: bool,
}

impl Default for VoiceProfile {
    fn default() -> VoiceProfile {
        VoiceProfile {
            sample_rate: 8000,
            duration_stretch: 1.0,
            f0_mean: 100.0,
            f0_stddev: 12.0,
            fold_ah_to_aa: false,
            segment_duration_model: true,
            f0_target_model: true,
        }
    }
}

impl VoiceProfile {
    /// The parameters a voice registers itself with.
    pub fn params(&self) -> VoiceParams {
        VoiceParams {
            int_f0_target_mean: self.f0_mean,
            int_f0_target_stddev: self.f0_stddev,
            duration_stretch: self.duration_stretch,
            f0_shift: 1.0,
            join_type: JoinType::ModifiedLpc,
            resynth_type: ResynthType::Fixed,
            fold_ah_to_aa: self.fold_ah_to_aa,
            segment_duration_model: self.segment_duration_model,
            f0_target_model: self.f0_target_model,
        }
    }
}

/// A waveform generator.
///
/// One implementation per synthesis technique. [`crate::Engine`] holds
/// `Arc<dyn Backend>` rather than being generic over this, because
/// `register_voice` means one engine holds voices of different kinds at once
/// and a type parameter would force them to agree.
pub trait Backend: Send + Sync {
    /// The voice's neutral settings. The pipeline reads this before analysing,
    /// so a backend that predicts its own durations can say so.
    fn profile(&self) -> VoiceProfile;

    /// Generate the waveform for a fully analysed utterance, handing each
    /// block of samples to `sink` as it is produced.
    ///
    /// Nothing larger than one utterance is held, so a caller that plays or
    /// writes the audio as it arrives uses memory that does not grow with the
    /// length of the speech. Returning [`Flow::Stop`] ends synthesis where it
    /// stands.
    fn synthesise_streaming(
        &self,
        utt: &Utterance,
        params: &VoiceParams,
        sink: &mut dyn FnMut(&[i16]) -> Flow,
    );

    /// The same synthesis, collected into one buffer.
    fn synthesise(&self, utt: &Utterance, params: &VoiceParams) -> Audio {
        let mut samples = Vec::new();
        self.synthesise_streaming(utt, params, &mut |block| {
            samples.extend_from_slice(block);
            Flow::Continue
        });
        Audio {
            samples,
            sample_rate: self.sample_rate(),
        }
    }

    /// The rate this backend produces, which is the profile's.
    fn sample_rate(&self) -> u32 {
        self.profile().sample_rate
    }
}

/// How the selected units are turned into a waveform. Diphone only; the field
/// lives on [`VoiceParams`] because that mirrors upstream's voice features.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    /// Select units but generate nothing, for analysing without paying for the
    /// audio. Upstream accepts the value but leaves the utterance without a
    /// waveform, so its own callers cannot use it; here the result is empty
    /// audio.
    None,
    /// Concatenate the recorded pitch periods unchanged. The recorded pitch
    /// and timing survive and the predicted contour is ignored, so this sounds
    /// like the original speaker reading the diphones.
    Simple,
    /// Respace the periods onto the predicted pitch contour, which is what
    /// makes the voice say something it never recorded.
    ModifiedLpc,
}

/// Which arithmetic the LPC synthesis filter uses. Diphone only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResynthType {
    /// Integer throughout, so the output is identical on every target. Every
    /// diphone voice this crate ships asks for this.
    Fixed,
    /// Floating point. Upstream's default for a modified-LPC join, and subject
    /// to the target's floating-point behaviour.
    ///
    /// Samples match upstream exactly, but the length does not: upstream sizes
    /// this path's output from the last pitch mark and, unlike its fixed-point
    /// filter, never trims it back to what was generated, so it returns up to
    /// an eighth of a second of trailing silence. That is not reproduced here.
    Float,
}

/// The parameters upstream Flite exposes as features on a voice.
///
/// The field names are upstream's, so that a value here can be checked against
/// the `register_*` function it was taken from without a translation step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoiceParams {
    /// Mean pitch in Hz. The intonation model's predictions are mapped onto
    /// this and [`Self::int_f0_target_stddev`].
    pub int_f0_target_mean: f32,
    /// Pitch spread in Hz around that mean.
    pub int_f0_target_stddev: f32,
    /// Multiplies every segment duration; above 1.0 is slower speech. A voice
    /// sets this to suit its own recordings, so it is rarely 1.0.
    pub duration_stretch: f32,
    /// Multiplies the pitch mean; above 1.0 is a higher voice.
    pub f0_shift: f32,
    pub join_type: JoinType,
    pub resynth_type: ResynthType,
    /// Whether this voice's postlexical rules rewrite `ah` as `aa`.
    ///
    /// Upstream gives each voice a whole postlex function and this is the only
    /// rule the two bundled diphone voices differ by. It belongs to the voice,
    /// not the language: the 8 kHz database records the vowel only as `aa`,
    /// while the 16 kHz one has both. Applying it to a voice that did not ask
    /// for it changes the phone, and with it the duration the model predicts.
    pub fold_ah_to_aa: bool,
    /// Whether to run the language's segment-duration model. See
    /// [`VoiceProfile::segment_duration_model`].
    pub segment_duration_model: bool,
    /// Whether to run the language's F0 target model. See
    /// [`VoiceProfile::f0_target_model`].
    pub f0_target_model: bool,
}

impl Default for VoiceParams {
    /// Upstream's fallbacks for a voice that sets nothing, not the bundled
    /// voice's values. [`crate::Engine`] starts from the voice's own.
    fn default() -> VoiceParams {
        VoiceParams {
            int_f0_target_mean: 100.0,
            int_f0_target_stddev: 12.0,
            duration_stretch: 1.0,
            f0_shift: 1.0,
            join_type: JoinType::ModifiedLpc,
            resynth_type: ResynthType::Float,
            fold_ah_to_aa: false,
            segment_duration_model: true,
            f0_target_model: true,
        }
    }
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
