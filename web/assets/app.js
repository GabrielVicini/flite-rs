// Wiring. The config says what the page contains; this puts it on screen and
// connects it to the engine.

import * as site from './config.js';
import * as wasm from './engine.js';
import { Scope, drawRuler, formatTime } from './waveform.js';
import { mountCustomiser } from './customiser.js';

const $ = (selector) => document.querySelector(selector);

/** Where the build left the modules, if `build.json` is missing (local dev). */
const FALLBACK_BUILD = {
  version: '',
  modules: {
    core: { file: 'wasm/core.wasm', bytes: 0 },
    kal16: { file: 'wasm/kal16.wasm', bytes: 0 },
    clustergen: { file: 'wasm/clustergen.wasm', bytes: 0 },
  },
};

const ui = {
  scope: null,
  audio: null,
  /** @type {AudioBufferSourceNode|null} */
  source: null,
  /** The most recent render, kept so it can be replayed and saved. */
  render: null,
  phones: [],
  /** @type {Map<string, object>} engine id -> loaded Synth */
  engines: new Map(),
  current: null,
  frame: 0,
};

let config;
let base;
let build;

// --- startup --------------------------------------------------------------

async function start() {
  ({ base, config } = await site.load());
  build = await loadBuild();

  site.applyTheme(config.theme);
  ui.scope = new Scope($('[data-waveform]'));

  applyConfig();
  bindControls();

  mountCustomiser({
    root: $('[data-customiser]'),
    drawer: $('[data-drawer]'),
    get config() {
      return config;
    },
    get base() {
      return base;
    },
    onChange(next) {
      config = next;
      site.persist(base, config);
      site.applyTheme(config.theme);
      applyConfig();
      ui.scope.draw();
    },
    onReset() {
      site.clearStored();
      location.reload();
    },
  });

  await selectEngine(pickDefaultEngine(), { silent: true });
}

async function loadBuild() {
  try {
    const response = await fetch('build.json', { cache: 'no-cache' });
    if (response.ok) return { ...FALLBACK_BUILD, ...(await response.json()) };
  } catch {
    /* Running from a checkout without the generator. */
  }
  return FALLBACK_BUILD;
}

// --- rendering the config -------------------------------------------------

/** Engines whose module this build actually produced. */
function availableEngines() {
  return (config.engines ?? []).filter((e) => e.enabled !== false && build.modules[e.module]);
}

function pickDefaultEngine() {
  const engines = availableEngines();
  return engines.find((e) => e.id === config.defaultEngine) ?? engines[0] ?? null;
}

function applyConfig() {
  document.title = config.brand.name || 'flite-rs';

  for (const node of document.querySelectorAll('[data-copy]')) {
    const value = read(config, node.dataset.copy);
    node.textContent = value ?? '';
    node.hidden = !value;
  }
  for (const node of document.querySelectorAll('[data-copy-html]')) {
    // The only HTML here is the About block, which comes from the repository's
    // own config file — the same trust as the markup around it. A visitor's
    // overrides are rendered as text everywhere else.
    node.innerHTML = read(config, node.dataset.copyHtml) ?? '';
  }

  renderLinks();
  renderSamples();
  renderEngines();
  renderBuildLines();

  $('[data-phones-wrap]').hidden = !config.panels.phones || !ui.phones.length;
  $('[data-readout]').hidden = !config.panels.readout;
  $('[data-facts]').hidden = !config.panels.facts;
  $('.essay').hidden = !config.panels.essay;
  $('.scope').hidden = !config.panels.waveform;
  $('.ruler').hidden = !config.panels.waveform;
}

function read(object, path) {
  return path.split('.').reduce((value, key) => (value == null ? value : value[key]), object);
}

function renderLinks() {
  const nav = $('[data-links]');
  nav.replaceChildren();
  for (const link of config.links ?? []) {
    if (!link.label || !/^https?:\/\//i.test(link.href ?? '')) continue;
    const anchor = document.createElement('a');
    anchor.href = link.href;
    anchor.textContent = link.label;
    anchor.rel = 'noopener';
    nav.append(anchor);
  }
}

function renderSamples() {
  const host = $('[data-samples]');
  host.replaceChildren();
  for (const sample of config.samples ?? []) {
    const button = document.createElement('button');
    button.type = 'button';
    button.textContent = sample.label;
    button.addEventListener('click', () => {
      $('[data-text]').value = sample.text;
      $('[data-text]').focus();
    });
    host.append(button);
  }
  const text = $('[data-text]');
  if (!text.value) text.value = config.samples?.[0]?.text ?? '';
}

function renderEngines() {
  const host = $('[data-engines]');
  host.replaceChildren();

  for (const engine of availableEngines()) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'engine';
    button.role = 'radio';
    button.setAttribute('aria-checked', String(ui.current?.id === engine.id));

    const label = document.createElement('span');
    label.textContent = engine.label;
    const tag = document.createElement('span');
    tag.className = 'engine__tag';
    tag.textContent = sizeOf(engine);
    button.append(label, tag);

    button.addEventListener('click', () => selectEngine(engine));
    host.append(button);
  }

  $('[data-engine-note]').textContent = ui.current?.note ?? '';
}

/** What the engine button says on its right: a download size, or "ready". */
function sizeOf(engine) {
  if (wasm.isLoaded(engine.module)) return 'ready';
  return megabytes(build.modules[engine.module]?.bytes);
}

function megabytes(bytes) {
  return bytes ? `${(bytes / 1024 / 1024).toFixed(1)} MB` : '—';
}

function renderBuildLines() {
  const parts = [];
  if (build.version) parts.push(`v${build.version}`);
  if (build.commit) parts.push(build.commit.slice(0, 7));
  if (build.built) parts.push(new Date(build.built).toISOString().slice(0, 10));

  const line = $('[data-build-line]');
  line.textContent = parts.join(' · ');
  line.hidden = !parts.length;

  const footer = $('[data-footer-build]');
  footer.textContent = build.rustc ? `built with rustc ${build.rustc}` : '';

  const meta = $('[data-about-meta]');
  meta.replaceChildren();
  const rows = [
    ['engine', ui.current?.label ?? '—'],
    ['backend', ui.current ? backendOf(ui.current) : '—'],
    ['module', megabytes(build.modules[ui.current?.module]?.bytes)],
    ['version', build.version ? `v${build.version}` : '—'],
  ];
  for (const [name, value] of rows) {
    const row = document.createElement('span');
    row.append(`${name} `, Object.assign(document.createElement('b'), { textContent: value }));
    meta.append(row);
  }
}

function backendOf(engine) {
  const synth = ui.engines.get(engine.id);
  const voice = synth?.info.voices.find((v) => v.id === engine.voice);
  return voice?.backend ?? '—';
}

// --- engines --------------------------------------------------------------

async function selectEngine(engine, { silent = false } = {}) {
  if (!engine) {
    status('No engine module was built for this page.', 'error');
    return;
  }
  ui.current = engine;
  renderEngines();

  try {
    const synth = await ensureLoaded(engine, silent);
    synth.selectVoice(engine.voice);
    synth.setRate(Number($('[data-rate]').value));
    synth.setPitch(Number($('[data-pitch]').value));
    renderEngines();
    renderBuildLines();
    renderFacts(synth);
    if (!silent) status('');
  } catch (error) {
    status(String(error.message ?? error), 'error');
  }
}

async function ensureLoaded(engine, silent) {
  const existing = ui.engines.get(engine.id);
  if (existing) return existing;

  const module = { name: engine.module, ...build.modules[engine.module] };
  const total = module.bytes || 0;
  if (!silent || total > 2_500_000) {
    status(`Fetching ${engine.label}…`, null, { progress: 0 });
  }

  const synth = await wasm.load(module, (fraction, received) => {
    const value = fraction ?? (total ? Math.min(received / total, 1) : null);
    status(`Fetching ${engine.label}…`, null, { progress: value });
  });

  ui.engines.set(engine.id, synth);
  status('');
  return synth;
}

function renderFacts(synth) {
  const facts = $('[data-facts]');
  facts.replaceChildren();
  const voice = synth.info.voices.find((v) => v.id === ui.current.voice) ?? synth.info.voices[0];
  const rows = [
    ['voice', voice?.id ?? '—'],
    ['backend', voice?.backend ?? '—'],
    ['sample rate', `${(synth.sampleRate / 1000).toFixed(0)} kHz`],
  ];
  if (ui.render) {
    rows.push(['last render', `${ui.render.elapsed.toFixed(1)} ms`]);
  }
  for (const [name, value] of rows) {
    facts.append(
      Object.assign(document.createElement('dt'), { textContent: name }),
      Object.assign(document.createElement('dd'), { textContent: value }),
    );
  }
}

// --- speaking -------------------------------------------------------------

async function speak() {
  const text = $('[data-text]').value.trim();
  if (!text) {
    $('[data-text]').focus();
    return;
  }

  const button = $('[data-speak]');
  button.classList.add('is-busy');
  button.disabled = true;

  try {
    const synth = await ensureLoaded(ui.current, false);
    synth.selectVoice(ui.current.voice);
    synth.setRate(Number($('[data-rate]').value));
    synth.setPitch(Number($('[data-pitch]').value));

    // Let the busy state paint before a call that blocks the thread.
    await new Promise((resolve) => requestAnimationFrame(() => resolve()));

    const render = synth.speak(text);
    if (!render.samples.length) {
      status('Nothing in that to say.', 'error');
      return;
    }
    ui.render = render;
    ui.phones = config.panels.phones ? synth.phones(text) : [];

    showRender(render);
    renderFacts(synth);
    play(render);
  } catch (error) {
    status(String(error.message ?? error), 'error');
  } finally {
    button.classList.remove('is-busy');
    button.disabled = false;
  }
}

function showRender(render) {
  const duration = render.samples.length / render.sampleRate;
  const scope = $('.scope');
  scope.classList.add('is-live');
  ui.scope.show(render.samples);
  drawRuler($('[data-ruler]'), duration, scope.clientWidth);

  const readout = $('[data-readout]');
  readout.replaceChildren();
  const speed = render.elapsed > 0 ? (duration * 1000) / render.elapsed : 0;
  const pairs = [
    [formatTime(duration), 'of audio'],
    [`${render.elapsed.toFixed(1)} ms`, 'to render'],
    [`${render.samples.length.toLocaleString()}`, 'samples'],
  ];
  if (speed >= 2) pairs.push([`${Math.round(speed)}×`, 'faster than real time']);
  for (const [value, label] of pairs) {
    const span = document.createElement('span');
    span.append(Object.assign(document.createElement('b'), { textContent: value }), ` ${label}`);
    readout.append(span);
  }

  renderPhones();
  $('[data-download]').disabled = false;
}

function renderPhones() {
  const wrap = $('[data-phones-wrap]');
  const track = $('[data-phones]');
  track.replaceChildren();
  wrap.hidden = !config.panels.phones || !ui.phones.length;
  if (wrap.hidden) return;

  for (const phone of ui.phones) {
    const chip = document.createElement('span');
    chip.className = 'phone';
    chip.textContent = phone.p;
    // `pau` is Flite's silence; it reads better as a gap than as a label.
    if (phone.p === 'pau' || phone.p === 'h#') chip.dataset.pause = '';
    track.append(chip);
  }
}

function play(render) {
  stop();
  ui.audio ??= new (window.AudioContext ?? window.webkitAudioContext)();
  const context = ui.audio;
  if (context.state === 'suspended') context.resume();

  const buffer = context.createBuffer(1, render.samples.length, render.sampleRate);
  const channel = buffer.getChannelData(0);
  for (let i = 0; i < render.samples.length; i += 1) channel[i] = render.samples[i] / 32768;

  const source = context.createBufferSource();
  source.buffer = buffer;
  source.connect(context.destination);

  const startedAt = context.currentTime;
  const duration = buffer.duration;
  source.start();
  ui.source = source;
  source.onended = () => {
    if (ui.source === source) stop();
  };

  $('[data-stop]').disabled = false;
  const playhead = $('[data-playhead]');
  playhead.hidden = false;

  const chips = [...$('[data-phones]').children];
  const follow = () => {
    const elapsed = context.currentTime - startedAt;
    const fraction = Math.min(elapsed / duration, 1);
    playhead.style.left = `${fraction * 100}%`;

    if (chips.length) {
      // Phone ends are cumulative, so the current phone is the first whose end
      // is still ahead of the playhead.
      let index = ui.phones.findIndex((p) => p.t > elapsed);
      if (index < 0) index = chips.length - 1;
      // Voices that predict duration inside the backend report no ends at all;
      // without a time axis, highlighting would be a lie, so it is skipped.
      const timed = ui.phones.at(-1)?.t > 0;
      chips.forEach((chip, i) => {
        chip.classList.toggle('is-now', timed && i === index);
        chip.classList.toggle('is-done', timed && i < index);
      });
    }

    if (fraction < 1) ui.frame = requestAnimationFrame(follow);
  };
  ui.frame = requestAnimationFrame(follow);
}

function stop() {
  cancelAnimationFrame(ui.frame);
  if (ui.source) {
    ui.source.onended = null;
    try {
      ui.source.stop();
    } catch {
      /* Already finished. */
    }
    ui.source = null;
  }
  $('[data-stop]').disabled = true;
  $('[data-playhead]').hidden = true;
  for (const chip of $('[data-phones]').children) {
    chip.classList.remove('is-now', 'is-done');
  }
}

function status(message, tone = null, { progress } = {}) {
  const node = $('[data-status]');
  node.replaceChildren();
  if (tone) node.dataset.tone = tone;
  else delete node.dataset.tone;
  if (!message) return;

  node.append(message);
  if (progress !== undefined) {
    const bar = document.createElement('span');
    bar.className = 'status__bar';
    const fill = document.createElement('i');
    if (progress === null) bar.dataset.indeterminate = '';
    else fill.style.width = `${Math.round(progress * 100)}%`;
    bar.append(fill);
    node.append(bar);
  }
}

// --- controls -------------------------------------------------------------

function bindControls() {
  $('[data-speak]').addEventListener('click', speak);
  $('[data-stop]').addEventListener('click', stop);

  $('[data-download]').addEventListener('click', () => {
    if (!ui.render) return;
    const blob = wasm.toWav(ui.render.samples, ui.render.sampleRate);
    const url = URL.createObjectURL(blob);
    const anchor = Object.assign(document.createElement('a'), {
      href: url,
      download: `${(config.brand.name || 'flite').replace(/\W+/g, '-').toLowerCase()}.wav`,
    });
    anchor.click();
    URL.revokeObjectURL(url);
  });

  for (const [input, output, apply] of [
    ['[data-rate]', '[data-rate-out]', (s, v) => s.setRate(v)],
    ['[data-pitch]', '[data-pitch-out]', (s, v) => s.setPitch(v)],
  ]) {
    const node = $(input);
    node.addEventListener('input', () => {
      const value = Number(node.value);
      $(output).textContent = value.toFixed(2);
      const synth = ui.engines.get(ui.current?.id);
      if (synth) apply(synth, value);
    });
  }

  $('[data-text]').addEventListener('keydown', (event) => {
    if ((event.ctrlKey || event.metaKey) && event.key === 'Enter') {
      event.preventDefault();
      speak();
    }
  });

  $('[data-open-customiser]').addEventListener('click', () => {
    $('[data-drawer]').hidden = false;
  });

  window.addEventListener('resize', () => {
    if (ui.render) {
      const duration = ui.render.samples.length / ui.render.sampleRate;
      drawRuler($('[data-ruler]'), duration, $('.scope').clientWidth);
    }
  });
}

start().catch((error) => {
  document.body.insertAdjacentHTML(
    'afterbegin',
    `<p style="padding:2rem;font-family:monospace">The page could not start: ${String(
      error.message ?? error,
    ).replace(/[<&]/g, '')}</p>`,
  );
});
