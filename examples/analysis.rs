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
    let text: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let text = if text.is_empty() {
        "Hello, and welcome to the world of speech synthesis.".to_string()
    } else {
        text
    };

    let engine = Engine::new();
    let utt = engine.analyse(&text);

    println!("SEGMENTS");
    for seg in utt.iter_relation("Segment") {
        println!("{} {:.6}", utt.name(seg), utt.feature_f32(seg, "end"));
    }

    println!("SYLLABLES");
    for syl in utt.iter_relation("Syllable") {
        println!(
            "stress={} accent={} endtone={}",
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
