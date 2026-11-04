# Verifying against upstream Flite

`flite-rs` produces the same samples as CMU Flite — for the diphone voices
`cmu_us_kal` and `cmu_us_kal16`, and for the Clustergen voice `cmu_us_slt` —
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

The build compiles with `-ffp-contract=off` on `cc` and `/fp:precise` on MSVC,
and both are load bearing. GCC defaults to `-ffp-contract=fast`, which fuses
`a*b+c` into one FMA instruction and drops the rounding of the intermediate
product. GCC 13 did not do that in the duration model and GCC 15 does, which
moves one segment boundary by a microsecond, moves a pitch mark by one sample,
and makes a few hundred samples differ before the output resynchronises. Rust
never contracts without an explicit `fma()`, so the engine cannot follow the
reference into FMA, and it is the reference that has to hold still. Without the
flag, whether this test passes depends on which compiler built it.

  `reffile`    text file in, WAV out. Trailing `NAME=VALUE` arguments set voice
               features, and `voice=NAME` picks the voice: `kal16` for the
               16 kHz diphone one, `slt` for Clustergen. That is how the paths
               a voice does not ask for by itself get exercised.
  `refdump`    the linguistic structure of one sentence. `-p` reads it as
               phones, `-u` adds the selected units and output pitch marks.
  `refphones`  a phone string in, WAV out.
  `reftrack`   everything Clustergen predicts for one sentence short of the
               waveform: state durations, the parameter track, the band
               strengths and the MLPG result. `-f` instead prints every feature
               the state duration tree consults, for every state.

Upstream's own `flite_main.c` is not used: it includes `<sys/time.h>` and
`<unistd.h>`, which MSVC does not have. The small drivers here replace it.

## Running the comparison

```bash
cargo test --release --test reference
```

The test synthesises every line of `corpus.txt` with both engines and requires
every sample to match. It also feeds the whole corpus through as a single file,
which is the only thing that exercises a chunk boundary inside a token or a
sentence break across one. Add `--features kal16` to check the 16 kHz voice and
`--features clustergen` to check the Clustergen one. Without the reference
binary the tests report that they were skipped and pass, so a plain checkout
still runs green.

`reffile` uses `flite_file_to_speech`, not `flite_text_to_speech`. The former
splits its input into sentences, which is what `Engine::synthesize` does; the
latter treats the whole input as one utterance and will disagree with us on
anything longer than a sentence for reasons that have nothing to do with a bug.

## Verifying Clustergen

Clustergen is checked three ways, and the order matters because each is sharper
than the last at saying *where* a divergence is.

`the_clustergen_voice_matches_upstream` is the corpus comparison, every line,
sample for sample, through `reffile voice=slt`.

`reading_a_file_in_chunks_matches_upstream_on_clustergen` runs the whole corpus
as one file, which is the only test that covers the excitation's continuity
across sentences — see the pinned-generator section below for why that is a
property at all.

`clustergen_trajectories_match_upstream` compares what `reftrack` prints
against what the engine predicts: the segment end times, the HMM states with
their durations, the parameter track after F0 splining and rescaling, the band
strengths, and the track after MLPG. These are compared as bit patterns, not
approximately, because both sides are deterministic there. This is the test to
read first when something breaks: it names the stage. It also covers the MLPG
smoothing, which the waveform tests only reach through the vocoder.

`examples/cgtrack.rs` prints the same format as `reftrack`, so the two diff:

```bash
tools/reference/build/reftrack "The quick brown fox." > ref.txt
cargo run --release --features clustergen --example cgtrack -- \
    "The quick brown fox." > ours.txt
diff ref.txt ours.txt
```

Whichever section differs first names the stage. `SEGMENTS` and `STATES` are
the state duration trees; `PARAM` is the F0 and parameter trees, the model
store and the F0 splining; `STR` is the band strengths; `MLPG` is the
smoothing. If every section agrees and the audio still differs, it is the
vocoder.

When the end times differ and the phones do not, `-f` on both sides prints
every feature the duration tree consults and the first differing column names
the feature:

```bash
tools/reference/build/reftrack -f "+49." > ref.txt
cargo run --release --features clustergen --example cgtrack -- -f "+49." > ours.txt
diff ref.txt ours.txt
```

That is how the `+` normalisation bug was found, in one step, after the audio
comparison had only said "sample 165515 of 234480".

### The pinned pseudo-random generator

Upstream's MLSA vocoder draws unvoiced excitation from the C library's
`rand()`:

```c
static double plus_or_minus_one()
{
    /* not sure rand() is portable */
    if (rand() > RAND_MAX/2.0)
        return 1.0;
    else
        return -1.0;
}
```

Its own comment is right, and this matters more than it looks. `rand()` is not
specified by the C standard beyond its range, `srand` is never called, and with
mixed excitation — which all three Clustergen voices upstream ships in-tree ask
for — it is consumed on *every output sample*. glibc's `rand()` is the TYPE_3
additive-feedback generator of `random()`; MSVC's is a 32-bit LCG; they agree on
nothing. So a stock Flite's Clustergen waveform is a function of the C library
that built it, and "bit-identical to upstream" is not a well-defined question
without naming an upstream.

This is the same class of problem as `-ffp-contract=off` above, and it gets the
same answer: an unspecified property of the C environment is pinned so that the
reference holds still, rather than being inherited from whichever compiler built
it. What it is pinned to is **glibc's `rand()`**, because that is what upstream
is overwhelmingly built and distributed with and what CI uses.

`refrand.c` reimplements that generator and `refrand.h` is force-included ahead
of every translation unit — `-include` on `cc`, `/FI` on MSVC — which is where
`rand` is redirected to it and `RAND_MAX` is set to glibc's.
`src/clustergen/random.rs` is the engine's half, and its tests check the
sequence against the values `rand()` is universally known to produce from the
default seed.

The consequence, stated plainly:

- On a glibc host the shim reproduces exactly the sequence the real `rand()`
  would have returned, so the reference behaves as unmodified upstream and the
  comparison is against stock Flite.
- On MSVC, musl or a BSD it makes the reference behave as it would have on
  glibc, so the comparison can run there at all. What is being compared is then
  not that host's stock Flite.

Both sides are pinned to one documented generator rather than to the host, which
is the only arrangement under which the question has an answer. Removing the
force-include is not a cleanup; it makes the Clustergen comparison depend on the
compiler, exactly as removing `-ffp-contract=off` does.

A second consequence is behavioural rather than about testing. Because the
generator is process-global upstream, the second sentence of one
`flite_file_to_speech` call continues the sequence the first left off: two
sentences spoken together are not the same audio as the same two spoken
separately. `Backend::start` exists so that `flite-rs` can match that — it opens
a run that the utterances of one synthesis call share — and the whole-corpus
test is what pins it. A backend that restarted its excitation per utterance
passes the per-sentence comparison and fails that one in the second sentence.

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

For a Clustergen voice this bisection order is different, because its durations
and pitch come from its own trees rather than from the stages above: use
`reftrack` against `cgtrack`, as the Clustergen section describes. Step 1 still
applies — phones first, always — and `refdump -p` and `-u` do not, since
Clustergen selects no units.

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
