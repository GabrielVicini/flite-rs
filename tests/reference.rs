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

/// The whole corpus as one file, read in chunks and synthesised as it goes.
///
/// The per-sentence test above never exercises a chunk boundary or a sentence
/// break found across one, and those are exactly where a streaming reader goes
/// wrong. Upstream reads the same file the same way, so this compares the two
/// token streams as much as the audio.
#[test]
fn reading_a_file_in_chunks_matches_upstream() {
    compare_whole_corpus_file(&flite_rs::Engine::new(), &[], "diphone");
}

/// Feed the whole corpus through as one file and compare the result.
///
/// Shared between the backends because this is the only test that exercises a
/// chunk boundary inside a token, a sentence break across one, and — for a
/// backend that carries state between sentences — several hundred sentences of
/// one continuous run.
fn compare_whole_corpus_file(engine: &flite_rs::Engine, overrides: &[&str], label: &str) {
    let Some(reference) = reference_binary() else {
        println!("skipped: no reference binary");
        return;
    };

    let work = std::env::temp_dir().join(format!(
        "flite-rs-whole-file-{}-{label}",
        std::process::id()
    ));
    std::fs::create_dir_all(&work).expect("cannot create a working directory");
    let input = work.join("corpus.txt");
    let output = work.join("out.wav");
    std::fs::write(&input, corpus().join("\n") + "\n").expect("cannot write the input file");

    let status = Command::new(&reference)
        .arg(&input)
        .arg(&output)
        .args(overrides)
        .status()
        .expect("cannot run the reference");
    assert!(status.success(), "the reference failed on the whole corpus");
    let theirs = wav_samples(&std::fs::read(&output).expect("reference wrote no output"));

    let mut ours = Vec::new();
    engine
        .synthesize_reader(
            std::fs::File::open(&input).expect("the file was just written"),
            |period| {
                ours.extend_from_slice(period);
                flite_rs::Flow::Continue
            },
        )
        .expect("reading a local file cannot fail");

    let _ = std::fs::remove_dir_all(&work);
    assert_eq!(
        difference(&ours, &theirs, false),
        None,
        "{} samples of continuous {label} speech",
        theirs.len()
    );
}

/// The 16 kHz voice, which shares the language models and nothing else: its
/// residual is raw mu-law rather than ADPCM, it has no decoded-size table, its
/// LPC order is 16 rather than 10, and its postlexical rules leave `ah` alone.
#[cfg(feature = "kal16")]
#[test]
fn the_16_khz_voice_matches_upstream() {
    let Some(reference) = reference_binary() else {
        println!("skipped: no reference binary");
        return;
    };

    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("kal16"), "kal16 is not registered");
    assert_eq!(engine.sample_rate(), 16000);

    let sentences = corpus();
    let failures = compare(&reference, &engine, &sentences, &["voice=kal16"], false);
    assert!(
        failures.is_empty(),
        "{} of {} sentences diverged on kal16:\n  {}",
        failures.len(),
        sentences.len(),
        failures.join("\n  ")
    );
    println!("{} sentences at 16 kHz, all bit-identical", sentences.len());
}

/// Synthesis from a phone string, which skips the lexicon and text analysis.
#[test]
fn phone_strings_match_upstream() {
    let Some(reference) = reference_binary().map(|p| {
        p.with_file_name(if cfg!(windows) {
            "refphones.exe"
        } else {
            "refphones"
        })
    }) else {
        println!("skipped: no reference binary");
        return;
    };
    assert!(reference.is_file(), "rebuild the reference for refphones");

    let work = std::env::temp_dir().join(format!("flite-rs-phones-{}", std::process::id()));
    std::fs::create_dir_all(&work).expect("cannot create a working directory");
    let output = work.join("out.wav");
    let engine = flite_rs::Engine::new();

    // A syllable break, `-`, is deliberately absent: it aborts upstream, so
    // there is nothing to compare against. See `pipeline::analyse_phones`.
    for phones in [
        "pau hh ax l ow pau",
        "pau w er l d pau",
        "hh ax l ow",
        "pau b uh k k iy p er pau",
        "pau s ih n th ax s ax s pau",
        "pau ay1 pau",
        "pau k w eh1 s ch ax0 n pau",
        "pau zh aa r jh oy oy pau",
        // One segment, so no diphone spans it and there is no audio at all.
        "pau",
    ] {
        let _ = std::fs::remove_file(&output);
        let status = Command::new(&reference)
            .arg(phones)
            .arg(&output)
            .status()
            .expect("cannot run refphones");
        assert!(status.success(), "the reference failed on {phones:?}");

        let theirs = wav_samples(&std::fs::read(&output).expect("reference wrote no output"));
        let ours = engine.synthesize_phones(phones).samples;
        assert_eq!(difference(&ours, &theirs, false), None, "{phones:?}");
    }

    let _ = std::fs::remove_dir_all(&work);
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
            ["join_type=simple_join", "resynth_type=fixed"],
        ),
        (
            JoinType::ModifiedLpc,
            ResynthType::Float,
            ["join_type=modified_lpc", "resynth_type=float"],
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

// ---------------------------------------------------------------------------
// Clustergen.
//
// The diphone path never reaches the C library's rand(); Clustergen's
// excitation does, on every sample. tools/reference/refrand.c explains what is
// done about that and why the comparison below is still a comparison against
// upstream rather than against a modified copy of it.
// ---------------------------------------------------------------------------

/// A reference binary other than `reffile`, if the reference has been built.
#[cfg(feature = "clustergen")]
fn reference_tool(name: &str) -> Option<PathBuf> {
    let exe = reference_binary()?.with_file_name(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    exe.is_file().then_some(exe)
}

/// The Clustergen voice, over the whole corpus, sample for sample.
#[cfg(feature = "clustergen")]
#[test]
fn the_clustergen_voice_matches_upstream() {
    let Some(reference) = reference_binary() else {
        println!("skipped: no reference binary");
        return;
    };

    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"), "slt is not registered");
    assert_eq!(engine.sample_rate(), 16000);

    let sentences = corpus();
    let failures = compare(&reference, &engine, &sentences, &["voice=slt"], false);
    assert!(
        failures.is_empty(),
        "{} of {} sentences diverged on the clustergen voice:\n  {}",
        failures.len(),
        sentences.len(),
        failures.join("\n  ")
    );
    println!(
        "{} sentences through clustergen, all bit-identical",
        sentences.len()
    );
}

/// One section of `reftrack` output: a name, a shape and the numbers.
#[cfg(feature = "clustergen")]
struct RefTrack {
    name: String,
    rows: usize,
    columns: usize,
    values: Vec<f32>,
}

/// Parse `reftrack`'s dump into its sections.
///
/// Values are read back as `f32`. The C side prints `%.9g`, which round-trips
/// a single-precision value exactly, so this recovers the original bits and
/// the comparison below is a bit comparison and not a tolerance.
#[cfg(feature = "clustergen")]
type RefSegments = Vec<(String, f32)>;
/// Name, state position within the phone, and end time.
#[cfg(feature = "clustergen")]
type RefStates = Vec<(String, i32, f32)>;

#[cfg(feature = "clustergen")]
fn parse_reftrack(text: &str) -> (RefSegments, RefStates, Vec<RefTrack>) {
    let mut segments = Vec::new();
    let mut states = Vec::new();
    let mut tracks = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let mut parts = line.split_whitespace();
        let Some(head) = parts.next() else { continue };
        let numbers: Vec<&str> = parts.collect();
        match head {
            "FRAMES" => {}
            "SEGMENTS" => {
                let n: usize = numbers[0].parse().expect("a segment count");
                for _ in 0..n {
                    let row = lines.next().expect("a segment line");
                    let (name, end) = row.rsplit_once(' ').expect("name and end time");
                    segments.push((name.to_string(), end.parse().expect("an end time")));
                }
            }
            "STATES" => {
                let n: usize = numbers[0].parse().expect("a state count");
                for _ in 0..n {
                    let row: Vec<&str> = lines.next().expect("a state line").split(' ').collect();
                    states.push((
                        row[0].to_string(),
                        row[1].parse().expect("a state position"),
                        row[2].parse().expect("an end time"),
                    ));
                }
            }
            "PARAM" | "STR" | "MLPG" => {
                let rows: usize = numbers[0].parse().expect("a row count");
                let columns: usize = numbers[1].parse().expect("a column count");
                let mut values = Vec::with_capacity(rows * columns);
                for _ in 0..rows {
                    let row = lines.next().expect("a track row");
                    for token in row.split_whitespace() {
                        values.push(token.parse().expect("a track value"));
                    }
                }
                tracks.push(RefTrack {
                    name: head.to_string(),
                    rows,
                    columns,
                    values,
                });
            }
            _ => {}
        }
    }
    (segments, states, tracks)
}

/// Compare one track against the reference's, reporting where they part.
#[cfg(feature = "clustergen")]
fn track_difference(reference: &RefTrack, ours: &[Vec<f32>]) -> Option<String> {
    if ours.len() != reference.rows {
        return Some(format!(
            "{}: {} frames against upstream's {}",
            reference.name,
            ours.len(),
            reference.rows
        ));
    }
    let columns = ours.first().map_or(0, Vec::len);
    if reference.rows > 0 && columns != reference.columns {
        return Some(format!(
            "{}: {columns} channels against upstream's {}",
            reference.name, reference.columns
        ));
    }
    for (row, frame) in ours.iter().enumerate() {
        for (column, value) in frame.iter().enumerate() {
            let theirs = reference.values[row * columns + column];
            // Bit comparison, not approximate: these stages are deterministic
            // on both sides and a last-bit difference is a real finding.
            if value.to_bits() != theirs.to_bits() {
                return Some(format!(
                    "{}: frame {row} channel {column} is {value:e} but upstream gives {theirs:e}",
                    reference.name
                ));
            }
        }
    }
    None
}

/// Every Clustergen stage short of the vocoder, compared directly.
///
/// This is the sharper of the two Clustergen tests. The waveform comparison
/// above says whether the output agrees; this says *which stage* disagrees
/// when it does not, and it covers the MLPG smoothing, which the waveform only
/// reaches through the vocoder. It also keeps the state duration trees and the
/// F0 splining honest independently of whether the audio happens to line up.
#[cfg(feature = "clustergen")]
#[test]
fn clustergen_trajectories_match_upstream() {
    use flite_rs::clustergen::ClustergenVoice;

    let Some(reference) = reference_tool("reftrack") else {
        println!("skipped: no reftrack binary. Rebuild the reference.");
        return;
    };

    let voice = ClustergenVoice::parse(flite_rs::SLT_VOICE_DATA).expect("bundled data is valid");
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"), "slt is not registered");
    let params = *engine.params();

    // reftrack synthesises its argument as one utterance, so this runs on
    // single sentences. A slice of the corpus rather than all of it: the
    // waveform test above already covers every line, and what is under test
    // here is the stages, which these exercise.
    let sentences: Vec<String> = corpus()
        .into_iter()
        .filter(|s| {
            !s.contains('.')
                || s.trim_end()
                    .strip_suffix('.')
                    .is_some_and(|r| !r.contains('.'))
        })
        .take(40)
        .collect();
    assert!(!sentences.is_empty(), "the corpus has single sentences");

    let mut failures = Vec::new();
    for sentence in &sentences {
        let output = Command::new(&reference)
            .arg(sentence)
            .output()
            .unwrap_or_else(|e| panic!("cannot run {}: {e}", reference.display()));
        assert!(output.status.success(), "reftrack failed on {sentence:?}");
        let text = String::from_utf8_lossy(&output.stdout);
        let (segments, states, tracks) = parse_reftrack(&text);

        let mut utt = engine.analyse(sentence);
        let predicted = voice.trajectories(&mut utt, &params);

        let mut problem = None;

        // Segment end times first: everything downstream is laid out from
        // them, so a difference here explains any difference after it.
        let ours: Vec<(String, f32)> = utt
            .iter_relation("Segment")
            .map(|s| (utt.name(s).to_string(), utt.feature_f32(s, "end")))
            .collect();
        if ours.len() != segments.len() {
            problem = Some(format!(
                "{} segments against upstream's {}",
                ours.len(),
                segments.len()
            ));
        } else {
            for (i, (theirs, ours)) in segments.iter().zip(&ours).enumerate() {
                if theirs.0 != ours.0 || theirs.1.to_bits() != ours.1.to_bits() {
                    problem = Some(format!(
                        "segment {i} is {} at {} but upstream has {} at {}",
                        ours.0, ours.1, theirs.0, theirs.1
                    ));
                    break;
                }
            }
        }

        // Then the HMM states, which is the duration model proper.
        if problem.is_none() {
            let ours: Vec<(String, i32, f32)> = utt
                .iter_relation("HMMstate")
                .map(|s| {
                    (
                        utt.name(s).to_string(),
                        utt.feature_i32(s, "statepos"),
                        utt.feature_f32(s, "end"),
                    )
                })
                .collect();
            if ours.len() != states.len() {
                problem = Some(format!(
                    "{} states against upstream's {}",
                    ours.len(),
                    states.len()
                ));
            } else {
                for (i, (theirs, ours)) in states.iter().zip(&ours).enumerate() {
                    if theirs.0 != ours.0
                        || theirs.1 != ours.1
                        || theirs.2.to_bits() != ours.2.to_bits()
                    {
                        problem = Some(format!(
                            "state {i} is {} ({}) at {} but upstream has {} ({}) at {}",
                            ours.0, ours.1, ours.2, theirs.0, theirs.1, theirs.2
                        ));
                        break;
                    }
                }
            }
        }

        // Then the tracks, in the order the stages run.
        if problem.is_none() {
            for track in &tracks {
                let ours = match track.name.as_str() {
                    "PARAM" => &predicted.track,
                    "STR" => &predicted.strengths,
                    "MLPG" => &predicted.smoothed,
                    _ => continue,
                };
                if let Some(difference) = track_difference(track, ours) {
                    problem = Some(difference);
                    break;
                }
            }
        }

        if let Some(problem) = problem {
            failures.push(format!("{sentence:?}\n    {problem}"));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} sentences diverged in the clustergen trajectories:\n  {}",
        failures.len(),
        sentences.len(),
        failures.join("\n  ")
    );
    println!(
        "{} sentences of clustergen trajectories, all bit-identical",
        sentences.len()
    );
}

/// The whole corpus as one Clustergen run.
///
/// This is the test that pins the excitation's continuity across sentences.
/// Upstream's pseudo-random source is process-global, so the hundredth
/// sentence of one run does not sound like the hundredth sentence spoken
/// alone; synthesising the corpus one sentence per process, as the test above
/// does, cannot see that. Here a single run covers every sentence, so a
/// backend that restarted its excitation per utterance would diverge in the
/// second sentence and stay wrong for the rest of the corpus.
#[cfg(feature = "clustergen")]
#[test]
fn reading_a_file_in_chunks_matches_upstream_on_clustergen() {
    let mut engine = flite_rs::Engine::new();
    assert!(engine.select_voice("slt"), "slt is not registered");
    compare_whole_corpus_file(&engine, &["voice=slt"], "clustergen");
}
