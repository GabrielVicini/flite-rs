# Where the Clustergen voice data comes from

This is the decision asked for in step 0 of the Clustergen work, written down
so that someone else can reproduce the result from this repository alone.

## The question

The diphone voices are built from the upstream Flite source tree by
`tools/gen_data.py`. Clustergen voices were expected to be different: upstream
publishes CG voices as standalone `.flitevox` files, which would have meant
either downloading one or teaching the loader to parse that format.

## What is actually in the upstream tree

That expectation is wrong, and checking it changed the plan. The pinned
upstream revision — `6c9f20dc915b17f5619340069889db0aa007fcdc`, the same one
`.github/workflows/ci.yml` pins for the reference build — ships **three
complete Clustergen voices in-tree**, as generated C arrays, exactly like the
diphone data:

| voice         | directory               | rate     | frames | channels |
| ------------- | ----------------------- | -------- | ------ | -------- |
| `cmu_us_awb`  | `lang/cmu_us_awb`       | 16 kHz   | 8855   | 114      |
| `cmu_us_rms`  | `lang/cmu_us_rms`       | 16 kHz   | 10446  | 114      |
| `cmu_us_slt`  | `lang/cmu_us_slt`       | 16 kHz   | 8873   | 114      |

Each directory holds, per voice:

```
cmu_us_slt_cg.c                       the cst_cg_db struct: rates, flags,
                                      mlsa_alpha/beta, gain, model_min/range,
                                      the five mixed-excitation filters
cmu_us_slt_cg_single_params.c         8873 x 114 unsigned shorts (8.0 MB of C)
cmu_us_slt_cg_single_mcep_trees.c/.h  124 CART trees, one per state type
cmu_us_slt_cg_f0_trees.c/.h           124 CART trees, one per state type
cmu_us_slt_cg_durmodel.c/.h           one duration CART plus per-state
                                      mean/stddev duration statistics
cmu_us_slt_cg_phonestate.c            phone -> HMM state-name map
```

`.flitevox` is the *dump* format for the same `cst_cg_db` structure — see
`src/cg/cst_cg_dump_voice.c` and `cst_cg_load_voice.c` upstream. It carries no
information the in-tree C arrays do not.

## The decision

**Convert the in-tree C arrays into the `FLRSDAT` container with
`tools/gen_data.py`, exactly as the diphone voices already are.** No
`.flitevox` parsing, no download, no new file format.

`cmu_us_slt` is the voice converted. It is the only one of the three with both
`spamf0 = 0` and a `spamf0_accent_tree` present but unused, so it exercises the
whole Clustergen path without also requiring the SPAM F0 model, which is a
separate model with its own trees and is not part of this work. (`cmu_us_awb`
would do equally well; `cmu_us_rms` sets `spamf0 = 1` and would not.)

### Why this and not runtime `.flitevox`

* It is continuous with what exists. `gen_data.py` already parses upstream's
  CART tables, string tables and C arrays; the CG trees use byte-for-byte the
  same `DEF_STATIC_CONST_VAL_*` / `CTNODE_*` / `_cart_nodes` / `_feat_table`
  shape that `convert_cart` already reads.
* It keeps one container format, so the loader has one parser, and the backend
  is named in the data for both kinds of voice (see `src/data.rs`).
* Reproducibility is the same property the diphone voices already have: the
  input is a pinned upstream revision, and the tool that converts it is in the
  repository.

Runtime loading of an external voice file is still *possible* — that is the
whole reason `VoiceData` is a borrowed/owned split rather than `&'static [u8]`
(see below) — but it is not how the bundled CG voice is obtained.

## How to obtain it

```bash
git init upstream-flite && cd upstream-flite
git remote add origin https://github.com/festvox/flite.git
git fetch --depth 1 origin 6c9f20dc915b17f5619340069889db0aa007fcdc
git checkout FETCH_HEAD && cd ..

python tools/gen_data.py upstream-flite data
```

That rewrites `data/en_us.dat`, `data/cmu_us_kal.dat`, `data/cmu_us_kal16.dat`
and `data/cmu_us_slt.dat`. The CG voice is behind the `clustergen` feature, so
a build that does not ask for it does not embed those bytes.

## Licence

The CG data carries the same CMU terms as the rest of the Flite data already
converted by `gen_data.py`; `src/cg` and `lang/cmu_us_*` are under the same
permissive CMU licence quoted in `THIRD-PARTY-LICENSES.md`. The MLSA and MLPG
code those directories contain additionally credits the Nagoya Institute of
Technology (HTS) and Tomoki Toda; `THIRD-PARTY-LICENSES.md` records that,
because the *algorithms* reimplemented here come from that lineage even though
none of its code does.

## Data ownership

Decided here rather than later, because it is structural (decision 4 of the
brief):

`DiphoneVoice` and `ClustergenVoice` both hold their bytes as
`data::VoiceData`, a two-case borrowed/owned split:

```rust
pub enum VoiceData {
    Static(&'static [u8]),   // include_bytes!, no allocation
    Shared(Arc<[u8]>),       // read at runtime, shared cheaply
}
```

`Deref<Target = [u8]>` makes it transparent at every use site. An embedded
voice costs no allocation and no copy at startup, which a bare `Arc<[u8]>`
could not give; a runtime-loaded voice is possible without touching either
backend. Both backends are built on this one choice, so adding a third needs no
further decision.
