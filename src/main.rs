//! Command-line front-end for `flite-rs`.
//!
//! Reads text from an argument, a file, or standard input and writes a WAV
//! file (or raw samples to stdout).
//!
//! The options follow upstream `flite`, including its single-dash multi-letter
//! spellings such as `-pw` and `-lv`, which are rewritten to `--pw` and `--lv`
//! before parsing because the argument parser has no notion of them. Both
//! spellings work.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use flite_rs::{
    ffeature, pipeline, text, Engine, Flow, JoinType, ResynthType, Utterance, WavWriter,
};

/// Upstream's benchmark sentence, kept so that a number from one engine can be
/// compared with a number from the other.
const BENCHMARK_TEXT: &str =
    "A whole joy was reaping, but they've gone south, you should fetch azure mike.";

/// Iterations `-b` runs, as upstream's `ITER_MAX` plus the first pass.
const BENCHMARK_ITERATIONS: usize = 4;

/// Options upstream spells with one dash and more than one letter. Clap has no
/// way to declare these, so they are rewritten before it sees them.
const SINGLE_DASH_LONG: &[&str] = &[
    "-pw",
    "-ps",
    "-psdur",
    "-psstress",
    "-pr",
    "-lv",
    "-set",
    "-voice",
    "-add_lex",
];

#[derive(Parser)]
#[command(
    name = "flite-rs",
    version,
    about = "Small, fast, portable text to speech",
    after_help = "\
EXAMPLES:
  flite-rs \"Hello there.\" hello.wav
  flite-rs -f speech.txt -o speech.wav
  echo \"Piped text.\" | flite-rs -o out.wav
  flite-rs -p \"pau hh ax l ow pau\" -o hello.wav
  flite-rs -t Bookkeeper -ps
  flite-rs --setf f0_shift=1.2 -t \"Higher.\" -o high.wav

A first positional argument containing a space is spoken as text; otherwise it
names a file to read. A second names the WAV file to write."
)]
struct Args {
    /// Text to speak if it contains a space, otherwise a file to read. Reads
    /// standard input when absent.
    text_or_file: Option<String>,

    /// WAV file to write.
    wavefile: Option<PathBuf>,

    /// Read the text from a file.
    #[arg(short = 'f', long, value_name = "FILE")]
    file: Option<PathBuf>,

    /// Speak this text.
    #[arg(short = 't', long, value_name = "TEXT")]
    text: Option<String>,

    /// Speak a phone string directly, as in "pau hh ax l ow pau".
    #[arg(short = 'p', long = "phone-string", value_name = "PHONES")]
    phones_input: Option<String>,

    /// Output WAV file; "-" is standard output and "none" discards the audio.
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Speak with this voice.
    #[arg(long, value_name = "NAME")]
    voice: Option<String>,

    /// Read extra pronunciations from a file, as "word [pos] : phones".
    #[arg(long = "add_lex", value_name = "FILE")]
    add_lex: Option<PathBuf>,

    /// List the available voices and exit.
    #[arg(long = "lv")]
    list_voices: bool,

    /// Set a voice parameter, guessing whether it is a number or a name.
    #[arg(short = 's', long = "set", value_name = "F=V")]
    set: Vec<String>,

    /// Set a numeric voice parameter.
    #[arg(long = "setf", value_name = "F=V")]
    setf: Vec<String>,

    /// Set a numeric voice parameter. An alias of --setf, since no parameter
    /// this engine has is an integer.
    #[arg(long = "seti", value_name = "F=V")]
    seti: Vec<String>,

    /// Set a named voice parameter, such as join_type.
    #[arg(long = "sets", value_name = "F=V")]
    sets: Vec<String>,

    /// Print the words.
    #[arg(long = "pw")]
    print_words: bool,

    /// Print the phones.
    #[arg(long = "ps", alias = "phones")]
    print_segments: bool,

    /// Print the phones with their end times.
    #[arg(long = "psdur")]
    print_segment_durations: bool,

    /// Print the phones with the stress of the syllable each is in.
    #[arg(long = "psstress")]
    print_segment_stress: bool,

    /// Print any relation of the utterance by name.
    #[arg(long = "pr", value_name = "RELATION")]
    print_relation: Option<String>,

    /// Synthesise the benchmark sentence repeatedly and report the speed.
    #[arg(short = 'b', long)]
    benchmark: bool,

    /// Keep synthesising the same input until interrupted.
    #[arg(short = 'l', long = "loop")]
    repeat: bool,

    /// Speech rate multiplier; above 1.0 is slower.
    #[arg(long, value_name = "FACTOR", default_value_t = 1.0)]
    speed: f32,

    /// Pitch multiplier; above 1.0 is higher.
    #[arg(long, value_name = "FACTOR", default_value_t = 1.0)]
    pitch: f32,

    /// Report how much faster than real time the synthesis ran.
    #[arg(short = 'v', long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let argv = std::env::args_os().map(|arg| match arg.to_str() {
        Some(text) if SINGLE_DASH_LONG.contains(&text) => format!("-{text}").into(),
        _ => arg,
    });
    let args = Args::parse_from(argv);
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flite-rs: {e}");
            ExitCode::FAILURE
        }
    }
}

/// What is to be spoken, and how it should be read.
enum Input {
    Text(String),
    Phones(String),
    /// Read as it is synthesised, so its size does not matter.
    File(PathBuf),
}

fn run(args: Args) -> io::Result<()> {
    let mut engine = Engine::new();

    if args.list_voices {
        println!("{}", engine.voice_names().collect::<Vec<_>>().join(" "));
        return Ok(());
    }
    if let Some(name) = &args.voice {
        if !engine.select_voice(name) {
            return Err(unusable(format!(
                "no voice named {name:?}; available: {}",
                engine.voice_names().collect::<Vec<_>>().join(" ")
            )));
        }
    }

    if let Some(path) = &args.add_lex {
        let entries = std::fs::read_to_string(path)?;
        let added = engine
            .add_lex_entries(&entries)
            .map_err(|e| unusable(format!("{}: {e}", path.display())))?;
        if args.verbose {
            eprintln!("added {added} pronunciations");
        }
    }

    engine.set_duration_stretch(args.speed);
    engine.set_f0_shift(args.pitch);
    for assignment in args.set.iter().chain(&args.setf).chain(&args.seti) {
        set_parameter(&mut engine, assignment)?;
    }
    for assignment in &args.sets {
        set_parameter(&mut engine, assignment)?;
    }

    if args.benchmark {
        return benchmark(&engine);
    }

    let input = choose_input(&args)?;
    if printing(&args) {
        return print_relations(&engine, &args, &input);
    }

    loop {
        speak(&engine, &args, &input)?;
        if !args.repeat {
            return Ok(());
        }
    }
}

/// Which of the ways of naming input was used.
///
/// A bare argument is text when it contains a space and a filename otherwise,
/// which is upstream's rule and the reason `-f` and `-t` exist to say so
/// explicitly.
fn choose_input(args: &Args) -> io::Result<Input> {
    if let Some(phones) = &args.phones_input {
        check_not_empty(phones)?;
        return Ok(Input::Phones(phones.clone()));
    }
    if let Some(path) = &args.file {
        return Ok(Input::File(path.clone()));
    }
    if let Some(text) = &args.text {
        check_not_empty(text)?;
        return Ok(Input::Text(text.clone()));
    }
    if let Some(argument) = &args.text_or_file {
        if argument.contains(' ') {
            return Ok(Input::Text(argument.clone()));
        }
        return Ok(Input::File(PathBuf::from(argument)));
    }
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    check_not_empty(&text)?;
    Ok(Input::Text(text))
}

/// Input given on the command line is checked before synthesis; a file is not,
/// because knowing would mean reading all of it first.
fn check_not_empty(text: &str) -> io::Result<()> {
    if text.trim().is_empty() {
        return Err(unusable("no text to speak".to_string()));
    }
    Ok(())
}

fn output_path(args: &Args) -> PathBuf {
    args.output
        .clone()
        .or_else(|| args.wavefile.clone())
        .unwrap_or_else(|| PathBuf::from("out.wav"))
}

fn speak(engine: &Engine, args: &Args, input: &Input) -> io::Result<()> {
    let output = output_path(args);
    let started = Instant::now();

    // "none" discards the audio, which is how upstream measures synthesis
    // without measuring the disk.
    let samples = if output.as_os_str() == "none" {
        let mut count = 0usize;
        stream(engine, input, |period| {
            count += period.len();
            Flow::Continue
        })?;
        count
    } else if output.as_os_str() == "-" {
        // A pipe cannot be seeked back to, so the header can only be written
        // once the length is known, which means holding the audio.
        let mut audio = flite_rs::Audio {
            samples: Vec::new(),
            sample_rate: engine.sample_rate(),
        };
        stream(engine, input, |period| {
            audio.samples.extend_from_slice(period);
            Flow::Continue
        })?;
        let stdout = io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        flite_rs::write_wav(&audio, &mut out)?;
        out.flush()?;
        audio.samples.len()
    } else {
        let mut writer =
            WavWriter::new(BufWriter::new(File::create(&output)?), engine.sample_rate())?;
        let mut count = 0usize;
        let mut failed = Ok(());
        stream(engine, input, |period| match writer.write(period) {
            Ok(()) => {
                count += period.len();
                Flow::Continue
            }
            Err(e) => {
                failed = Err(e);
                Flow::Stop
            }
        })?;
        failed?;
        writer.finish()?;
        if count == 0 && matches!(input, Input::File(_)) {
            // An empty file is only discovered once it has all been read, but
            // the answer should still be the error it would have been, and not
            // a WAV file with nothing in it.
            let _ = std::fs::remove_file(&output);
            return Err(unusable("no text to speak".to_string()));
        }
        count
    };

    if args.verbose {
        report(samples as f32 / engine.sample_rate() as f32, started);
    }
    Ok(())
}

/// Synthesise whatever the input is, a pitch period at a time.
fn stream<F>(engine: &Engine, input: &Input, sink: F) -> io::Result<()>
where
    F: FnMut(&[i16]) -> Flow,
{
    match input {
        Input::Text(text) => {
            engine.synthesize_streaming(text, sink);
            Ok(())
        }
        Input::Phones(phones) => {
            engine.synthesize_phones_streaming(phones, sink);
            Ok(())
        }
        Input::File(path) => engine.synthesize_reader(File::open(path)?, sink),
    }
}

fn report(seconds: f32, started: Instant) {
    let elapsed = started.elapsed().as_secs_f32();
    println!(
        "times faster than real-time: {}\n({} seconds of speech synthesized in {})",
        seconds / elapsed,
        seconds,
        elapsed
    );
}

fn benchmark(engine: &Engine) -> io::Result<()> {
    let started = Instant::now();
    let mut samples = 0usize;
    for _ in 0..BENCHMARK_ITERATIONS {
        samples = 0;
        engine.synthesize_streaming(BENCHMARK_TEXT, |period| {
            samples += period.len();
            Flow::Continue
        });
    }
    let seconds = samples as f32 / engine.sample_rate() as f32;
    report(seconds, started);
    Ok(())
}

fn printing(args: &Args) -> bool {
    args.print_words
        || args.print_segments
        || args.print_segment_durations
        || args.print_segment_stress
        || args.print_relation.is_some()
}

/// Print the requested relation of every utterance, without generating audio.
fn print_relations(engine: &Engine, args: &Args, input: &Input) -> io::Result<()> {
    let text = match input {
        Input::Text(text) | Input::Phones(text) => text.clone(),
        Input::File(path) => std::fs::read_to_string(path)?,
    };
    let language = engine.language();
    let params = *engine.params();

    let utterances: Vec<Utterance> = match input {
        Input::Phones(_) => {
            let tokens: Vec<text::Token> = text::tokenize(&text).into_iter().flatten().collect();
            vec![pipeline::analyse_phones(&language, &tokens, &params)]
        }
        _ => text::sentences(&text)
            .map(|sentence| pipeline::analyse(&language, &sentence, &params))
            .collect(),
    };

    for utt in &utterances {
        if args.print_words {
            print_relation(utt, "Word", Detail::Name);
        }
        if args.print_segments {
            print_relation(utt, "Segment", Detail::Name);
        }
        if args.print_segment_durations {
            print_relation(utt, "Segment", Detail::EndTime);
        }
        if args.print_segment_stress {
            print_relation(utt, "Segment", Detail::Stress);
        }
        if let Some(name) = &args.print_relation {
            print_relation(utt, name, Detail::Name);
        }
    }
    Ok(())
}

/// What to print alongside each item's name.
enum Detail {
    Name,
    EndTime,
    /// The stress of the syllable a vowel belongs to, and nothing for anything
    /// that is not a vowel.
    Stress,
}

fn print_relation(utt: &Utterance, relation: &str, detail: Detail) {
    let mut line = String::new();
    for item in utt.iter_relation(relation) {
        match detail {
            Detail::Name => line.push_str(utt.name(item)),
            Detail::EndTime => line.push_str(&format!(
                "{}:{:.3}",
                utt.name(item),
                utt.feature_f32(item, "end")
            )),
            Detail::Stress => {
                line.push_str(utt.name(item));
                if ffeature::eval_str(utt, item, "ph_vc").as_str() == "+" {
                    line.push_str(
                        ffeature::eval_str(utt, item, "R:SylStructure.parent.stress").as_str(),
                    );
                }
            }
        }
        line.push(' ');
    }
    println!("{line}");
}

/// Apply one `name=value` to the current voice.
///
/// The names are the fields of `VoiceParams`, which are upstream's feature
/// names, so a command written for one engine sets the same thing in the other.
fn set_parameter(engine: &mut Engine, assignment: &str) -> io::Result<()> {
    let (name, value) = assignment
        .split_once('=')
        .ok_or_else(|| unusable(format!("expected NAME=VALUE, got {assignment:?}")))?;

    let number = || {
        value
            .parse::<f32>()
            .map_err(|_| unusable(format!("{name} needs a number, got {value:?}")))
    };
    let params = engine.params_mut();
    match name {
        "int_f0_target_mean" => params.int_f0_target_mean = number()?,
        "int_f0_target_stddev" => params.int_f0_target_stddev = number()?,
        "duration_stretch" => params.duration_stretch = number()?,
        "f0_shift" => params.f0_shift = number()?,
        "join_type" => {
            params.join_type = match value {
                "none" => JoinType::None,
                "simple_join" => JoinType::Simple,
                "modified_lpc" => JoinType::ModifiedLpc,
                _ => return Err(unusable(format!("unknown join_type {value:?}"))),
            }
        }
        "resynth_type" => {
            params.resynth_type = match value {
                "fixed" => ResynthType::Fixed,
                "float" => ResynthType::Float,
                _ => return Err(unusable(format!("unknown resynth_type {value:?}"))),
            }
        }
        _ => return Err(unusable(format!("unknown parameter {name:?}"))),
    }
    Ok(())
}

fn unusable(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
