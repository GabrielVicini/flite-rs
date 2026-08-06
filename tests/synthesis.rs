//! End-to-end tests.
//!
//! The expectations here were taken from reference Flite output, so they are
//! not just "this looks plausible": they pin the pronunciations and the
//! normalisation decisions the models actually make. If one of these changes,
//! the engine has diverged from the models it ships.

use flite_rs::Engine;

fn engine() -> Engine {
    Engine::new()
}

/// Phones with the surrounding silences stripped, for readable assertions.
fn spoken(engine: &Engine, text: &str) -> String {
    engine
        .phones(text)
        .split_whitespace()
        .filter(|p| *p != "pau")
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn dictionary_and_letter_to_sound_both_work() {
    let e = engine();
    // In the dictionary, because the rules get it wrong.
    assert_eq!(spoken(&e, "hello"), "hh ax l ow");
    // Not in the dictionary: the rules handle it, which is the normal case.
    assert_eq!(spoken(&e, "computer"), "k ax m p y uw t er");
    assert_eq!(spoken(&e, "synthesis"), "s ih n th ax s ax s");
}

#[test]
fn numbers_are_read_according_to_context() {
    let e = engine();
    assert_eq!(spoken(&e, "1984"), "n ay n t iy n ey t iy f ao r");
    assert_eq!(spoken(&e, "The 3 books"), "dh ax th r iy b uh k s");
    assert_eq!(spoken(&e, "21st"), "t w eh n t iy f er s t");
}

#[test]
fn money_is_read_with_its_unit() {
    let e = engine();
    assert!(
        spoken(&e, "$1,000,000").contains("m ih l y ax n"),
        "expected a million: {}",
        spoken(&e, "$1,000,000")
    );
    assert!(spoken(&e, "$1").ends_with("d aa l er"), "singular dollar");
    assert!(spoken(&e, "$5").ends_with("d aa l er z"), "plural dollars");
}

#[test]
fn abbreviations_expand_from_context() {
    let e = engine();
    assert!(spoken(&e, "Mr. Jones").starts_with("m ih s t er"));
    // "St" before a lowercase word is a street, after one it is a saint.
    assert!(spoken(&e, "Main St, where").contains("s t r iy t"));
}

#[test]
fn unpronounceable_letter_strings_are_spelled_out() {
    let e = engine();
    // "TS" cannot be said as a word, so it is spelled.
    assert_eq!(spoken(&e, "TS"), "t iy eh s");
    // "NASA" can be, so it is not.
    assert_eq!(spoken(&e, "NASA"), "n ae s ax");
}

#[test]
fn possessive_s_gets_a_schwa_after_a_sibilant() {
    let e = engine();
    // "boss's" needs the extra vowel; "cat's" does not, and devoices.
    assert!(spoken(&e, "the boss's").ends_with("ax z"));
    assert!(spoken(&e, "the cat's").ends_with("t s"));
}

#[test]
fn the_changes_vowel_before_a_vowel() {
    let e = engine();
    assert!(spoken(&e, "the apple").starts_with("dh iy"));
    assert!(spoken(&e, "the pear").starts_with("dh ax"));
}

#[test]
fn every_sentence_is_spoken_and_separated_by_silence() {
    let e = engine();
    // Each sentence gets its own leading and trailing silence.
    let phones = e.phones("The first one. The second one.");
    assert_eq!(phones.matches("pau").count(), 4, "phones: {phones}");
}

#[test]
fn a_short_capitalised_word_before_a_period_reads_as_an_abbreviation() {
    let e = engine();
    // "One." looks like an initialism, so this stays a single sentence, the
    // same judgement that keeps "Dr. Smith" together.
    assert_eq!(e.phones("One. Two.").matches("pau").count(), 3);
}

#[test]
fn audio_is_well_formed() {
    let e = engine();
    let audio = e.synthesize("This is a test of the synthesiser.");
    assert_eq!(audio.sample_rate, 8000);
    assert!(audio.duration_seconds() > 1.0);
    // Real speech: not silent, and not clipping.
    assert!(audio.samples.iter().any(|s| s.abs() > 1000));
    assert!(audio.samples.iter().all(|s| *s != i16::MIN));
}

#[test]
fn wav_output_round_trips_through_its_own_header() {
    let e = engine();
    let audio = e.synthesize("Round trip.");
    let mut bytes = Vec::new();
    flite_rs::write_wav(&audio, &mut bytes).expect("writing to a Vec cannot fail");

    assert_eq!(&bytes[0..4], b"RIFF");
    let declared = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) as usize;
    assert_eq!(declared, audio.samples.len() * 2);
    assert_eq!(bytes.len(), 44 + declared);
}

#[test]
fn prosody_controls_have_the_expected_direction() {
    let mut e = engine();
    let text = "Testing the prosody controls.";
    let normal = e.synthesize(text).samples.len();

    e.set_duration_stretch(1.5);
    assert!(
        e.synthesize(text).samples.len() > normal,
        "slower is longer"
    );

    e.set_duration_stretch(0.7);
    assert!(
        e.synthesize(text).samples.len() < normal,
        "faster is shorter"
    );

    // Pitch changes the waveform without changing the schedule much.
    e.set_duration_stretch(1.0);
    let low = e.synthesize(text);
    e.set_f0_shift(1.5);
    let high = e.synthesize(text);
    assert_ne!(low.samples, high.samples);
}

#[test]
fn awkward_input_does_not_panic() {
    let e = engine();
    for text in [
        "",
        " ",
        "\n\n\n",
        ".",
        "?!.,;:",
        "\"'`([{",
        "a",
        "\u{2019}",   // a lone typographic apostrophe
        "café naïve", // non-ASCII letters
        "日本語",     // no Latin letters at all
        "$",
        "$.",
        "1/0",
        "-",
        "--",
        "1-",
        "-1",
        "1.2.3.4",
        "12:99",
        "0:00",
        "999999999999999999999",
        "0.000000001",
        "1e400",
        &"very ".repeat(200),
        &"x".repeat(500),
    ] {
        let audio = e.synthesize(text);
        assert!(
            audio.samples.len() < 8000 * 600,
            "implausible output for {text:?}"
        );
        // Analysis must survive the same inputs.
        let _ = e.phones(text);
    }
}

#[test]
fn a_shared_engine_can_be_used_from_several_threads() {
    let engine = std::sync::Arc::new(engine());
    let expected = engine.synthesize("Concurrent synthesis.").samples;

    let handles: Vec<_> = (0..4)
        .map(|_| {
            let engine = std::sync::Arc::clone(&engine);
            std::thread::spawn(move || engine.synthesize("Concurrent synthesis.").samples)
        })
        .collect();

    for handle in handles {
        assert_eq!(handle.join().expect("thread panicked"), expected);
    }
}

#[test]
fn a_segment_can_see_the_token_it_came_from() {
    // The per-token prosody overrides in the duration and F0 models are read
    // through these paths. Nothing sets those features yet, so they would read
    // as absent whether the walk worked or not; check the walk itself with a
    // feature that does have a value.
    let e = engine();
    let utt = e.analyse("Hello there.");
    let segment = utt
        .iter_relation("Segment")
        .nth(1)
        .expect("the utterance has segments after the leading silence");

    assert_eq!(
        flite_rs::ffeature::eval_str(
            &utt,
            segment,
            "R:SylStructure.parent.parent.R:Token.parent.name"
        )
        .as_str(),
        "Hello"
    );
    // The F0 model walks from a syllable, one step shorter.
    let syllable = utt
        .iter_relation("Syllable")
        .next()
        .expect("the utterance has syllables");
    assert_eq!(
        flite_rs::ffeature::eval_str(&utt, syllable, "R:SylStructure.parent.R:Token.parent.name")
            .as_str(),
        "Hello"
    );
}

#[test]
fn voice_parameters_are_reachable_and_take_effect() {
    let mut e = engine();
    // The convenience setter works in multiples of the voice's own rate, so
    // its 1.0 is the voice's 1.1.
    assert_eq!(e.params().duration_stretch, 1.1);
    e.set_duration_stretch(2.0);
    assert_eq!(e.params().duration_stretch, 2.2);

    e.set_duration_stretch(1.0);
    let normal = e.synthesize("Parameters.").samples.len();
    e.params_mut().duration_stretch = 2.2;
    assert!(e.synthesize("Parameters.").samples.len() > normal);

    // Selecting units without generating audio.
    e.params_mut().join_type = flite_rs::JoinType::None;
    assert!(e.synthesize("Parameters.").samples.is_empty());
}

/// A reader that hands over one byte at a time, so that every token, sentence
/// and multi-byte character in a test straddles a chunk boundary.
struct Trickle<'a>(&'a [u8]);

impl std::io::Read for Trickle<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.0.is_empty() || out.is_empty() {
            return Ok(0);
        }
        out[0] = self.0[0];
        self.0 = &self.0[1..];
        Ok(1)
    }
}

const STREAMING_TEXT: &str = "The first sentence. A second one, longer than the first!\n\n\
                              After a blank line. Dr. Smith stays put. Caf\u{e9} na\u{ef}ve.\n";

#[test]
fn streaming_produces_exactly_the_buffered_samples() {
    let e = engine();
    let mut streamed = Vec::new();
    e.synthesize_streaming(STREAMING_TEXT, |period| {
        streamed.extend_from_slice(period);
        flite_rs::Flow::Continue
    });
    assert_eq!(streamed, e.synthesize(STREAMING_TEXT).samples);
}

#[test]
fn reading_in_chunks_produces_exactly_the_same_samples() {
    let e = engine();
    // Long enough that the reader's internal buffer is refilled, so that a
    // token, a sentence break and a blank line all land on a boundary.
    let text = STREAMING_TEXT.repeat(80);
    assert!(text.len() > 8192, "the chunk boundary must be crossed");
    let expected = e.synthesize(&text).samples;

    fn read<R: std::io::Read>(engine: &Engine, reader: R) -> Vec<i16> {
        let mut streamed = Vec::new();
        engine
            .synthesize_reader(reader, |period| {
                streamed.extend_from_slice(period);
                flite_rs::Flow::Continue
            })
            .expect("reading from a slice cannot fail");
        streamed
    }

    // Whole chunks, as a file would give.
    assert_eq!(read(&e, text.as_bytes()), expected);
    // One byte at a time, which puts a boundary inside every token and inside
    // the multi-byte characters.
    assert_eq!(read(&e, Trickle(text.as_bytes())), expected);
}

#[test]
fn a_sink_can_stop_synthesis_early() {
    let e = engine();
    let text = "One sentence. Another sentence. A third sentence.";
    let full = e.synthesize(text).samples.len();

    let mut periods = 0;
    let mut samples = 0;
    e.synthesize_streaming(text, |period| {
        periods += 1;
        samples += period.len();
        if periods == 5 {
            flite_rs::Flow::Stop
        } else {
            flite_rs::Flow::Continue
        }
    });

    assert_eq!(periods, 5, "synthesis continued past the stop");
    assert!(
        samples < full,
        "stopping produced the whole utterance anyway"
    );
}

#[test]
fn voices_can_be_registered_and_selected_by_name() {
    let mut e = engine();
    // The 8 kHz voice is selected whatever else is compiled in, so that a
    // build flag adds a choice rather than changing what everyone hears.
    assert_eq!(e.voice_name(), "kal");
    assert!(e.voice_names().any(|n| n == "kal"));
    assert!(
        !e.select_voice("nobody"),
        "an unknown name cannot be chosen"
    );
    assert_eq!(e.voice_name(), "kal", "a failed selection changes nothing");

    // A second voice sharing one set of language models, which is the case
    // registration is shaped for.
    let mut params = *e.params();
    params.int_f0_target_mean = 130.0;
    e.register_voice(
        "higher",
        flite_rs::Voice::parse(flite_rs::KAL_VOICE_DATA).expect("bundled data is valid"),
        e.language(),
        params,
    );

    assert_eq!(e.voice_name(), "higher", "registering selects");
    assert_eq!(
        e.voice_names().last(),
        Some("higher"),
        "a new voice goes on the end"
    );
    let higher = e.synthesize("Comparing voices.").samples;

    assert!(e.select_voice("kal"));
    assert_eq!(
        e.params().int_f0_target_mean,
        95.0,
        "settings are per voice"
    );
    assert_ne!(higher, e.synthesize("Comparing voices.").samples);
}

#[test]
fn a_phone_string_says_what_it_spells() {
    let e = engine();
    let audio = e.synthesize_phones("pau hh ax l ow pau");
    assert_eq!(audio.sample_rate, 8000);
    assert!(audio.samples.iter().any(|s| s.abs() > 1000), "silent");

    // What `phones` prints is what `synthesize_phones` accepts, so a
    // pronunciation can be printed, corrected and spoken back.
    assert_eq!(e.phones("hello"), "pau hh ax l ow pau");
    assert_eq!(
        e.synthesize_phones(&e.phones("hello")).samples,
        audio.samples
    );
}

#[test]
fn a_phone_string_can_mark_syllables_and_stress() {
    let e = engine();
    // A syllable break changes the duration model's answers, so it must change
    // the audio. Upstream documents `-` and then aborts on it; this is the
    // behaviour it describes.
    let flat = e.synthesize_phones("pau t eh s t ih ng pau").samples;
    let split = e.synthesize_phones("pau t eh1 s t - ih0 ng pau").samples;
    assert!(!split.is_empty(), "a syllable break produced no audio");
    assert_ne!(flat, split, "the syllable break changed nothing");

    // The break itself is not a phone and must not become one.
    let utt = e.analyse_phones("pau t eh1 s t - ih0 ng pau");
    assert_eq!(
        utt.iter_relation("Segment")
            .map(|s| utt.name(s))
            .collect::<Vec<_>>(),
        ["pau", "t", "eh", "s", "t", "ih", "ng", "pau"]
    );
    assert_eq!(utt.iter_relation("Syllable").count(), 2);
}

#[test]
fn an_unknown_phone_is_dropped_rather_than_fatal() {
    let e = engine();
    let utt = e.analyse_phones("pau hh zzz ax pau");
    assert_eq!(
        utt.iter_relation("Segment")
            .map(|s| utt.name(s))
            .collect::<Vec<_>>(),
        ["pau", "hh", "ax", "pau"]
    );
}

#[test]
fn pronunciations_can_be_added_at_runtime() {
    let mut e = engine();
    // Wrong out of the box, because the letter-to-sound rules have never seen
    // it and the dictionary predates it.
    assert_ne!(spoken(&e, "kubernetes"), "k uw b er n eh t iy z");

    let added = e
        .add_lex_entries(
            "# A word the rules get wrong.\n\
             \n\
             kubernetes : k uw b er n eh1 t iy z\n\
             hello : hh ax l ow1 t iy  # overrides the dictionary\n",
        )
        .expect("both lines are well formed");
    assert_eq!(added, 2);

    assert_eq!(spoken(&e, "kubernetes"), "k uw b er n eh t iy z");
    // An added word beats the compiled dictionary, which is what makes this
    // useful for fixing a pronunciation rather than only adding one.
    assert_eq!(spoken(&e, "hello"), "hh ax l ow t iy");
    // The stress digit is kept for the models even though `phones` strips it.
    assert!(e.synthesize("kubernetes").samples.len() > 1000);
}

#[test]
fn a_bad_pronunciation_is_refused_whole() {
    let mut e = engine();
    let before = spoken(&e, "hello");

    let error = e
        .add_lex_entries("good : g uh d\nbad : hh zzz ow\n")
        .expect_err("zzz is not a phone");
    assert_eq!(
        error,
        flite_rs::LexiconError::UnknownPhone {
            line: 2,
            phone: "zzz".to_string()
        }
    );
    // The good line on either side of a bad one is not kept, so a rejected
    // file leaves the dictionary exactly as it was.
    assert_eq!(spoken(&e, "good"), spoken(&engine(), "good"));
    assert_eq!(spoken(&e, "hello"), before);

    assert!(e.add_lex_entries("no colon here").is_err());
}

#[test]
fn added_words_belong_to_the_voice_that_learned_them() {
    let mut e = engine();
    let mut params = *e.params();
    params.int_f0_target_mean = 120.0;
    e.register_voice(
        "second",
        flite_rs::Voice::parse(flite_rs::KAL_VOICE_DATA).expect("bundled data is valid"),
        e.language(),
        params,
    );

    e.add_lex_entries("kubernetes : k uw b er n eh1 t iy z")
        .expect("well formed");
    let taught = spoken(&e, "kubernetes");

    assert!(e.select_voice("kal"));
    assert_ne!(
        spoken(&e, "kubernetes"),
        taught,
        "the other voice should not have learned it"
    );
}

#[test]
fn synthesis_is_deterministic() {
    let e = engine();
    let text = "Determinism matters for testing.";
    assert_eq!(e.synthesize(text).samples, e.synthesize(text).samples);
}
