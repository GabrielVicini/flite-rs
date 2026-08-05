//! Bit-exactness against upstream Flite.
//!
//! The whole point of this engine is that it produces the same samples as the
//! C implementation, not merely similar ones, so this synthesises a corpus with
//! both and compares every sample. A near miss is a failure: it means some
//! model is being fed a feature value the trees were not trained on.
//!
//! The reference binary is not checked in, since building it needs an upstream
//! source tree and a C compiler. Build it with
//!
//! ```text
//! python tools/reference/build.py --flite-src PATH/TO/flite
//! ```
//!
//! When it is absent this test reports that it was skipped and passes, so a
//! plain checkout still runs green. Set `FLITE_REF_DIR` to use binaries built
//! somewhere else. Run under `--release`: the corpus is a few hundred
//! sentences and debug synthesis is slow.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The reference binary, if it has been built.
fn reference_binary() -> Option<PathBuf> {
    let dir = match std::env::var_os("FLITE_REF_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tools")
            .join("reference")
            .join("build"),
    };
    let exe = dir.join(if cfg!(windows) {
        "reffile.exe"
    } else {
        "reffile"
    });
    exe.is_file().then_some(exe)
}

fn corpus() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tools")
        .join("reference")
        .join("corpus.txt");
    let text = std::fs::read_to_string(&path).expect("the corpus is part of the repository");
    text.lines()
        .map(str::trim_end)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The 16-bit mono samples of a RIFF/WAVE file.
///
/// Chunks are walked rather than assumed to start at a fixed offset, because
/// the header upstream writes is not the one this crate writes.
fn wav_samples(bytes: &[u8]) -> Vec<i16> {
    assert!(
        bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE",
        "reference output is not a WAV file"
    );
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let body = at + 8;
        if id == b"data" {
            let end = (body + size).min(bytes.len());
            return bytes[body..end]
                .chunks_exact(2)
                .map(|s| i16::from_le_bytes([s[0], s[1]]))
                .collect();
        }
        // Chunks are padded to an even length.
        at = body + size + (size & 1);
    }
    panic!("no data chunk in the reference output");
}

/// Where two sample sequences first differ, if they do.
///
/// `zero_tail_allowed` lets the reference run longer provided the excess is
/// silence. Upstream's float resynthesiser sizes its output buffer from the
/// last pitch mark and never trims it to the samples it actually generated,
/// which its fixed-point counterpart does do; the difference is trailing zeros.
fn difference(ours: &[i16], theirs: &[i16], zero_tail_allowed: bool) -> Option<String> {
    if let Some(i) = ours.iter().zip(theirs).position(|(a, b)| a != b) {
        return Some(format!(
            "sample {i} of {}: {} but Flite gives {}",
            ours.len().min(theirs.len()),
            ours[i],
            theirs[i]
        ));
    }
    if zero_tail_allowed && theirs.len() > ours.len() {
        let tail = &theirs[ours.len()..];
        return tail.iter().any(|s| *s != 0).then(|| {
            format!(
                "{} samples against Flite's {}, and its extra tail is not silent",
                ours.len(),
                theirs.len()
            )
        });
    }
    if ours.len() != theirs.len() {
        return Some(format!(
            "{} samples but Flite gives {}",
            ours.len(),
            theirs.len()
        ));
    }
    None
}

/// Synthesise every sentence both ways and report the ones that differ.
///
/// `overrides` are passed to the reference as the voice features to set before
/// synthesising, so that `engine` and the reference are asked for the same
/// thing.
fn compare(
    reference: &Path,
    engine: &flite_rs::Engine,
    sentences: &[String],
    overrides: &[&str],
    zero_tail_allowed: bool,
) -> Vec<String> {
    let work = std::env::temp_dir().join(format!(
        "flite-rs-reference-{}-{}",
        std::process::id(),
        overrides.join("-")
    ));
    std::fs::create_dir_all(&work).expect("cannot create a working directory");
    let input = work.join("in.txt");
    let output = work.join("out.wav");
    let mut failures = Vec::new();

    for sentence in sentences {
        // The reference reads a file, so it sees the trailing newline too.
        let text = format!("{sentence}\n");
        std::fs::write(&input, &text).expect("cannot write the input file");
        let _ = std::fs::remove_file(&output);

        let status = Command::new(reference)
            .arg(&input)
            .arg(&output)
            .args(overrides)
            .status()
            .unwrap_or_else(|e| panic!("cannot run {}: {e}", reference.display()));
        assert!(status.success(), "reference failed on {sentence:?}");

        let theirs = wav_samples(&std::fs::read(&output).expect("reference wrote no output"));
        let ours = engine.synthesize(&text).samples;

        if let Some(difference) = difference(&ours, &theirs, zero_tail_allowed) {
            failures.push(format!("{sentence:?}\n    {difference}"));
        }
    }

    let _ = std::fs::remove_dir_all(&work);
    failures
}

#[test]
fn output_is_identical_to_upstream_flite() {
    let Some(reference) = reference_binary() else {
        println!(
            "skipped: no reference binary. Build one with \
             `python tools/reference/build.py --flite-src PATH/TO/flite`."
        );
        return;
    };

    let sentences = corpus();
    let failures = compare(&reference, &flite_rs::Engine::new(), &sentences, &[], false);

    assert!(
        failures.is_empty(),
        "{} of {} sentences diverged from upstream Flite:\n  {}",
        failures.len(),
        sentences.len(),
        failures.join("\n  ")
    );
    println!("{} sentences, all bit-identical", sentences.len());
}

/// The join and resynthesis paths the bundled voice does not ask for.
///
/// These are reachable only through [`flite_rs::VoiceParams`], so without this
/// they would be code that compiles and is never compared with anything.
#[test]
fn alternative_join_and_resynth_paths_match_upstream() {
    use flite_rs::{JoinType, ResynthType};

    let Some(reference) = reference_binary() else {
        println!("skipped: no reference binary");
        return;
    };

    // A slice of the corpus rather than all of it: these paths share unit
    // selection and the filter with the default one, so what is under test is
    // the join and the arithmetic, not the analysis.
    let sentences: Vec<String> = corpus().into_iter().take(25).collect();

    // Upstream's simple join implements only the fixed-point filter, so the
    // fourth combination has nothing to be compared against. Here the two
    // settings are independent and it works.
    for (join, resynth, names) in [
        (
            JoinType::Simple,
            ResynthType::Fixed,
            ["simple_join", "fixed"],
        ),
        (
            JoinType::ModifiedLpc,
            ResynthType::Float,
            ["modified_lpc", "float"],
        ),
    ] {
        let mut engine = flite_rs::Engine::new();
        engine.params_mut().join_type = join;
        engine.params_mut().resynth_type = resynth;

        // Only the float filter leaves the untrimmed tail.
        let untrimmed = resynth == ResynthType::Float;
        let failures = compare(&reference, &engine, &sentences, &names, untrimmed);
        assert!(
            failures.is_empty(),
            "{} of {} sentences diverged with {names:?}:\n  {}",
            failures.len(),
            sentences.len(),
            failures.join("\n  ")
        );
    }
    println!(
        "{} sentences on each alternative path, all bit-identical",
        sentences.len()
    );
}
