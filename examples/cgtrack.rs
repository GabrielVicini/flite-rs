//! Dump a Clustergen voice's parameter trajectories, for comparing against
//! upstream.
//!
//! The output format is exactly what `tools/reference/reftrack` prints, so a
//! divergence in Clustergen is bisected with a plain diff:
//!
//! ```text
//! tools/reference/build/reftrack "The quick brown fox." > ref.txt
//! cargo run --release --features clustergen --example cgtrack -- \
//!     "The quick brown fox." > ours.txt
//! diff ref.txt ours.txt
//! ```
//!
//! Whichever section differs first names the stage that broke: the segment end
//! times are the state duration trees, `PARAM` is the F0 and parameter trees
//! and the model store, `STR` is the band strengths, and `MLPG` is the
//! smoothing. If every section agrees and the audio still differs, the
//! difference is in the vocoder.
//!
//! Floats print with the shortest representation that round-trips, as the C
//! side's `%.9g` does, so a last-bit difference still shows up as text.

#[cfg(not(feature = "clustergen"))]
fn main() {
    eprintln!("cgtrack needs the `clustergen` feature");
    std::process::exit(1);
}

#[cfg(feature = "clustergen")]
fn main() {
    use flite_rs::clustergen::ClustergenVoice;

    let text: Vec<String> = std::env::args().skip(1).collect();
    if text.is_empty() {
        eprintln!("usage: cgtrack \"one sentence\"");
        std::process::exit(1);
    }
    // -f dumps every feature the duration tree consults, for every HMM state,
    // which is the last-resort bisection the reference README prescribes:
    // diff it against `reftrack -f` and the first differing column names the
    // feature that is wrong.
    let features = text.iter().any(|a| a == "-f");
    let text = text
        .iter()
        .filter(|a| !a.starts_with('-'))
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");

    // The voice is parsed directly rather than reached through the engine,
    // because the trajectories are what is wanted and the engine's job is to
    // hide them.
    let voice = ClustergenVoice::parse(flite_rs::SLT_VOICE_DATA).expect("bundled data is valid");

    // Analysed by the engine with the Clustergen voice selected, so that the
    // pipeline sees that voice's parameters: analysing with the wrong voice
    // produces a difference that is entirely self-inflicted.
    let mut engine = flite_rs::Engine::new();
    assert!(
        engine.select_voice("slt"),
        "the clustergen voice is registered"
    );
    let params = *engine.params();
    let mut utt = engine.analyse(&text);
    let predicted = voice.trajectories(&mut utt, &params);

    if features {
        let paths = voice.duration_features();
        let states: Vec<_> = utt.iter_relation("HMMstate").collect();
        println!("FEATURES {} {}", states.len(), paths.len());
        for path in &paths {
            println!("{path}");
        }
        for state in states {
            let mut row = utt.name(state).to_string();
            for path in &paths {
                row.push('\t');
                row.push_str(&show(&flite_rs::ffeature::eval_str(&utt, state, path)));
            }
            println!("{row}");
        }
        return;
    }

    println!("FRAMES {}", predicted.track.len());

    let segments: Vec<_> = utt.iter_relation("Segment").collect();
    println!("SEGMENTS {}", segments.len());
    for segment in segments {
        println!(
            "{} {}",
            utt.name(segment),
            g9(utt.feature_f32(segment, "end"))
        );
    }

    let states: Vec<_> = utt.iter_relation("HMMstate").collect();
    println!("STATES {}", states.len());
    for state in states {
        println!(
            "{} {} {}",
            utt.name(state),
            utt.feature_i32(state, "statepos"),
            g9(utt.feature_f32(state, "end"))
        );
    }

    print_track("PARAM", &predicted.track);
    print_track("STR", &predicted.strengths);
    print_track("MLPG", &predicted.smoothed);
}

#[cfg(feature = "clustergen")]
fn print_track(name: &str, track: &[Vec<f32>]) {
    let channels = track.first().map_or(0, Vec::len);
    println!("{name} {} {channels}", track.len());
    for frame in track {
        let row: Vec<String> = frame.iter().map(|v| g9(*v)).collect();
        println!("{}", row.join(" "));
    }
}

/// A float as C's `%.9g` prints it.
///
/// Nine significant digits round-trips a single-precision value, so this is
/// lossless; the point is to match the reference's text exactly rather than to
/// be readable.
#[cfg(feature = "clustergen")]
fn g9(v: f32) -> String {
    if v == 0.0 {
        // C prints negative zero as "-0"; Rust's formatter agrees, but the
        // common case is worth short-circuiting.
        return if v.is_sign_negative() {
            "-0".to_string()
        } else {
            "0".to_string()
        };
    }
    if !v.is_finite() {
        return if v.is_nan() {
            "nan".to_string()
        } else if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }

    let exponent = v.abs().log10().floor() as i32;
    // %g uses %e outside [1e-4, 1e9) for nine significant digits.
    if !(-4..9).contains(&exponent) {
        let s = format!("{:.8e}", v);
        // Rust writes `1.23456789e5`; C writes `1.23456789e+05`.
        let (mantissa, exp) = s.split_once('e').expect("formatted as exponential");
        let exp: i32 = exp.parse().expect("exponent is a number");
        let mantissa = trim_zeros(mantissa);
        return format!(
            "{mantissa}e{}{:02}",
            if exp < 0 { '-' } else { '+' },
            exp.abs()
        );
    }
    let decimals = (8 - exponent).max(0) as usize;
    trim_zeros(&format!("{:.*}", decimals, v))
}

#[cfg(feature = "clustergen")]
fn trim_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A feature value as `reftrack -f` prints it.
#[cfg(feature = "clustergen")]
fn show(value: &flite_rs::value::Value) -> String {
    use flite_rs::value::Value;
    match value {
        Value::Float(v) => g9(*v),
        Value::Int(v) => v.to_string(),
        _ => value.as_str().to_string(),
    }
}
