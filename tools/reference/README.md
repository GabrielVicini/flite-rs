# Verifying against upstream Flite

`flite-rs` produces the same samples as CMU Flite for the `cmu_us_kal` voice,
byte for byte. That property is the reason the crate exists, so it is checked
rather than assumed: this directory builds a real Flite and compares its output
with ours over a corpus, sample by sample.

Nothing here is needed to use the crate. It is needed before changing anything
that can reach the audio.

## Building the reference

You need an upstream Flite source tree (2.3 or compatible) and a C compiler.
On Windows that means the Visual Studio C++ toolchain; elsewhere any `cc`.

```bash
python tools/reference/build.py --flite-src PATH/TO/flite
```

This writes `reffile`, `refdump` and `refphones` into `tools/reference/build`,
which is ignored by git. Object files are cached there, so a second run is
quick.

  `reffile`    text file in, WAV out. Trailing `NAME=VALUE` arguments set voice
               features, and `voice=kal16` picks the 16 kHz voice, which is how
               the paths a voice does not ask for by itself get exercised.
  `refdump`    the linguistic structure of one sentence. `-p` reads it as
               phones, `-u` adds the selected units and output pitch marks.
  `refphones`  a phone string in, WAV out.

Upstream's own `flite_main.c` is not used: it includes `<sys/time.h>` and
`<unistd.h>`, which MSVC does not have. The small drivers here replace it.

## Running the comparison

```bash
cargo test --release --test reference
```

The test synthesises every line of `corpus.txt` with both engines and requires
every sample to match. It also feeds the whole corpus through as a single file,
which is the only thing that exercises a chunk boundary inside a token or a
sentence break across one. Add `--features kal16` to check the 16 kHz voice too.
Without the reference binary the tests report that they were skipped and pass,
so a plain checkout still runs green.

`reffile` uses `flite_file_to_speech`, not `flite_text_to_speech`. The former
splits its input into sentences, which is what `Engine::synthesize` does; the
latter treats the whole input as one utterance and will disagree with us on
anything longer than a sentence for reasons that have nothing to do with a bug.

## When something diverges

Do not guess, and do not start adjusting constants. Bisect the pipeline in the
order the stages run, and stop at the first one that differs.

1. Phones. `cargo run --release -- "text" --phones` against the `SEGMENTS`
   section of `refdump`. A difference here is normalisation, the lexicon or
   letter-to-sound, and the audio is a red herring.
2. Segment end times, the rest of that same section. A difference here is the
   duration model or a feature it reads.
3. Pitch targets, the `TARGETS` section. A difference here is the F0 model or a
   feature it reads.
4. Individual features. Add the features that the relevant CART consults to a
   dump on both sides and diff those. Every remaining bug in the original port
   was found this way, and none of them were found by listening.

`refdump` prints in the same format as `cargo run --example analysis`, so steps
1 to 3 are a plain diff:

```bash
tools/reference/build/refdump "The quick brown fox." > ref.txt
cargo run --release --example analysis -- "The quick brown fox." > ours.txt
diff ref.txt ours.txt
```

Both take `voice=NAME` and `-p`, and they must agree: analysing with the wrong
voice produces a difference that is entirely your own doing, because the
postlexical rules and the pitch range belong to the voice.

If those sections agree and the audio still differs, the divergence is in unit
selection or in the pitch marks. `refdump -u` prints both, and the engine will
print its own if `FLITE_DEBUG_UNITS` is set.

`refdump` synthesises its argument as a single utterance, so give it one
sentence. For anything longer, compare audio with `reffile`.

## The corpus

`corpus.txt` covers Flite's own normalisation tests from `testsuite/us.flitecheck`,
then numbers, money, dates, times, phone numbers, ordinals, fractions,
abbreviations, initialisms, place names and unit suffixes, then multi-sentence
input and all three terminators, then nonsense words that force the
letter-to-sound path, and finally generated sentences long enough to saturate
the counting features.

The generated sections came from a seeded run and were then frozen. Regenerating
them each time would let a regression hide behind a corpus that had quietly
moved, which is the one thing this test exists to prevent.

Adding cases is welcome. Adding a case that fails is a finding, not a broken
test: check it against upstream before assuming our side is wrong, because
several behaviours that look like bugs are inherited from the models as trained
and are commented where they occur.
