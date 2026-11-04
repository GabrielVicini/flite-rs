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
        std::sync::Arc::new(
            flite_rs::DiphoneVoice::parse(flite_rs::KAL_VOICE_DATA).expect("bundled data is valid"),
        ),
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
        std::sync::Arc::new(
            flite_rs::DiphoneVoice::parse(flite_rs::KAL_VOICE_DATA).expect("bundled data is valid"),
        ),
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

// ---------------------------------------------------------------------------
// Clustergen.
//
// What upstream agrees with is checked in tests/reference.rs; these are the
// properties that hold with or without a reference binary to compare against.
// ---------------------------------------------------------------------------

/// The Clustergen voice goes through the ordinary voice-selection mechanism,
/// and nothing on its path is diphone-specific.
#[cfg(feature = "clustergen")]
#[test]
fn the_clustergen_voice_is_selected_like_any_other() {
    let mut engine = flite_rs::Engine::new();
    let names: Vec<&str> = engine.voice_names().collect();
    assert!(names.contains(&"slt"), "voices are {names:?}");
    assert!(names.contains(&"kal"), "voices are {names:?}");

    // The default is still the diphone voice: a feature should add a choice,
    // not change what a caller who asked for nothing hears.
    assert_eq!(engine.voice_name(), "kal");
    assert_eq!(engine.sample_rate(), 8000);

    assert!(engine.select_voice("slt"));
    assert_eq!(engine.voice_name(), "slt");
    assert_eq!(engine.sample_rate(), 16000);

    let audio = engine.synthesize("Hello from Clustergen.");
    assert_eq!(audio.sample_rate, 16000);
    assert!(
        audio.duration_seconds() > 0.8,
        "expected over a second, got {}",
        audio.duration_seconds()
    );
    assert!(audio.samples.iter().any(|s| *s != 0), "audio is silent");
}

/// The voice data names the backend; nothing maps a voice name to a
/// synthesiser.
#[cfg(feature = "clustergen")]
#[test]
fn the_backend_is_chosen_by_the_voice_data() {
    use flite_rs::data::{BackendKind, Container};

    let diphone = Container::parse(flite_rs::KAL_VOICE_DATA).expect("bundled data is valid");
    assert_eq!(diphone.backend().unwrap(), BackendKind::Diphone);

    let clustergen = Container::parse(flite_rs::SLT_VOICE_DATA).expect("bundled data is valid");
    assert_eq!(clustergen.backend().unwrap(), BackendKind::Clustergen);

    // And a voice asking for the wrong one is refused rather than mis-parsed.
    assert!(flite_rs::DiphoneVoice::parse(flite_rs::SLT_VOICE_DATA).is_err());
    assert!(flite_rs::clustergen::ClustergenVoice::parse(flite_rs::KAL_VOICE_DATA).is_err());
}

/// Switching voices switches synthesiser, and neither leaks into the other.
#[cfg(feature = "clustergen")]
#[test]
fn the_two_backends_coexist_in_one_engine() {
    let mut engine = flite_rs::Engine::new();
    let text = "This is a test of two voices.";

    let diphone = engine.synthesize(text);
    assert!(engine.select_voice("slt"));
    let clustergen = engine.synthesize(text);
    assert!(engine.select_voice("kal"));
    let diphone_again = engine.synthesize(text);

    assert_eq!(diphone.sample_rate, 8000);
    assert_eq!(clustergen.sample_rate, 16000);
    // Selecting the statistical voice and coming back leaves the diphone
    // voice exactly as it was.
    assert_eq!(diphone.samples, diphone_again.samples);
    // And the two are genuinely different synthesisers, not one rate-shifted.
    assert_ne!(diphone.samples.len() * 2, clustergen.samples.len());
}

/// Clustergen declines the language's prosody models and supplies its own.
///
/// If the pipeline ran them anyway the segment end times would be the duration
/// model's rather than the state trees', and there would be a `Target`
/// relation that nothing reads.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_predicts_its_own_durations_and_pitch() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    assert!(!engine.params().segment_duration_model);
    assert!(!engine.params().f0_target_model);

    let utt = engine.analyse("Hello world.");
    assert!(
        utt.head_of("Target").is_none(),
        "the F0 target model ran for a voice that declined it"
    );
    // The duration model did not run, so no segment has an end time yet: the
    // backend fills them in from its own state durations.
    let first = utt.head_of("Segment").expect("there are segments");
    assert_eq!(utt.feature_f32(first, "end"), 0.0);

    // The diphone voice still gets both.
    assert!(engine.select_voice("kal"));
    assert!(engine.params().segment_duration_model);
    let utt = engine.analyse("Hello world.");
    assert!(utt.head_of("Target").is_some());
    let first = utt.head_of("Segment").expect("there are segments");
    assert!(utt.feature_f32(first, "end") > 0.0);
}

/// Synthesis annotates the utterance with the structure Clustergen works on.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_builds_the_frame_structure_it_predicts_on() {
    use flite_rs::clustergen::ClustergenVoice;

    let voice = ClustergenVoice::parse(flite_rs::SLT_VOICE_DATA).expect("bundled data is valid");
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    let params = *engine.params();
    let mut utt = engine.analyse("Hello world.");

    let predicted = voice.trajectories(&mut utt, &params);

    let segments = utt.iter_relation("Segment").count();
    let states = utt.iter_relation("HMMstate").count();
    let frames = utt.iter_relation("mcep").count();
    assert!(segments > 0);
    // Three states per phone in this voice, so the states outnumber the
    // segments and the frames outnumber the states.
    assert!(states > segments, "{states} states for {segments} segments");
    assert!(frames > states, "{frames} frames for {states} states");
    assert_eq!(frames, predicted.track.len());
    assert_eq!(frames, predicted.smoothed.len());
    assert_eq!(frames, predicted.strengths.len());

    // Five bands of mixed excitation, and the smoothed track is F0 plus the
    // static cepstrum.
    assert_eq!(predicted.strengths[0].len(), 5);
    assert_eq!(predicted.smoothed[0].len(), 26);
    assert_eq!(predicted.track[0].len(), 102);

    // Segment end times rise and the last one is where the audio ends.
    let ends: Vec<f32> = utt
        .iter_relation("Segment")
        .map(|s| utt.feature_f32(s, "end"))
        .collect();
    assert!(ends.windows(2).all(|w| w[1] > w[0]), "{ends:?}");
}

/// The excitation continues across the sentences of one call.
///
/// Not an incidental property: upstream's pseudo-random source is global, so
/// the second sentence of a call is not the second sentence spoken alone, and
/// matching that is what `reading_a_file_in_chunks_matches_upstream_on_clustergen`
/// checks against the real thing. Here it is checked without a reference.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_speech_is_one_continuous_run_per_call() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));

    let first = engine.synthesize("Wow!!!");
    let second = engine.synthesize("Really???");
    let both = engine.synthesize("Wow!!! Really??? ");

    assert_eq!(
        both.samples.len(),
        first.samples.len() + second.samples.len()
    );
    // The first sentence is the same either way, because the run starts fresh.
    assert_eq!(both.samples[..first.samples.len()], first.samples[..]);
    // The second is not, because by then the excitation has moved on.
    assert_ne!(&both.samples[first.samples.len()..], &second.samples[..]);
}

/// Two calls produce the same audio, so synthesis is still deterministic.
///
/// The excitation carries state *within* a call and must not carry it between
/// calls, or the same text would sound different the second time and the
/// engine would not be usable from several threads.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_synthesis_is_deterministic_across_calls() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    let text = "The quick brown fox. It jumped over the lazy dog.";
    assert_eq!(
        engine.synthesize(text).samples,
        engine.synthesize(text).samples
    );

    // And from a second engine, which is what a thread would hold.
    let mut other = flite_rs::Engine::new();
    assert!(other.select_voice("slt"));
    assert_eq!(
        engine.synthesize(text).samples,
        other.synthesize(text).samples
    );
}

/// Streaming hands over exactly the samples the buffered call returns.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_streaming_matches_the_buffered_call() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    let text = "One sentence. Then another one.";

    let buffered = engine.synthesize(text);
    let mut streamed = Vec::new();
    engine.synthesize_streaming(text, |block| {
        streamed.extend_from_slice(block);
        flite_rs::Flow::Continue
    });
    assert_eq!(buffered.samples, streamed);
}

/// Prosody controls reach the statistical backend too.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_responds_to_the_prosody_controls() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    let text = "Testing one two three.";

    let normal = engine.synthesize(text).samples.len();
    engine.set_duration_stretch(1.8);
    let slow = engine.synthesize(text).samples.len();
    assert!(slow > normal, "normal {normal}, slow {slow}");

    engine.set_duration_stretch(1.0);
    engine.set_f0_shift(1.4);
    let higher = engine.synthesize(text);
    // Pitch does not change the length, but it does change the samples.
    assert_eq!(higher.samples.len(), normal);
    assert_ne!(higher.samples.len(), 0);
}

/// Empty and awkward input is as harmless here as on the diphone path.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_survives_awkward_input() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    assert!(engine.synthesize("").samples.is_empty());
    assert!(engine.synthesize("   \n ").samples.is_empty());
    for text in ["pau", ".", "!?", "a", "zzz", "\u{1F600}", "1 2 3"] {
        let _ = engine.synthesize(text);
    }
}

/// A sink can stop a Clustergen run part-way.
#[cfg(feature = "clustergen")]
#[test]
fn a_sink_can_stop_clustergen_early() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"));
    let full = engine
        .synthesize("One. Two. Three. Four. Five.")
        .samples
        .len();

    let mut taken = 0usize;
    engine.synthesize_streaming("One. Two. Three. Four. Five.", |block| {
        taken += block.len();
        if taken > 1000 {
            flite_rs::Flow::Stop
        } else {
            flite_rs::Flow::Continue
        }
    });
    assert!(taken > 0 && taken < full, "took {taken} of {full}");
}
