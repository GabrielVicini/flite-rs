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

pub use dsp::Flow;
pub use lexicon::LexiconError;
pub use utterance::Utterance;
pub use voice::{Audio, JoinType, ResynthType, Voice, VoiceParams};
pub use wav::{write_wav, WavWriter};

use language::Language;
use std::sync::Arc;

/// US English models: lexicon, letter-to-sound rules, and the CART models for
/// phrasing, part of speech, intonation and duration.
pub(crate) static EN_US_DATA: &[u8] = include_bytes!("../data/en_us.dat");

/// The `cmu_us_kal` diphone voice: 8 kHz, male, American English.
///
/// Public so that a caller can register a second voice from the same
/// recordings with different settings, which is the cheapest way to get a
/// second voice: the data is already in the binary.
pub static KAL_VOICE_DATA: &[u8] = include_bytes!("../data/cmu_us_kal.dat");

/// The same speaker at 16 kHz, which is the largest quality difference on
/// offer. Its residual is not compressed, so it is nearly three times the size
/// of the 8 kHz voice; that is why it is behind a feature.
#[cfg(feature = "kal16")]
pub static KAL16_VOICE_DATA: &[u8] = include_bytes!("../data/cmu_us_kal16.dat");

/// One registered voice: its data, the language it speaks, and its settings.
struct Registered {
    name: String,
    voice: Voice,
    /// Shared, because several voices of the same language use one set of
    /// models and those are the large part.
    language: Arc<Language>,
    params: VoiceParams,
    /// The stretch this voice was registered with, which
    /// [`Engine::set_duration_stretch`] treats as its natural rate.
    natural_duration_stretch: f32,
}

/// A ready-to-use synthesiser.
///
/// Holds the parsed language models and voices. Cheap to use, not cheap to
/// construct, so create one and share it.
pub struct Engine {
    voices: Vec<Registered>,
    selected: usize,
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
        let mut engine = Engine {
            voices: Vec::new(),
            selected: 0,
        };
        let language = Arc::new(Language::parse(EN_US_DATA)?);

        // The higher-rate voice is registered first so that the 8 kHz one is
        // the selected default whether or not the feature is on: a build flag
        // should add a choice, not silently change what everyone hears.
        #[cfg(feature = "kal16")]
        {
            let voice = Voice::parse(KAL16_VOICE_DATA)?;
            let params = voice.params();
            engine.register_voice("kal16", voice, Arc::clone(&language), params);
        }
        // Upstream registers this voice under its speaker's name.
        let voice = Voice::parse(KAL_VOICE_DATA)?;
        let params = voice.params();
        engine.register_voice("kal", voice, language, params);
        Ok(engine)
    }

    /// Add a voice and select it.
    ///
    /// Pass an existing [`Engine::language`] to share one set of models between
    /// voices rather than parsing them again. A name already in use is
    /// replaced.
    pub fn register_voice(
        &mut self,
        name: &str,
        voice: Voice,
        language: Arc<Language>,
        params: VoiceParams,
    ) {
        let registered = Registered {
            name: name.to_string(),
            voice,
            language,
            params,
            natural_duration_stretch: params.duration_stretch,
        };
        match self.voices.iter().position(|v| v.name == name) {
            Some(existing) => {
                self.voices[existing] = registered;
                self.selected = existing;
            }
            None => {
                self.voices.push(registered);
                self.selected = self.voices.len() - 1;
            }
        }
    }

    /// The names of every registered voice, in registration order.
    pub fn voice_names(&self) -> impl Iterator<Item = &str> {
        self.voices.iter().map(|v| v.name.as_str())
    }

    /// The voice currently being spoken with.
    pub fn voice_name(&self) -> &str {
        &self.current().name
    }

    /// Speak with a different registered voice, reporting whether there is one
    /// by that name. A failed selection leaves the current voice in place.
    pub fn select_voice(&mut self, name: &str) -> bool {
        match self.voices.iter().position(|v| v.name == name) {
            Some(found) => {
                self.selected = found;
                true
            }
            None => false,
        }
    }

    /// The models the current voice speaks with, for registering a second
    /// voice of the same language without parsing them twice.
    pub fn language(&self) -> Arc<Language> {
        Arc::clone(&self.current().language)
    }

    /// Teach the current voice some pronunciations, written as
    /// `word [pos] : phone phone phone`, one per line.
    ///
    /// ```no_run
    /// # let mut engine = flite_rs::Engine::new();
    /// engine.add_lex_entries("kubernetes : k uw b er n eh1 t iy z")?;
    /// # Ok::<(), flite_rs::LexiconError>(())
    /// ```
    ///
    /// Added words are looked up before the compiled dictionary, so this also
    /// corrects a word that is already in it. A phone the voice does not have
    /// is refused here rather than mispronounced later, and a file with one bad
    /// line adds nothing.
    ///
    /// Voices sharing one set of models keep sharing it until this is called;
    /// the models are copied at that point, so the addition applies to the
    /// voice that is selected and not to its siblings.
    pub fn add_lex_entries(&mut self, entries: &str) -> Result<usize, LexiconError> {
        let selected = self.selected;
        let language = Arc::make_mut(&mut self.voices[selected].language);
        language.lexicon.add_entries(entries)
    }

    fn current(&self) -> &Registered {
        &self.voices[self.selected]
    }

    /// Speech rate, where 1.0 is the voice's natural rate. Values above 1.0
    /// lengthen every segment.
    ///
    /// This multiplies the voice's own stretch rather than replacing it, so
    /// 1.0 here is not 1.0 in [`VoiceParams::duration_stretch`]. Set that field
    /// directly through [`Engine::params_mut`] to work in upstream's units.
    pub fn set_duration_stretch(&mut self, stretch: f32) {
        let natural = self.current().natural_duration_stretch;
        self.params_mut().duration_stretch = stretch.max(0.05) * natural;
    }

    /// Pitch multiplier applied to the target mean; 1.0 leaves it unchanged.
    pub fn set_f0_shift(&mut self, shift: f32) {
        self.params_mut().f0_shift = shift.max(0.1);
    }

    /// The current voice's parameters in full, for settings the two helpers
    /// above do not cover.
    pub fn params(&self) -> &VoiceParams {
        &self.current().params
    }

    /// The current voice's parameters, mutably. Values are used as given:
    /// unlike [`Engine::set_duration_stretch`] nothing is clamped or rescaled.
    pub fn params_mut(&mut self) -> &mut VoiceParams {
        let selected = self.selected;
        &mut self.voices[selected].params
    }

    /// The sample rate of everything this engine produces.
    ///
    /// Voices may differ in this, so read it again after selecting one.
    pub fn sample_rate(&self) -> u32 {
        self.current().voice.sample_rate
    }

    /// Synthesise text to audio.
    ///
    /// The text is split into sentences and each is synthesised in turn, so
    /// passing a whole paragraph is both correct and efficient. Empty input
    /// yields empty audio rather than an error.
    pub fn synthesize(&self, text: &str) -> Audio {
        let mut samples = Vec::new();
        self.synthesize_streaming(text, |period| {
            samples.extend_from_slice(period);
            Flow::Continue
        });
        Audio {
            samples,
            sample_rate: self.sample_rate(),
        }
    }

    /// Synthesise text, handing each pitch period to `sink` as it is produced.
    ///
    /// Sentences are analysed one at a time and nothing is accumulated, so a
    /// caller that plays or writes the audio as it arrives holds no more for an
    /// hour of speech than for a second of it. Returning [`Flow::Stop`] ends
    /// synthesis where it stands.
    pub fn synthesize_streaming<F>(&self, text: &str, mut sink: F)
    where
        F: FnMut(&[i16]) -> Flow,
    {
        let current = self.current();
        let mut stopped = false;
        for sentence in text::sentences(text) {
            if stopped {
                return;
            }
            let utt = pipeline::analyse(&current.language, &sentence, &current.params);
            voice::synthesise_streaming(&current.voice, &utt, &current.params, &mut |period| {
                let flow = sink(period);
                stopped = flow == Flow::Stop;
                flow
            });
        }
    }

    /// Synthesise everything a reader produces, without holding it all.
    ///
    /// Text is consumed in fixed-size chunks and grouped into sentences as the
    /// tokens arrive, so neither the input nor the output is ever fully in
    /// memory. This is what to use for a file of any size.
    pub fn synthesize_reader<R, F>(&self, reader: R, mut sink: F) -> std::io::Result<()>
    where
        R: std::io::Read,
        F: FnMut(&[i16]) -> Flow,
    {
        let current = self.current();
        let mut speak = |sentence: &[text::Token]| {
            let utt = pipeline::analyse(&current.language, sentence, &current.params);
            let mut flow = Flow::Continue;
            voice::synthesise_streaming(&current.voice, &utt, &current.params, &mut |period| {
                flow = sink(period);
                flow
            });
            flow
        };

        let mut sentences = text::SentenceBuilder::default();
        let mut tokens = text::ChunkedTokens::new(reader);
        while let Some(token) = tokens.next_token()? {
            if let Some(sentence) = sentences.push(token) {
                if speak(&sentence) == Flow::Stop {
                    return Ok(());
                }
            }
        }
        if let Some(sentence) = sentences.finish() {
            speak(&sentence);
        }
        Ok(())
    }

    /// Synthesise a phone string directly, as in `"pau hh ax l ow pau"`.
    ///
    /// Nothing is looked up and no intonation is predicted: the phones are
    /// spoken in order, timed by the duration model, on a straight pitch line.
    /// A `-` between phones is a syllable boundary and a trailing `0` or `1`
    /// marks that syllable's stress, both of which the duration model reads.
    ///
    /// Use it to say something the lexicon gets wrong, or to hear exactly what
    /// [`Engine::phones`] printed.
    pub fn synthesize_phones(&self, phones: &str) -> Audio {
        let mut samples = Vec::new();
        self.synthesize_phones_streaming(phones, |period| {
            samples.extend_from_slice(period);
            Flow::Continue
        });
        Audio {
            samples,
            sample_rate: self.sample_rate(),
        }
    }

    /// [`Engine::synthesize_phones`], a pitch period at a time.
    pub fn synthesize_phones_streaming<F>(&self, phones: &str, mut sink: F)
    where
        F: FnMut(&[i16]) -> Flow,
    {
        // A phone string is one utterance however many sentences it looks
        // like, so the tokens are not split.
        let tokens: Vec<text::Token> = text::tokenize(phones).into_iter().flatten().collect();
        let current = self.current();
        let utt = pipeline::analyse_phones(&current.language, &tokens, &current.params);
        voice::synthesise_streaming(&current.voice, &utt, &current.params, &mut sink);
    }

    /// Run the phone-string pipeline and return the utterance, for inspection.
    pub fn analyse_phones(&self, phones: &str) -> Utterance {
        let tokens: Vec<text::Token> = text::tokenize(phones).into_iter().flatten().collect();
        let current = self.current();
        pipeline::analyse_phones(&current.language, &tokens, &current.params)
    }

    /// The phone sequence that would be spoken, space separated.
    ///
    /// Useful for checking pronunciation without generating audio, and for
    /// diagnosing a word that comes out wrong.
    pub fn phones(&self, text: &str) -> String {
        text::tokenize(text)
            .iter()
            .map(|sentence| {
                let current = self.current();
                let utt = pipeline::analyse(&current.language, sentence, &current.params);
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
        let current = self.current();
        pipeline::analyse(&current.language, &tokens, &current.params)
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
