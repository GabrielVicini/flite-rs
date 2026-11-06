// The customiser.
//
// Everything the page renders comes out of one object, so editing the page is
// editing that object. The controls below are generated from it; each one
// writes a path and hands the whole config back, which is also exactly what
// gets copied out as a replacement `site.config.json`.

import { THEMES, FONTS, shareLink } from './config.js';

const ACCENTS = [
  '',
  '#a8341a',
  '#b45309',
  '#1d4e4a',
  '#1e3a8a',
  '#6d28d9',
  '#9d174d',
  '#3f3f46',
];

/** Read a dotted path. */
function read(object, path) {
  return path.split('.').reduce((value, key) => (value == null ? value : value[key]), object);
}

/** Write a dotted path into a clone, leaving the original alone. */
function write(object, path, value) {
  const next = structuredClone(object);
  const keys = path.split('.');
  let cursor = next;
  for (const key of keys.slice(0, -1)) {
    cursor[key] ??= {};
    cursor = cursor[key];
  }
  cursor[keys.at(-1)] = value;
  return next;
}

export function mountCustomiser(host) {
  const { root, drawer } = host;

  for (const button of drawer.querySelectorAll('[data-drawer-close]')) {
    button.addEventListener('click', () => {
      drawer.hidden = true;
    });
  }
  document.addEventListener('keydown', (event) => {
    if (event.key === 'Escape' && !drawer.hidden) drawer.hidden = true;
  });

  const set = (path, value) => {
    host.onChange(write(host.config, path, value));
    render();
  };

  // --- control builders ---------------------------------------------------

  function field(label, control) {
    const wrapper = document.createElement('label');
    wrapper.className = 'field';
    wrapper.append(Object.assign(document.createElement('span'), { textContent: label }));
    wrapper.append(control);
    return wrapper;
  }

  function text(label, path, { multiline = false } = {}) {
    const input = document.createElement(multiline ? 'textarea' : 'input');
    if (!multiline) input.type = 'text';
    input.value = read(host.config, path) ?? '';
    // `change`, not `input`: re-rendering on every keystroke would move the
    // caret, and nothing here is worth live-updating mid-word.
    input.addEventListener('change', () => set(path, input.value));
    return field(label, input);
  }

  function choose(label, path, options) {
    const select = document.createElement('select');
    for (const option of options) {
      const value = typeof option === 'string' ? option : option.value;
      const name = typeof option === 'string' ? option : option.label;
      select.append(new Option(name, value, false, read(host.config, path) === value));
    }
    select.addEventListener('change', () => set(path, select.value));
    return field(label, select);
  }

  function toggle(label, path) {
    const wrapper = document.createElement('label');
    wrapper.className = 'field field--inline';
    wrapper.append(Object.assign(document.createElement('span'), { textContent: label }));
    const input = document.createElement('input');
    input.type = 'checkbox';
    input.checked = Boolean(read(host.config, path));
    input.addEventListener('change', () => set(path, input.checked));
    wrapper.append(input);
    return wrapper;
  }

  function slider(label, path, { min, max, step }) {
    const input = document.createElement('input');
    Object.assign(input, { type: 'range', min, max, step, value: read(host.config, path) ?? 1 });
    input.addEventListener('change', () => set(path, Number(input.value)));
    return field(label, input);
  }

  function group(title, ...children) {
    const section = document.createElement('section');
    section.className = 'group';
    section.append(Object.assign(document.createElement('h3'), { textContent: title }));
    section.append(...children.flat());
    return section;
  }

  // --- the panel ----------------------------------------------------------

  function themePresets() {
    const row = document.createElement('div');
    row.className = 'presets';
    for (const [id, theme] of Object.entries(THEMES)) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'preset';
      button.setAttribute('aria-pressed', String(host.config.theme.preset === id));
      const chip = document.createElement('i');
      chip.style.background = theme.tokens['--bg'];
      chip.style.borderColor = theme.tokens['--accent'];
      button.append(chip, theme.label);
      button.addEventListener('click', () => set('theme.preset', id));
      row.append(button);
    }
    return field('Palette', row);
  }

  function accentSwatches() {
    const row = document.createElement('div');
    row.className = 'swatches';
    for (const colour of ACCENTS) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'swatch';
      button.title = colour || "the palette's own accent";
      button.setAttribute('aria-pressed', String((host.config.theme.accent ?? '') === colour));
      button.style.background = colour || 'var(--accent)';
      if (!colour) button.style.backgroundImage = 'linear-gradient(135deg, var(--accent) 50%, var(--rule) 50%)';
      button.addEventListener('click', () => set('theme.accent', colour));
      row.append(button);
    }

    const custom = document.createElement('input');
    custom.type = 'color';
    custom.className = 'swatch';
    custom.value = host.config.theme.accent || '#a8341a';
    custom.addEventListener('change', () => set('theme.accent', custom.value));
    row.append(custom);

    return field('Accent', row);
  }

  function engineRows() {
    const rows = [];
    (host.config.engines ?? []).forEach((engine, index) => {
      rows.push(text(`Engine ${index + 1} name`, `engines.${index}.label`));
      rows.push(toggle(`Show "${engine.label}"`, `engines.${index}.enabled`));
    });
    const shown = (host.config.engines ?? []).filter((e) => e.enabled !== false);
    rows.push(
      choose(
        'Selected on load',
        'defaultEngine',
        shown.map((e) => ({ value: e.id, label: e.label })),
      ),
    );
    return rows;
  }

  function sampleRows() {
    return (host.config.samples ?? []).flatMap((sample, index) => [
      text(`Sample ${index + 1} name`, `samples.${index}.label`),
      text(`Sample ${index + 1} text`, `samples.${index}.text`, { multiline: true }),
    ]);
  }

  function render() {
    root.replaceChildren(
      group(
        'Identity',
        text('Wordmark', 'brand.name'),
        text('Line above it', 'brand.eyebrow'),
        text('Standfirst', 'brand.standfirst', { multiline: true }),
        text('Note under the masthead', 'brand.note', { multiline: true }),
        text('Footer', 'brand.footer', { multiline: true }),
      ),
      group(
        'Appearance',
        themePresets(),
        accentSwatches(),
        choose('Display face', 'theme.display', FONTS.display),
        choose('Body face', 'theme.body', FONTS.body),
        choose('Data face', 'theme.mono', FONTS.mono),
        slider('Wordmark size', 'theme.wordmarkScale', { min: 0.5, max: 1.6, step: 0.05 }),
        toggle('Paper grain', 'theme.grain'),
      ),
      group('Engines', ...engineRows()),
      group(
        'Sections',
        toggle('Waveform', 'panels.waveform'),
        toggle('Phone track', 'panels.phones'),
        toggle('Readout', 'panels.readout'),
        toggle('Facts', 'panels.facts'),
        toggle('Closing note', 'panels.essay'),
      ),
      group(
        'Wording',
        ...Object.keys(host.config.labels ?? {}).map((key) => text(key, `labels.${key}`)),
      ),
      group('Samples', ...sampleRows()),
    );
  }

  // --- export -------------------------------------------------------------

  async function copy(value, button) {
    const original = button.textContent;
    try {
      await navigator.clipboard.writeText(value);
      button.textContent = 'Copied';
    } catch {
      // Clipboard access can be refused; a prompt still lets them take it.
      window.prompt('Copy this:', value);
      button.textContent = original;
      return;
    }
    setTimeout(() => {
      button.textContent = original;
    }, 1400);
  }

  drawer.querySelector('[data-copy-config]').addEventListener('click', (event) => {
    const { $comment, ...rest } = host.config;
    copy(JSON.stringify(rest, null, 2), event.currentTarget);
  });

  drawer.querySelector('[data-copy-link]').addEventListener('click', (event) => {
    copy(shareLink(host.base, host.config), event.currentTarget);
  });

  drawer.querySelector('[data-reset-config]').addEventListener('click', host.onReset);

  render();
}
