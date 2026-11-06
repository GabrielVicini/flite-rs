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

| Module       | Features      | Voices       | Size   |
| ------------ | ------------- | ------------ | ------ |
| `core`       | —             | `kal`        | ~2.5 MB |
| `kal16`      | `kal16`       | `kal16`, `kal` | ~6.3 MB |
| `clustergen` | `clustergen`  | `slt`, `kal` | ~4.9 MB |

The page fetches a module the first time its engine is picked and keeps it
afterwards. `build.json` records each module's size and content hash, and the
engine list hides any engine whose module a build left out — so a build made
with `--only core` produces a page offering one engine, with no edit to the
markup.

## Changing what it says

Everything the page renders comes from `site.config.json`: the wordmark, every
label, the sample sentences, which panels appear, which engines are offered and
what they are called, and the palette and typefaces. Nothing is baked into
`index.html`.

There are two ways to change it:

- **Edit the file.** The shape is documented by its own contents; the page deep
  merges it over a fallback, so a partial file is fine.
- **Use the page.** "Customise this page" opens a drawer over the same object.
  Edits apply live and are kept in `localStorage`; "Copy site.config.json"
  hands back a replacement for this file, and "Copy share link" encodes the
  changes into the URL so someone else opens your version rather than theirs.

A visitor's overrides never reach anyone else's page, and they are stored as a
diff, so a wording change in the repository still shows through to people who
only changed the palette.

### Palettes and faces

`theme.preset` picks one of `paper`, `night`, `terminal` or `blueprint`, each a
complete set of tokens rather than a tint of the others. `theme.accent` either
names a colour or is empty, which means "whatever the preset says" — that is
what keeps a preset a design and not a background colour. The three font slots
accept only the faces listed in `assets/config.js`, which is also the list the
font request is built from.

## Files

| File                   | What it is                                          |
| ---------------------- | --------------------------------------------------- |
| `index.html`           | Structure, with `data-copy` hooks the config fills   |
| `site.config.json`     | Everything the page says and most of how it looks    |
| `assets/config.js`     | Palettes, faces, merging, sharing, persistence       |
| `assets/engine.js`     | The wasm calling convention and WAV writing          |
| `assets/waveform.js`   | Canvas drawing and the time ruler                    |
| `assets/customiser.js` | The drawer, generated from the config                |
| `assets/app.js`        | Wiring                                               |
| `assets/style.css`     | The look, entirely in custom properties              |
