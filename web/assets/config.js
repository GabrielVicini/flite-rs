// The page's settings: what it says, what it looks like, and which engines it
// offers. `site.config.json` holds the repository's answer; a visitor's edits
// are kept as a diff against it, in the URL so a themed page can be shared and
// in localStorage so it survives a reload.

const STORAGE_KEY = 'flite-rs.site';
const HASH_KEY = 's';

/** Enough of a config to render something if `site.config.json` is missing. */
const FALLBACK = {
  brand: { name: 'flite-rs', eyebrow: '', standfirst: '', note: '', footer: '' },
  theme: {
    preset: 'paper',
    accent: '',
    display: 'Instrument Serif',
    body: 'Archivo',
    mono: 'DM Mono',
    grain: true,
    wordmarkScale: 1,
  },
  labels: {},
  engines: [],
  defaultEngine: '',
  samples: [],
  panels: { waveform: true, phones: true, readout: true, facts: true, essay: true },
  about: { body: '' },
  links: [],
};

// --- themes ---------------------------------------------------------------
//
// Each preset is a full set of tokens rather than a tweak of a base, so a
// preset can change the whole character of the page — including whether the
// body text is a serif or a mono — and not just its colours.

export const THEMES = {
  paper: {
    label: 'Paper',
    tokens: {
      '--bg': '#f3efe6',
      '--bg-deep': '#e9e3d6',
      '--surface': '#fbf9f4',
      '--ink': '#17150f',
      '--ink-soft': '#4a4438',
      '--ink-faint': '#8b8273',
      '--rule': '#d9d2c2',
      '--rule-strong': '#bdb4a0',
      '--accent': '#a8341a',
      '--accent-ink': '#fbf9f4',
      '--signal': '#1d4e4a',
      '--grain': '0.5',
      '--shadow': '0 1px 0 rgba(23, 21, 15, 0.06)',
    },
  },
  night: {
    label: 'Night lab',
    tokens: {
      '--bg': '#111010',
      '--bg-deep': '#0a0a09',
      '--surface': '#1a1917',
      '--ink': '#ece6d9',
      '--ink-soft': '#b3ab9b',
      '--ink-faint': '#797264',
      '--rule': '#2d2a26',
      '--rule-strong': '#423d36',
      '--accent': '#e2703a',
      '--accent-ink': '#120f0c',
      '--signal': '#5fb3a8',
      '--grain': '0.35',
      '--shadow': '0 1px 0 rgba(0, 0, 0, 0.5)',
    },
  },
  terminal: {
    label: 'Terminal',
    tokens: {
      '--bg': '#0b0a07',
      '--bg-deep': '#070603',
      '--surface': '#121008',
      '--ink': '#f2c14e',
      '--ink-soft': '#b98f33',
      '--ink-faint': '#7a5e22',
      '--rule': '#2a2110',
      '--rule-strong': '#473714',
      '--accent': '#ffb300',
      '--accent-ink': '#0b0a07',
      '--signal': '#ffb300',
      '--grain': '0.2',
      '--shadow': 'none',
    },
  },
  blueprint: {
    label: 'Blueprint',
    tokens: {
      '--bg': '#0d1b2a',
      '--bg-deep': '#081320',
      '--surface': '#122438',
      '--ink': '#e6edf5',
      '--ink-soft': '#a8bdd2',
      '--ink-faint': '#6c8299',
      '--rule': '#1e3650',
      '--rule-strong': '#2d4a68',
      '--accent': '#f5c518',
      '--accent-ink': '#0d1b2a',
      '--signal': '#5fc7e8',
      '--grain': '0.3',
      '--shadow': '0 1px 0 rgba(0, 0, 0, 0.35)',
    },
  },
};

// Only these are fetched, so the font link stays one request and nobody can
// talk the page into loading something arbitrary from a shared URL.
export const FONTS = {
  display: ['Instrument Serif', 'Fraunces', 'Newsreader', 'Archivo', 'DM Mono'],
  body: ['Archivo', 'Public Sans', 'Newsreader', 'DM Mono'],
  mono: ['DM Mono', 'IBM Plex Mono', 'Martian Mono'],
};

const FONT_SPECS = {
  'Instrument Serif': 'Instrument+Serif:ital@0;1',
  Fraunces: 'Fraunces:opsz,wght@9..144,300..700',
  Newsreader: 'Newsreader:ital,opsz,wght@0,6..72,300..600;1,6..72,300..500',
  Archivo: 'Archivo:wght@300..700',
  'Public Sans': 'Public+Sans:wght@300..700',
  'DM Mono': 'DM+Mono:ital,wght@0,300;0,400;0,500;1,400',
  'IBM Plex Mono': 'IBM+Plex+Mono:wght@300;400;500;600',
  'Martian Mono': 'Martian+Mono:wght@300..700',
};

const SERIF_FONTS = new Set(['Instrument Serif', 'Fraunces', 'Newsreader']);
const MONO_FONTS = new Set(Object.keys(FONT_SPECS).filter((n) => n.includes('Mono')));

function fallbackStack(name) {
  if (MONO_FONTS.has(name)) return 'ui-monospace, SFMono-Regular, Menlo, monospace';
  if (SERIF_FONTS.has(name)) return 'Georgia, "Times New Roman", serif';
  return 'system-ui, sans-serif';
}

// --- merging --------------------------------------------------------------

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Deep merge, where an array in the override replaces rather than merges. */
export function merge(base, override) {
  if (!isPlainObject(base) || !isPlainObject(override)) {
    return override === undefined ? base : structuredClone(override);
  }
  const out = structuredClone(base);
  for (const [key, value] of Object.entries(override)) {
    if (value === undefined) continue;
    out[key] = isPlainObject(value) && isPlainObject(out[key]) ? merge(out[key], value) : structuredClone(value);
  }
  return out;
}

/** The parts of `current` that differ from `base` — what gets shared or stored. */
export function diff(base, current) {
  if (!isPlainObject(base) || !isPlainObject(current)) {
    return JSON.stringify(base) === JSON.stringify(current) ? undefined : current;
  }
  const out = {};
  for (const [key, value] of Object.entries(current)) {
    const changed = diff(base[key], value);
    if (changed !== undefined) out[key] = changed;
  }
  return Object.keys(out).length ? out : undefined;
}

// --- persistence ----------------------------------------------------------

function encode(object) {
  const bytes = new TextEncoder().encode(JSON.stringify(object));
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function decode(text) {
  const padded = text.replace(/-/g, '+').replace(/_/g, '/');
  const binary = atob(padded + '='.repeat((4 - (padded.length % 4)) % 4));
  const bytes = Uint8Array.from(binary, (c) => c.charCodeAt(0));
  return JSON.parse(new TextDecoder().decode(bytes));
}

function readHash() {
  const params = new URLSearchParams(location.hash.replace(/^#/, ''));
  const packed = params.get(HASH_KEY);
  if (!packed) return null;
  try {
    return decode(packed);
  } catch {
    return null;
  }
}

function readStored() {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

/**
 * Load the site config: the repository's file, then whatever the visitor has
 * changed. A link's settings beat local ones, so a shared URL shows the sender's
 * page and not the recipient's.
 */
export async function load() {
  let base = FALLBACK;
  try {
    const response = await fetch('site.config.json', { cache: 'no-cache' });
    if (response.ok) base = merge(FALLBACK, await response.json());
  } catch {
    /* The fallback above is enough to render. */
  }
  const overrides = readHash() ?? readStored() ?? {};
  return { base, config: merge(base, overrides) };
}

export function persist(base, config) {
  const changes = diff(base, config);
  try {
    if (changes) localStorage.setItem(STORAGE_KEY, JSON.stringify(changes));
    else localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* Private browsing. The page still works; the settings just will not last. */
  }
  return changes;
}

export function clearStored() {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    /* Nothing to clear. */
  }
  history.replaceState(null, '', location.pathname + location.search);
}

export function shareLink(base, config) {
  const changes = diff(base, config);
  const url = new URL(location.href);
  url.hash = changes ? `${HASH_KEY}=${encode(changes)}` : '';
  return url.toString();
}

// --- applying -------------------------------------------------------------

let fontLink = null;

function applyFonts(theme) {
  const picked = [theme.display, theme.body, theme.mono].filter(
    (name, index, all) => FONT_SPECS[name] && all.indexOf(name) === index,
  );
  const href =
    'https://fonts.googleapis.com/css2?' +
    picked.map((name) => `family=${FONT_SPECS[name]}`).join('&') +
    '&display=swap';

  if (!fontLink) {
    fontLink = document.createElement('link');
    fontLink.rel = 'stylesheet';
    document.head.append(fontLink);
  }
  if (fontLink.href !== href) fontLink.href = href;

  const root = document.documentElement.style;
  root.setProperty('--font-display', `"${theme.display}", ${fallbackStack(theme.display)}`);
  root.setProperty('--font-body', `"${theme.body}", ${fallbackStack(theme.body)}`);
  root.setProperty('--font-mono', `"${theme.mono}", ${fallbackStack(theme.mono)}`);
  // A display face set in mono wants looser tracking than a didone does.
  root.setProperty('--wordmark-tracking', MONO_FONTS.has(theme.display) ? '-0.02em' : '-0.035em');
}

/** Push the theme into CSS custom properties on `:root`. */
export function applyTheme(theme) {
  const preset = THEMES[theme.preset] ?? THEMES.paper;
  const root = document.documentElement;
  root.dataset.theme = THEMES[theme.preset] ? theme.preset : 'paper';
  for (const [name, value] of Object.entries(preset.tokens)) {
    root.style.setProperty(name, value);
  }
  // An accent of "" means "whatever the preset says", which is how a preset
  // stays a complete design rather than a background colour.
  if (theme.accent) {
    root.style.setProperty('--accent', theme.accent);
    root.style.setProperty('--accent-ink', contrastInk(theme.accent));
  }
  root.style.setProperty('--grain-opacity', theme.grain ? preset.tokens['--grain'] : '0');
  root.style.setProperty('--wordmark-scale', String(theme.wordmarkScale ?? 1));
  applyFonts(theme);
}

/** Black or white, whichever is readable on `hex`. */
function contrastInk(hex) {
  const match = /^#?([\da-f]{6})$/i.exec(hex.trim());
  if (!match) return '#ffffff';
  const value = parseInt(match[1], 16);
  const channels = [(value >> 16) & 255, (value >> 8) & 255, value & 255].map((c) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  });
  const luminance = 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
  return luminance > 0.42 ? '#12100c' : '#fbf9f4';
}
