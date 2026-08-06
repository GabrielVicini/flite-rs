//! Print the linguistic analysis of a sentence.
//!
//! Handy when a word comes out mispronounced or a phrase sounds oddly timed:
//! it shows the phones with their end times, the syllable-level intonation
//! decisions, and the pitch contour, without generating any audio.
//!
//! ```text
//! cargo run --example analysis -- "Hello, and welcome."
//! ```

use flite_rs::Engine;

fn main() {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    // `-p` analyses the argument as phones instead, which skips the lexicon
    // and the intonation model. `voice=NAME` picks a voice, since the
    // postlexical rules and the pitch range belong to it.
    let phones = arguments.first().is_some_and(|a| a == "-p");
    if phones {
        arguments.remove(0);
    }
    let voice = arguments
        .first()
        .and_then(|a| a.strip_prefix("voice=").map(str::to_string));
    if voice.is_some() {
        arguments.remove(0);
    }

    let text = arguments.join(" ");
    let text = if text.is_empty() {
        "Hello, and welcome to the world of speech synthesis.".to_string()
    } else {
        text
    };

    let mut engine = Engine::new();
    if let Some(name) = &voice {
        assert!(engine.select_voice(name), "no voice named {name}");
    }
    let utt = if phones {
        engine.analyse_phones(&text)
    } else {
        engine.analyse(&text)
    };

    println!("SEGMENTS");
    for seg in utt.iter_relation("Segment") {
        println!("{} {:.6}", utt.name(seg), utt.feature_f32(seg, "end"));
    }

    println!("SYLLABLES");
    for syl in utt.iter_relation("Syllable") {
        println!(
            "{} stress={} accent={} endtone={}",
            flite_rs::ffeature::eval_str(&utt, syl, "R:SylStructure.parent.name"),
            or_default(utt.feature_str(syl, "stress")),
            or_default(utt.feature_str(syl, "accent")),
            or_default(utt.feature_str(syl, "endtone")),
        );
    }

    println!("TARGETS");
    for target in utt.iter_relation("Target") {
        println!(
            "{:.6} {:.4}",
            utt.feature_f32(target, "pos"),
            utt.feature_f32(target, "f0")
        );
    }
}

/// Absent features read as `"0"` throughout the engine; show that explicitly
/// so the output lines up with what the models actually see.
fn or_default(value: &str) -> &str {
    if value.is_empty() {
        "0"
    } else {
        value
    }
}
