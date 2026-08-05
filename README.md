# flite-rs

Text-to-speech in pure Rust. Small, fast, and dependency-free.

A clean-room implementation of the diphone concatenative synthesis used by CMU's
Flite and Festival: CART-based duration, phrasing and intonation models,
rule-based letter-to-sound, and residual-excited LPC waveform generation. No C,
no FFI, no `build.rs`, no `unsafe`. The models and the voice are embedded in the
binary, so there is nothing to install and nothing to load at runtime.

Ships with one voice, `cmu_us_kal` (8 kHz, male, US English), and US English
text handling.

## Install

```toml
[dependencies]
flite-rs = "0.1"
```

The library has no dependencies with `default-features = false`. The default
`cli` feature pulls in `clap` for the command-line tool.

```console
$ cargo install flite-rs
```

## Usage

```rust
use flite_rs::Engine;

let engine = Engine::new();
let audio = engine.synthesize("Hello from Rust.");

let mut file = std::fs::File::create("hello.wav")?;
flite_rs::write_wav(&audio, &mut file)?;
```

Speed and pitch:

```rust
let mut engine = Engine::new();
engine.set_duration_stretch(1.3); // slower
engine.set_f0_shift(1.1);         // higher
```

`Engine` is `Send + Sync` and all synthesis methods take `&self`, so a single
engine can be shared across threads. Construction parses the embedded tables;
build one and reuse it.

### Command line

```console
$ flite-rs "The quick brown fox jumps over the lazy dog." -o fox.wav
$ flite-rs -f script.txt -o script.wav --speed 1.2
$ echo "Piped text works too." | flite-rs -o out.wav
$ flite-rs "Bookkeeper" --phones
pau b uh k k iy p er pau
```

`--phones` prints the phone sequence without generating audio, which is the
quickest way to check a pronunciation.

## Compatibility

Output is bit-identical to upstream Flite for the `cmu_us_kal` voice. A
600-sentence corpus covering Flite's own text-normalisation regression tests,
numbers, money, dates, phone numbers, abbreviations and nonsense words was
synthesised by both engines and compared sample by sample: 30,147,852 bytes of
audio, no differences.

Two intentional deviations:

- Upstream's streaming file writer emits a WAV byte-rate field of twice the
  correct value. `flite-rs` writes the correct value, matching upstream's own
  non-streaming writer. Audio samples are unaffected.
- Four US state names were misspelled in the upstream expansion table and are
  spelled correctly here.

## Performance

Synthesising a 600-sentence corpus, 31 minutes of speech:

| | time | faster than real time |
|---|---|---|
| `flite-rs` (release) | 1.5 s | ~1280× |
| Flite 2.3 (`cl /O2`) | 5.1 s | ~370× |

`Engine::new()` retains about 80 KiB of heap and does no decoding of the
lexicon or voice, which are read directly from the binary. Synthesis allocates
roughly 45 KiB per second of audio produced.

## Platform support

Runs on any target with `std`: Linux, macOS, Windows, BSD, Android, iOS and
WASM, on x86-64, aarch64, ARMv7, RISC-V and others. There is no
platform-specific code, no filesystem access and no threading.

Data files are read with explicit little-endian conversions rather than by
transmuting bytes, so big-endian targets work unchanged. The DSP path is
integer-only, so output is bit-identical across targets regardless of
floating-point settings, and it does not need an FPU.

`std` is used in one place, the WAV writer. A `no_std` build needs a feature
gate there plus `alloc`.

## Architecture

Two stages. The linguistic pipeline (`src/pipeline.rs`) turns text into phones
with durations and a pitch contour:

```
Token  --normalise-->  Word  --tag--> pos
                         |--phrase--> Phrase
                         '--look up-> SylStructure / Syllable / Segment
Syllable --intonation--> accent, endtone
Segment  --duration--->  end times
Syllable --F0 model---->  Target
```

Each stage adds a relation to a heterogeneous relation graph
(`src/utterance.rs`), in which one linguistic object appears in several
relations at once: a word is a node in the flat `Word` chain and also the root
of a tree in `SylStructure`, sharing one feature set. The trained models query
the graph through feature paths (`src/ffeature.rs`) such as
`R:SylStructure.parent.parent.gpos`, parsed once when the models load.

The voice (`src/voice.rs`) generates the waveform from diphones, recorded
transitions from the middle of one phone to the middle of the next, so joins
fall in the steady part of a sound. The pitch contour becomes a sequence of
pitch marks; each output pitch period takes its LPC coefficients and excitation
from the nearest period of the selected unit, and the result runs through the
synthesis filter in `src/dsp.rs`.

| Module | Responsibility |
|---|---|
| `text` | Tokenisation and sentence splitting |
| `normalize`, `numbers`, `patterns`, `lang` | Turning tokens into speakable words |
| `lexicon` | Dictionary lookup, letter-to-sound rules, syllabification |
| `cart`, `ffeature`, `value` | Decision trees and the feature language they query |
| `utterance` | The relation graph |
| `pipeline` | Stage ordering and prosody models |
| `voice`, `dsp` | Unit selection and waveform generation |
| `data`, `language` | Reading the embedded data files |

To inspect the analysis of a sentence without generating audio:

```console
$ cargo run --example analysis -- "Hello, and welcome."
```

## Data

The voice and the trained models come from Flite and Festival and are
redistributed under their original permissive terms. A dictionary and a recorded
speaker cannot be reimplemented, only reused. `THIRD-PARTY-LICENSES.md` records
what came from where, what was changed and who to credit; those terms cover the
data files only and impose nothing on the Rust code.

`tools/gen_data.py` regenerates `data/*.dat` from a Flite source tree, needed
only when rebuilding or replacing the data:

```console
$ python tools/gen_data.py /path/to/flite
```

## Roadmap

- **`cmu_us_kal16`**, the same speaker at 16 kHz. The voice loader already
  reads sample rate, LPC order and coefficient range from the data file; this
  additionally needs a codec flag in the voice header, since its residuals are
  raw µ-law rather than ADPCM, and the per-voice prosody constants moved out of
  `lib.rs`.
- **Runtime dictionaries**, for custom and domain-specific pronunciations. The
  static addenda table in `lexicon.rs` is the existing mechanism; making it
  settable at runtime means relaxing the `&'static str` phone slices that keep
  lookup allocation-free.
- **Streaming synthesis**, so memory does not scale with utterance length. The
  DSP already runs a pitch period at a time.
- **A `Language` trait.** The pipeline is language-neutral; the models are not.
  The English-specific parts are the phone inventory (`phoneset.rs`), the
  syllabifier and its onset clusters (`lexicon.rs`), the duration and F0 tables
  (`pipeline.rs`), and the word lists and normalisation rules (`lang.rs`,
  `normalize.rs`, `numbers.rs`, `patterns.rs`). Upstream has grapheme-based and
  Indic models to port once this exists. A new language also needs a voice
  recorded for it, since the two must share a phoneset.
- **`no_std` support.**

Not planned: SSML, and audio playback. Playback belongs to the caller; owning
it would mean owning a platform audio dependency.

Clustergen voices (`slt`, `rms`, `awb`) sound better than any diphone voice but
use statistical parametric synthesis, which would replace everything below the
linguistic pipeline rather than extend it. Out of scope for now.

## Contributing

Bug reports and patches welcome.

The models are sensitive to details that look like mistakes. A pitch average
mixes two different scales, a syllable-counting feature stops one syllable short
of its mirror image, a count saturates at 19. These are commented where they
occur; changing them changes the prosody. When touching the pipeline or the DSP,
compare output against upstream Flite before and after. `--phones` and
`cargo run --example analysis` narrow down where a divergence begins.

```console
$ cargo test
$ cargo clippy --all-targets
```

## Licence

Apache-2.0. See `LICENSE`, `NOTICE` and `THIRD-PARTY-LICENSES.md`.
