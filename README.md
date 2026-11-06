# flite-rs

[![CI](https://github.com/GabrielVicini/flite-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/GabrielVicini/flite-rs/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/flite-rs.svg)](https://crates.io/crates/flite-rs)
[![docs.rs](https://docs.rs/flite-rs/badge.svg)](https://docs.rs/flite-rs)

Text-to-speech in pure Rust. Small, fast, and dependency-free.

A clean-room implementation of the synthesis used by CMU's Flite and Festival:
CART-based duration, phrasing and intonation models, rule-based
letter-to-sound, and two waveform backends — residual-excited LPC over a
diphone database, and Clustergen statistical parametric synthesis through an
MLSA filter. No C, no FFI, no `build.rs`, no `unsafe`. The models and the
voices are embedded in the binary, so there is nothing to install and nothing
to load at runtime.

There is a [live demo](https://flite-rs.vicini.io/) that runs the engine in the
browser — both backends, no server, nothing uploaded. It is generated from this
repository on every push; see [`web/`](web/README.md) for how it is built and
how to change what it says.

Ships with `cmu_us_kal` (8 kHz, male, US English) and US English text handling;
optionally the same speaker at 16 kHz, and the `cmu_us_slt` Clustergen voice
(16 kHz, female).

## Install

```toml
[dependencies]
flite-rs = "0.3.0"
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

The 8 kHz `kal` voice is built in. Two features add more, and both are off by
default because each costs binary size and because a build flag should add a
choice rather than change what a caller who asked for nothing hears:

| feature      | voice                | adds   | notes                              |
| ------------ | -------------------- | ------ | ---------------------------------- |
| `kal16`      | `kal16`, 16 kHz male | ~4 MB  | the same speaker and recordings, uncompressed |
| `clustergen` | `slt`, 16 kHz female | ~2.5 MB | a different synthesiser, not a second database |

```toml
flite-rs = { version = "0.3.0", features = ["kal16", "clustergen"] }
```

```rust
engine.select_voice("kal16");
engine.select_voice("slt");
```

Neither feature adds a dependency, and `--no-default-features` stays
dependency-free.

#### Clustergen

`slt` is not another diphone database; it is a different technique behind the
same API. There are no recordings in it. Speech is modelled as a trajectory
through a mel-cepstral parameter space: each phone is split into the sub-phone
states its models were trained on, filled with frames at 200 a second, and each
frame is classified by a regression tree into a cluster whose mean and variance
say what the spectrum should be there. [Maximum-likelihood parameter
generation](src/clustergen/mlpg.rs) then solves for the smoothest trajectory
consistent with those distributions and their deltas, and an [MLSA
filter](src/clustergen/mlsa.rs) excites it with a pulse train, noise, or a
mixture of the two in five bands.

It sounds quite different from the diphone voices — smoother and more muffled,
which is the characteristic trade of statistical parametric synthesis — and it
is a little slower, because a frame of MLSA costs more than a pitch period of
LPC.

Which backend a voice uses is read from the voice file, not decided by its
name, so adding a third needs no change to `Engine`. See
[`src/voice.rs`](src/voice.rs) for the seam and
[`docs/clustergen-voice-data.md`](docs/clustergen-voice-data.md) for where the
voice data comes from.

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

Output is bit-identical to upstream Flite, for all three voices, for both
backends, for text and for phone strings. A corpus covering Flite's own
text-normalisation regression tests, numbers, money, dates, phone numbers,
abbreviations, place names and nonsense words is synthesised by both engines and
compared sample by sample on every test run; so is the whole corpus as one file,
a quarter of an hour of continuous speech in a single pass. For Clustergen the
predicted trajectories are compared too — state durations, F0 contour,
parameter track and the MLPG result — so that a divergence lands on a named
stage rather than on a sample number. `tools/reference` builds the Flite it is
all checked against, and its README explains how.

Clustergen comes with one qualification that the diphone path does not have,
and it is upstream's rather than ours. Its vocoder draws unvoiced excitation
from the C library's `rand()`, whose sequence is not specified by the C standard
and which upstream never seeds; glibc's and MSVC's agree on nothing, and with
mixed excitation — which every Clustergen voice upstream ships asks for — it is
consumed on every output sample. So a stock Flite's Clustergen waveform is a
function of the C library that built it, and "bit-identical to upstream" needs
a particular upstream to mean anything.

`flite-rs` reimplements glibc's generator, because that is what upstream is
overwhelmingly built and distributed with. Against a glibc-built Flite the match
is therefore against stock upstream. Elsewhere the reference build substitutes
the same generator, so that the comparison can run at all — the same reasoning
and the same mechanism as the floating-point contraction flags that reference
build already pins. `src/clustergen/random.rs` and
`tools/reference/refrand.c` are the two halves of it.

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

Clustergen's excitation continues across the sentences of one synthesis call,
as upstream's global `rand()` does, and starts afresh on the next call. So
`synthesize` on two sentences is not `synthesize` twice, which is upstream's
behaviour and not a choice made here; synthesis is still deterministic, since
every call starts from the same state.

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

The browser target needs nothing beyond `cargo build --target
wasm32-unknown-unknown`: [`wasm/`](wasm/src/lib.rs) is a small `cdylib` that
exposes the engine through a dozen `extern "C"` functions, with no
`wasm-bindgen` and no JavaScript toolchain in the build.

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

The second stage is a *backend* (`src/voice.rs`), and there are two of them.
`Backend` is the trait at the seam: a run of speech in, samples out. `Engine`
holds `Arc<dyn Backend>` rather than being generic, because one engine holds
voices of different kinds at once; dispatch is one virtual call per utterance.
Which backend a voice uses is named in its data file, so adding a third needs no
change to `Engine` and nothing in `src/` maps a voice name to a technique.

The diphone backend (`src/diphone.rs`) generates the waveform from diphones,
recorded transitions from the middle of one phone to the middle of the next, so
joins fall in the steady part of a sound. The pitch contour becomes a sequence
of pitch marks; each output pitch period takes its LPC coefficients and
excitation from the nearest period of the selected unit, and the result runs
through the synthesis filter in `src/dsp.rs`.

The Clustergen backend (`src/clustergen/`) has no recordings. It splits each
phone into HMM states, lays frames over them at a fixed rate, classifies each
frame into a cluster of mel-cepstral parameters, smooths the resulting
trajectory with MLPG and runs it through an MLSA filter. It predicts its own
durations and pitch, so it declines the pipeline's segment-duration and F0
target models — which is how upstream arranges it too.

| Module | Responsibility |
|---|---|
| `text` | Tokenisation and sentence splitting |
| `normalize`, `numbers`, `patterns`, `lang` | Turning tokens into speakable words |
| `lexicon` | Dictionary lookup, letter-to-sound rules, syllabification |
| `cart`, `ffeature`, `value` | Decision trees and the feature language they query |
| `utterance` | The relation graph |
| `pipeline` | Stage ordering and prosody models |
| `voice` | The backend seam: `Backend`, `Speaking`, `VoiceProfile`, `VoiceParams` |
| `diphone`, `dsp` | Unit selection and residual-excited LPC |
| `clustergen` | Statistical parametric synthesis: trees, MLPG, MLSA |
| `data`, `language` | Reading the embedded data files |

To inspect the analysis of a sentence without generating audio:

```console
$ cargo run --example analysis -- "Hello, and welcome."
```

It prints the phones with their end times, the intonation decisions and the
pitch contour, in the same format `tools/reference/refdump` prints them, so the
two can be diffed when something diverges.

For Clustergen there is a second one, printing everything it predicts short of
the waveform against the format `tools/reference/reftrack` prints:

```console
$ cargo run --features clustergen --example cgtrack -- "Hello, and welcome."
```

With `-f` it prints instead every feature the state duration tree consults, for
every state, which is what to reach for when the end times differ and the
phones do not.

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

That covers the Clustergen voice too: upstream ships `cmu_us_slt` in-tree as
generated C arrays, exactly like the diphone data, so no separate `.flitevox`
download is involved. `docs/clustergen-voice-data.md` records that decision and
how to reproduce it.

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

- **The other two Clustergen voices.** `awb` converts the same way `slt` does.
  `rms` needs the SPAM F0 model, which is a separate set of trees and is not
  implemented; `tools/gen_data.py` refuses a voice that asks for it rather than
  converting something the backend would ignore.

Not planned: SSML, and audio playback. Playback belongs to the caller; owning
it would mean owning a platform audio dependency.

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
