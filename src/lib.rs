//! A small, fast, portable text-to-speech engine.
//!
//! `flite-rs` is a clean-room Rust implementation of the diphone concatenative
//! synthesis techniques used by CMU's Flite and Festival: CART-based duration,
//! phrasing and intonation models, rule-based letter-to-sound, and
//! residual-excited LPC waveform generation. It carries no C code and no
//! runtime dependencies, and the models and voice data are embedded in the
//! binary, so there is nothing to install and nothing to load from disk.
//!
//! # Quick start
//!
//! ```no_run
//! let engine = flite_rs::Engine::new();
//! let audio = engine.synthesize("Hello from Rust.");
//! println!("{} samples at {} Hz", audio.samples.len(), audio.sample_rate);
//! ```
//!
//! Writing a WAV file:
//!
//! ```no_run
//! # let engine = flite_rs::Engine::new();
//! let audio = engine.synthesize("The quick brown fox.");
//! let mut file = std::fs::File::create("out.wav").unwrap();
//! flite_rs::write_wav(&audio, &mut file).unwrap();
//! ```
//!
//! Adjusting speed and pitch:
//!
//! ```no_run
//! # let mut engine = flite_rs::Engine::new();
//! engine.set_duration_stretch(1.3); // slower
//! engine.set_f0_shift(1.1);         // higher pitch
//! ```
//!
//! # How it works
//!
//! Synthesis runs in two halves. The [linguistic pipeline](pipeline) turns
//! text into a sequence of phones with durations and a pitch contour; the
//! [voice](voice) turns that into a waveform by splicing together recorded
//! diphones and resynthesising them at the requested pitch. The intermediate
//! structure is a heterogeneous relation graph ([`utterance::Utterance`]),
//! which every model queries through [feature paths](ffeature).
//!
//! # Threading
//!
//! [`Engine`] is [`Send`] and [`Sync`] and all synthesis methods take `&self`,
//! so one engine can serve many threads. Construction parses the embedded
//! tables, so build one and keep it.

// The whole engine is safe Rust: the data files are parsed with bounds-checked
// slice reads rather than transmuted, which also makes them endian-independent.
#![forbid(unsafe_code)]

pub mod cart;
pub mod data;
pub mod dsp;
pub mod ffeature;
pub mod lang;
pub mod language;
pub mod lexicon;
pub mod normalize;
pub mod numbers;
pub mod patterns;
pub mod phoneset;
pub mod pipeline;
pub mod text;
pub mod utterance;
pub mod value;
pub mod voice;
pub mod wav;

pub use utterance::Utterance;
pub use voice::{Audio, JoinType, ResynthType, Voice, VoiceParams};
pub use wav::write_wav;

use language::Language;

/// US English models: lexicon, letter-to-sound rules, and the CART models for
/// phrasing, part of speech, intonation and duration.
pub(crate) static EN_US_DATA: &[u8] = include_bytes!("../data/en_us.dat");

/// The `cmu_us_kal` diphone voice: 8 kHz, male, American English.
static KAL_VOICE_DATA: &[u8] = include_bytes!("../data/cmu_us_kal.dat");

/// Duration multiplier baked into the bundled voice, tuned to its recordings.
/// User-supplied stretch multiplies this rather than replacing it, so
/// `set_duration_stretch(1.0)` means "the voice's natural rate".
const VOICE_DURATION_STRETCH: f32 = 1.1;
/// Default pitch mean and spread in Hz for the bundled voice.
const DEFAULT_F0_MEAN: f32 = 95.0;
const DEFAULT_F0_STDDEV: f32 = 11.0;

/// A ready-to-use synthesiser.
///
/// Holds the parsed language models and voice. Cheap to use, not cheap to
/// construct, so create one and share it.
pub struct Engine {
    language: Language,
    voice: Voice,
    params: VoiceParams,
}

impl Engine {
    /// Build an engine with the bundled US English voice.
    ///
    /// # Panics
    ///
    /// Only if the embedded data files are corrupt, which would mean the crate
    /// itself was built wrong. Use [`Engine::try_new`] if you would rather
    /// handle that as an error.
    pub fn new() -> Engine {
        Engine::try_new().expect("embedded data files are valid")
    }

    /// Build an engine, reporting malformed embedded data instead of panicking.
    pub fn try_new() -> Result<Engine, data::DataError> {
        Ok(Engine {
            language: Language::parse(EN_US_DATA)?,
            voice: Voice::parse(KAL_VOICE_DATA)?,
            params: VoiceParams {
                int_f0_target_mean: DEFAULT_F0_MEAN,
                int_f0_target_stddev: DEFAULT_F0_STDDEV,
                duration_stretch: VOICE_DURATION_STRETCH,
                f0_shift: 1.0,
                join_type: JoinType::ModifiedLpc,
                resynth_type: ResynthType::Fixed,
            },
        })
    }

    /// Speech rate, where 1.0 is the voice's natural rate. Values above 1.0
    /// lengthen every segment.
    ///
    /// This multiplies the voice's own stretch rather than replacing it, so
    /// 1.0 here is not 1.0 in [`VoiceParams::duration_stretch`]. Set that field
    /// directly through [`Engine::params_mut`] to work in upstream's units.
    pub fn set_duration_stretch(&mut self, stretch: f32) {
        self.params.duration_stretch = stretch.max(0.05) * VOICE_DURATION_STRETCH;
    }

    /// Pitch multiplier applied to the target mean; 1.0 leaves it unchanged.
    pub fn set_f0_shift(&mut self, shift: f32) {
        self.params.f0_shift = shift.max(0.1);
    }

    /// The voice parameters in full, for settings the two helpers above do not
    /// cover.
    pub fn params(&self) -> &VoiceParams {
        &self.params
    }

    /// The voice parameters, mutably. Values are used as given: unlike
    /// [`Engine::set_duration_stretch`] nothing is clamped or rescaled.
    pub fn params_mut(&mut self) -> &mut VoiceParams {
        &mut self.params
    }

    /// The sample rate of everything this engine produces.
    pub fn sample_rate(&self) -> u32 {
        self.voice.sample_rate
    }

    /// Synthesise text to audio.
    ///
    /// The text is split into sentences and each is synthesised in turn, so
    /// passing a whole paragraph is both correct and efficient. Empty input
    /// yields empty audio rather than an error.
    pub fn synthesize(&self, text: &str) -> Audio {
        let mut samples = Vec::new();
        for sentence in text::tokenize(text) {
            let utt = pipeline::analyse(&self.language, &sentence, &self.params);
            samples.extend(voice::synthesise(&self.voice, &utt, &self.params).samples);
        }
        Audio {
            samples,
            sample_rate: self.voice.sample_rate,
        }
    }

    /// The phone sequence that would be spoken, space separated.
    ///
    /// Useful for checking pronunciation without generating audio, and for
    /// diagnosing a word that comes out wrong.
    pub fn phones(&self, text: &str) -> String {
        text::tokenize(text)
            .iter()
            .map(|sentence| {
                let utt = pipeline::analyse(&self.language, sentence, &self.params);
                pipeline::phone_string(&utt)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Run the linguistic pipeline over one sentence and return the resulting
    /// utterance, for callers that want to inspect or post-process it.
    ///
    /// Text spanning several sentences is analysed as one utterance here; use
    /// [`text::tokenize`] first if that is not what you want.
    pub fn analyse(&self, text: &str) -> Utterance {
        let tokens: Vec<text::Token> = text::tokenize(text).into_iter().flatten().collect();
        pipeline::analyse(&self.language, &tokens, &self.params)
    }
}

impl Default for Engine {
    fn default() -> Engine {
        Engine::new()
    }
}

/// Synthesise text with default settings.
///
/// Convenient for one-shot use; building an [`Engine`] is better when
/// synthesising repeatedly, since this parses the embedded data every call.
pub fn synthesize(text: &str) -> Audio {
    Engine::new().synthesize(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Engine>();
    }

    #[test]
    fn synthesises_audio_for_plain_text() {
        let engine = Engine::new();
        let audio = engine.synthesize("Hello world.");
        assert_eq!(audio.sample_rate, 8000);
        assert!(
            audio.duration_seconds() > 0.4,
            "expected roughly a second of audio, got {}",
            audio.duration_seconds()
        );
        assert!(audio.samples.iter().any(|s| *s != 0), "audio is silent");
    }

    #[test]
    fn empty_input_produces_empty_audio() {
        let engine = Engine::new();
        assert!(engine.synthesize("").samples.is_empty());
        assert!(engine.synthesize("   \n  ").samples.is_empty());
    }

    #[test]
    fn dictionary_words_use_their_recorded_pronunciation() {
        let engine = Engine::new();
        assert_eq!(engine.phones("hello"), "pau hh ax l ow pau");
    }

    #[test]
    fn unknown_words_fall_back_to_letter_to_sound_rules() {
        let engine = Engine::new();
        let phones = engine.phones("zorbling");
        assert!(phones.starts_with("pau z"), "unexpected phones: {phones}");
        assert!(phones.ends_with("pau"), "unexpected phones: {phones}");
    }

    #[test]
    fn numbers_are_read_as_words() {
        let engine = Engine::new();
        let phones = engine.phones("1984");
        // "nineteen eighty four" begins with the /n/ of "nineteen".
        assert!(
            phones.starts_with("pau n ay"),
            "unexpected phones: {phones}"
        );
    }

    #[test]
    fn longer_text_produces_proportionally_more_audio() {
        let engine = Engine::new();
        let short = engine.synthesize("One.").samples.len();
        let long = engine
            .synthesize("One two three four five six.")
            .samples
            .len();
        assert!(long > short * 2, "short {short}, long {long}");
    }

    #[test]
    fn duration_stretch_changes_length() {
        let mut engine = Engine::new();
        let normal = engine.synthesize("Testing one two three.").samples.len();
        engine.set_duration_stretch(2.0);
        let slow = engine.synthesize("Testing one two three.").samples.len();
        assert!(slow > normal, "normal {normal}, slow {slow}");
    }

    #[test]
    fn several_sentences_are_all_spoken() {
        let engine = Engine::new();
        let one = engine.synthesize("This is a test.").samples.len();
        let two = engine
            .synthesize("This is a test. This is another test.")
            .samples
            .len();
        assert!(two > one, "one {one}, two {two}");
    }
}
