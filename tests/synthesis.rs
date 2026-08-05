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
fn synthesis_is_deterministic() {
    let e = engine();
    let text = "Determinism matters for testing.";
    assert_eq!(e.synthesize(text).samples, e.synthesize(text).samples);
}
