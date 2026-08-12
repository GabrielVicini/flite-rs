# flite-rs

Text-to-speech in pure Rust. Small, fast, and dependency-free.

A clean-room implementation of the diphone concatenative synthesis used by CMU's
Flite and Festival: CART-based duration, phrasing and intonation models,
rule-based letter-to-sound, and residual-excited LPC waveform generation. No C,
no FFI, no `build.rs`, no `unsafe`. The models and the voice are embedded in the
binary, so there is nothing to install and nothing to load at runtime.

Feel free to check out the [WebAssembly live demo](https://flite-rs.vicini.io/).

Ships with `cmu_us_kal` (8 kHz, male, US English), optionally the same speaker
at 16 kHz, and US English text handling.

## Install

```toml
[dependencies]
flite-rs = "0.2.0"
```

The library has no dependencies with `default-features = false`. The default
`cli` feature pulls in `clap` for the command-line tool. The optional `kal16`
feature embeds a second voice; see [Voices](#voices).

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

Audio as it is produced, rather than all at the end:

```rust
engine.synthesize_streaming("A long piece of text.", |period| {
    play(period);
    flite_rs::Flow::Continue // or Stop, to give up on the rest
});
```

`synthesize_reader` does the same from anything implementing `Read`, consuming
it in chunks, so a file of any size is synthesised in constant memory.

Saying exactly the phones you want, and correcting one the dictionary gets
wrong:

```rust
engine.synthesize_phones("pau hh ax l ow pau");
engine.add_lex_entries("kubernetes : k uw b er n eh1 t iy z")?;
```

`Engine` is `Send + Sync` and all synthesis methods take `&self`, so a single
engine can be shared across threads. Construction parses the embedded tables;
build one and reuse it.

### Voices

The 8 kHz `kal` voice is built in. The `kal16` feature adds the same speaker at
16 kHz, which sounds considerably better and costs about 4 MB, since its
recordings are not compressed:

```toml
flite-rs = { version = "0.2.0", features = ["kal16"] }
```

```rust
engine.select_voice("kal16");
```

### Command line

The options follow upstream `flite`, including its single-dash multi-letter
spellings.

```console
$ flite-rs "The quick brown fox jumps over the lazy dog." fox.wav
$ flite-rs -f script.txt -o script.wav --speed 1.2
$ echo "Piped text works too." | flite-rs -o out.wav
$ flite-rs -p "pau hh ax l ow pau" -o hello.wav
$ flite-rs -t Bookkeeper -ps
pau b uh k k iy p er pau
```

A bare argument containing a space is spoken as text and otherwise names a file
to read, as upstream does; `-t` and `-f` say which explicitly. A second bare
argument is the WAV file to write.

`-ps` prints the phones without generating audio, which is the quickest way to
check a pronunciation; `-psdur` adds their end times, `-pw` prints the words and
`-pr NAME` prints any relation. `-p` speaks a phone string directly, `--add_lex`
loads extra pronunciations, `--set/--setf/--sets NAME=VALUE` set voice
parameters, `-lv` lists the voices and `-b` benchmarks.

## Compatibility

Output is bit-identical to upstream Flite, for both voices, for text and for
phone strings. A corpus covering Flite's own text-normalisation regression
tests, numbers, money, dates, phone numbers, abbreviations, place names and
nonsense words is synthesised by both engines and compared sample by sample on
every test run; so is the whole corpus as one file, a quarter of an hour of
continuous speech in a single pass. `tools/reference` builds the Flite it is
checked against.

Three intentional deviations, all cases where upstream is wrong:

- Upstream's streaming file writer emits a WAV byte-rate field of twice the
  correct value. `flite-rs` writes the correct value, matching upstream's own
  non-streaming writer. Audio samples are unaffected.
- Four US state names were misspelled in the upstream expansion table and are
  spelled correctly here.
- Upstream's floating-point resynthesiser, which no shipped voice selects, sizes
  its output from the last pitch mark and never trims it back to what it
  generated, so it returns up to an eighth of a second of trailing silence. The
  samples are the same; the silence is not reproduced.

A syllable break in a phone string, `-`, aborts upstream `flite -p` outright.
Here it does what upstream's own documentation says it should.

## Performance

Synthesising `tools/reference/corpus.txt`, 16.7 minutes of speech, with the
audio discarded so that the disk is not being measured. Whole-process wall
clock, best of five, same machine, same input:

| | time | faster than real time |
|---|---|---|
| `flite-rs` (release) | 0.71 s | ~1410× |
| Flite 2.3 (`cl /O2`) | 1.15 s | ~870× |

```console
$ flite-rs -f tools/reference/corpus.txt -o none -v
```

`Engine::new()` retains about 80 KiB of heap and does no decoding of the
lexicon or voice, which are read directly from the binary. `synthesize`
allocates roughly 45 KiB per second of audio produced, since it returns all of
it; `synthesize_streaming` and `synthesize_reader` hand over each pitch period
as it is made and hold nothing, so memory is flat in the length of both the
input and the speech.

## Platform support

Runs on any target with `std`: Linux, macOS, Windows, BSD, Android, iOS and
WASM, on x86-64, aarch64, ARMv7, RISC-V and others. There is no
platform-specific code, no filesystem access and no threading.

Data files are read with explicit little-endian conversions rather than by
transmuting bytes, so big-endian targets work unchanged. The synthesis filter is
integer-only on the path both voices take, so output is bit-identical across
targets regardless of floating-point settings, and it does not need an FPU.
Upstream's floating-point filter is implemented too, for parity, and carries no
such guarantee; nothing selects it unless you ask for it by name.

`std` is used for the WAV writer and for reading streamed input. A `no_std`
build needs a feature gate there plus `alloc`.

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

It prints the phones with their end times, the intonation decisions and the
pitch contour, in the same format `tools/reference/refdump` prints them, so the
two can be diffed when something diverges.

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

- **A `Language` trait.** The pipeline is language-neutral; the models are not.
  The English-specific parts are the phone inventory (`phoneset.rs`), the
  syllabifier and its onset clusters (`lexicon.rs`), the duration and F0 tables
  (`pipeline.rs`), and the word lists and normalisation rules (`lang.rs`,
  `normalize.rs`, `numbers.rs`, `patterns.rs`). Upstream has grapheme-based and
  Indic models to port once this exists. A new language also needs a voice
  recorded for it, since the two must share a phoneset, and Flite ships no
  diphone voice outside US English: adding German or any other language means
  finding or recording the data, not writing code.
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
compare output against upstream Flite before and after. `tools/reference` builds
a real Flite and checks a corpus against it sample by sample; see the README
there for the build and for how to bisect a divergence once you have one.

```console
$ cargo test --release
$ cargo clippy --all-targets
```

## Licence

Apache-2.0. See `LICENSE`, `NOTICE` and `THIRD-PARTY-LICENSES.md`.
