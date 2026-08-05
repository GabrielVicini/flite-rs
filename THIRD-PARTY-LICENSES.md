# Third-party data and licenses

The Rust source code in this repository is licensed under the Apache License,
Version 2.0 (see `LICENSE`). It is an independent implementation of published
speech-synthesis techniques and is not derived from the source code of any
other synthesiser.

The **data** is a different matter. Speech data, meaning a dictionary, trained models
and recorded speech, cannot be reimplemented, only reused. The files listed
below are redistributed under the terms they were originally published under.
Those terms are permissive and impose no conditions on the Rust code, but they
do require attribution, and this file provides it.

If you redistribute this crate, or a binary built from it, keep this file and
`NOTICE` with it.

---

## `data/cmu_us_kal.dat`: the voice

**Contents:** the `cmu_us_kal` diphone database, holding a diphone index, quantised
LPC coefficient frames, and ADPCM-compressed excitation residuals, recorded
from a single speaker (Kevin A. Lenzo) at 8 kHz.

**Origin:** the Flite speech synthesis system, Language Technologies Institute,
Carnegie Mellon University.

**License:** see [CMU license](#cmu-license) below.

## `data/en_us.dat`: the language models

**Contents and origin:**

| Section | What it is | Copyright |
|---|---|---|
| `lex.*` | Pronunciation dictionary, derived from CMULEX / the CMU Pronouncing Dictionary and pruned to letter-to-sound exceptions | Carnegie Mellon University |
| `lts.*` | Letter-to-sound decision graphs, built with the Lenzo and Black technique | Carnegie Mellon University |
| `cart.dur` | Segment duration model | CMU and the University of Edinburgh |
| `cart.accent`, `cart.tone` | Pitch accent and boundary tone models | CMU and the University of Edinburgh |
| `cart.pos`, `cart.phrasing`, `cart.nums` | Part-of-speech, phrase break and number-reading models | Carnegie Mellon University |
| `aswd.*` | Finite-state machines deciding whether a letter string is pronounceable | Carnegie Mellon University |

The duration and intonation models were trained from the **Boston University FM
Radio Data Corpus** and reached this project by way of the Festival Speech
Synthesis System; hence the joint CMU/Edinburgh copyright on those sections.

**License:** see [CMU license](#cmu-license) below. The jointly held sections
carry the same terms, with the University of Edinburgh as an additional
copyright holder.

## Tables embedded in the Rust source

Some data is small enough to be more readable as source than as a binary blob.
These tables are data, not code, and carry the same terms as the sections
above:

- `src/phoneset.rs`: the US English phone inventory and its distinctive
  features (CMU and the University of Edinburgh, via Festival).
- `src/pipeline.rs`: `F0_TERMS`, the F0 linear-regression coefficients (CMU
  and the University of Edinburgh, via Festival, trained from the Boston
  University FM Radio Data Corpus); `DURATION_STATS`, per-phone duration
  statistics measured from the `cmu_us_kal` recordings (CMU).
- `src/lang.rs`, `src/lexicon.rs`, `src/normalize.rs`, `src/patterns.rs`:
  word lists, function words, month and day names, US state abbreviations,
  unit abbreviations, syllable onset clusters, and dictionary addenda (CMU).

## Modifications made

The CMU license asks that modifications be marked clearly. They are:

1. **Repackaging.** The data was converted from C source arrays into the binary
   container documented in `src/data.rs`, by `tools/gen_data.py`. The values are
   unchanged; only their storage format differs. The lexicon was additionally
   decompressed from its Huffman-coded form and re-serialised as a sorted table
   with an offset index, so that lookup needs no decoding step.
2. **Corrections.** Four US state names were misspelled in the source table
   (`minnestota`, `carlolina`, `montanna`, `tennesee`); they are spelled
   correctly in `src/normalize.rs`.

No model was retrained, re-estimated, or otherwise altered in value.

---

## CMU license

The following notice applies to the data files described above.

```
                  Language Technologies Institute
                     Carnegie Mellon University
                      Copyright (c) 1999-2017
                        All Rights Reserved.

  Permission is hereby granted, free of charge, to use and distribute
  this software and its documentation without restriction, including
  without limitation the rights to use, copy, modify, merge, publish,
  distribute, sublicense, and/or sell copies of this work, and to
  permit persons to whom this work is furnished to do so, subject to
  the following conditions:
   1. The code must retain the above copyright notice, this list of
      conditions and the following disclaimer.
   2. Any modifications must be clearly marked as such.
   3. Original authors' names are not deleted.
   4. The authors' names are not used to endorse or promote products
      derived from this software without specific prior written
      permission.

  CARNEGIE MELLON UNIVERSITY AND THE CONTRIBUTORS TO THIS WORK
  DISCLAIM ALL WARRANTIES WITH REGARD TO THIS SOFTWARE, INCLUDING
  ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS, IN NO EVENT
  SHALL CARNEGIE MELLON UNIVERSITY NOR THE CONTRIBUTORS BE LIABLE
  FOR ANY SPECIAL, INDIRECT OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
  WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN
  AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
  ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF
  THIS SOFTWARE.
```

For the sections marked as jointly held, the same notice applies with the
University of Edinburgh named alongside Carnegie Mellon University as a
copyright holder and as an additional beneficiary of the disclaimer.

## Algorithms implemented from published specifications

These are noted for provenance; they impose no license conditions.

- **G.711 µ-law companding** and **G.721 32 kbit/s ADPCM** (`src/dsp.rs`) are
  ITU-T recommendations, implemented here from their published algorithm
  descriptions.
- **RIFF/WAVE** (`src/wav.rs`) is a published container format.

## Acknowledgements

The techniques this crate implements were developed by Alan W Black, Kevin A.
Lenzo, and colleagues at Carnegie Mellon University's Language Technologies
Institute, building on the Festival Speech Synthesis System from the Centre for
Speech Technology Research at the University of Edinburgh. This project exists
because they published both the methods and the data.

Neither Carnegie Mellon University nor the University of Edinburgh endorses
this project.
