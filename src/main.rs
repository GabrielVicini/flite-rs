//! Command-line front-end for `flite-rs`.
//!
//! Reads text from an argument, a file, or standard input and writes a WAV
//! file (or raw samples to stdout).

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use flite_rs::Engine;

#[derive(Parser)]
#[command(
    name = "flite-rs",
    version,
    about = "Small, fast, portable text to speech",
    after_help = "\
EXAMPLES:
  flite-rs \"Hello there.\" -o hello.wav
  flite-rs -f speech.txt -o speech.wav
  echo \"Piped text.\" | flite-rs -o out.wav
  flite-rs \"Bookkeeper\" --phones"
)]
struct Args {
    /// Text to speak. Reads standard input when absent and no file is given.
    text: Option<String>,

    /// Read the text from a file instead.
    #[arg(short = 'f', long, value_name = "FILE")]
    file: Option<PathBuf>,

    /// Output WAV file; use "-" for standard output.
    #[arg(short, long, value_name = "FILE", default_value = "out.wav")]
    output: PathBuf,

    /// Speech rate multiplier; above 1.0 is slower.
    #[arg(short = 's', long, value_name = "FACTOR", default_value_t = 1.0)]
    speed: f32,

    /// Pitch multiplier; above 1.0 is higher.
    #[arg(short = 'p', long, value_name = "FACTOR", default_value_t = 1.0)]
    pitch: f32,

    /// Print the phone sequence instead of writing audio.
    #[arg(long)]
    phones: bool,

    /// Print the duration of the synthesised audio to standard error.
    #[arg(short = 'v', long)]
    verbose: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("flite-rs: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args) -> io::Result<()> {
    let text = read_input(&args)?;
    if text.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no text to speak",
        ));
    }

    let mut engine = Engine::new();
    // The CLI exposes speed as "how much slower", which is the same direction
    // as the underlying duration stretch.
    engine.set_duration_stretch(args.speed);
    engine.set_f0_shift(args.pitch);

    if args.phones {
        println!("{}", engine.phones(&text));
        return Ok(());
    }

    let audio = engine.synthesize(&text);
    if args.verbose {
        eprintln!(
            "{:.2} s, {} samples at {} Hz",
            audio.duration_seconds(),
            audio.samples.len(),
            audio.sample_rate
        );
    }

    if args.output.as_os_str() == "-" {
        let stdout = io::stdout();
        let mut out = BufWriter::new(stdout.lock());
        flite_rs::write_wav(&audio, &mut out)?;
        out.flush()
    } else {
        let mut out = BufWriter::new(File::create(&args.output)?);
        flite_rs::write_wav(&audio, &mut out)?;
        out.flush()
    }
}

fn read_input(args: &Args) -> io::Result<String> {
    if let Some(path) = &args.file {
        return std::fs::read_to_string(path);
    }
    if let Some(text) = &args.text {
        return Ok(text.clone());
    }
    let mut text = String::new();
    io::stdin().read_to_string(&mut text)?;
    Ok(text)
}
