# The demo site

A single page that runs the synthesiser in the browser. It is generated, not
committed: `tools/build_site.py` compiles the engine to WebAssembly, copies the
files in this directory next to the modules, and writes the manifest the page
reads to learn what the build contains.

```bash
python tools/build_site.py --out _site
python -m http.server 8000 -d _site
```

`--skip-build` reuses modules already staged under `wasm/target`, which is what
you want while editing the page itself. `--only core` builds just the small
module, which is enough to see everything except the other engines.

GitHub Actions runs the same script on every push to `main`
(`.github/workflows/pages.yml`) and publishes the result. Publishing needs
Pages set to deploy from GitHub Actions once, in the repository's settings.

A custom domain is passed in rather than committed — `--cname flite-rs.example`
— because a `CNAME` for a domain whose DNS is not pointed at Pages takes the
site off the air at both addresses. Setting the domain in the repository's
settings instead has the same effect and survives a deploy.

## How it talks to the engine

There is no `wasm-bindgen` and no JavaScript toolchain anywhere in the build.
`wasm/` is a small `cdylib` that exposes a dozen `extern "C"` functions; the
page instantiates the `.wasm` file itself and reads results straight out of
linear memory. Adding a function to the ABI means editing `wasm/src/lib.rs` and
`assets/engine.js`, and nothing else.

One module is built per voice set, because a voice is almost entirely data and
there is no reason to make a visitor download the 16 kHz diphone database to
hear the 8 kHz one:

| Module       | Features     | Voices         | Size    |
| ------------ | ------------ | -------------- | ------- |
| `core`       | —            | `kal`          | ~2.5 MB |
| `kal16`      | `kal16`      | `kal16`, `kal` | ~6.3 MB |
| `clustergen` | `clustergen` | `slt`, `kal`   | ~4.9 MB |

The page fetches a module the first time its engine is picked and keeps it
afterwards. `build.json` records each module's size and content hash, and the
engine list hides any engine whose module a build left out — so a build made
with `--only core` produces a page offering one engine, with no edit to the
markup.

## Changing what it says

Everything the page renders comes from `site.config.json`: the wordmark, every
label, the sample sentences, which engines are offered and what they are
called, and which of the three output panels appear. Nothing is baked into
`index.html`, which carries structure and `data-copy` hooks and no copy.

Edit the file. The shape is documented by its own contents; the page deep
merges it over a fallback, so a partial file is fine. There is no editor in the
page — this is a demo, not a theme builder.

`theme` carries the three font slots and an optional `accent`, which overrides
the one colour that reads as a brand. The faces must be ones listed in
`assets/config.js`, which is also the list the single font request is built
from. The palette itself is one set of tokens in that file and is not a choice:
the page is a printed record under lamplight, and that is the whole design.

## Layout

The page holds one screen and does not scroll. `body` is a three-row grid —
masthead, main, footer — and inside `main` the signal strip takes whatever
height is left over, so the waveform grows on a tall display rather than
leaving a gap. Below 820 px wide or 560 px tall the fixed height stops being a
feature, and the page goes back to scrolling.

Two details are load-bearing and easy to undo by accident. The canvas is
absolutely positioned, because a canvas carries an intrinsic size from its
`width`/`height` attributes and `draw` sets those from the box it is
measuring — in normal flow it feeds its own height back into the grid row that
sizes it. And every grid here names its column `minmax(0, 1fr)`, because an
implicit `auto` column sizes to max-content, and the phone track's max-content
is the entire transcription however much it scrolls.

## Files

| File                 | What it is                                        |
| -------------------- | ------------------------------------------------- |
| `index.html`         | Structure, with `data-copy` hooks the config fills |
| `site.config.json`   | Everything the page says                          |
| `assets/config.js`   | Palette, faces, merging the config over a fallback |
| `assets/engine.js`   | The wasm calling convention and WAV writing        |
| `assets/waveform.js` | Canvas drawing and the time ruler                  |
| `assets/app.js`      | Wiring                                             |
| `assets/style.css`   | The look, entirely in custom properties            |
