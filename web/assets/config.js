// The page's settings: what it says, what it looks like, and which engines it
// offers. All of it comes from `site.config.json`; nothing is baked into the
// markup, and nothing here is editable from the page itself.

/** Enough of a config to render something if `site.config.json` is missing. */
const FALLBACK = {
  brand: { name: 'flite-rs', standfirst: '', note: '' },
  theme: { accent: '', display: 'Instrument Serif', body: 'Archivo', mono: 'DM Mono', grain: true },
  labels: {},
  engines: [],
  defaultEngine: '',
  samples: [],
  panels: { waveform: true, phones: true, readout: true },
  links: [],
};

/* The palette. One set of tokens, not a choice of several: the page is a
 * printed record under lamplight and the ink is warm on near-black. */
const TOKENS = {
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
};

// Only these are fetched, so the font request stays one round trip.
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
const MONO_FONTS = new Set(Object.keys(FONT_SPECS).filter((name) => name.includes('Mono')));

function fallbackStack(name) {
  if (MONO_FONTS.has(name)) return 'ui-monospace, SFMono-Regular, Menlo, monospace';
  if (SERIF_FONTS.has(name)) return 'Georgia, "Times New Roman", serif';
  return 'system-ui, sans-serif';
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

/** Deep merge, where an array in the override replaces rather than merges. */
function merge(base, override) {
  if (!isPlainObject(base) || !isPlainObject(override)) {
    return override === undefined ? base : structuredClone(override);
  }
  const out = structuredClone(base);
  for (const [key, value] of Object.entries(override)) {
    if (value === undefined) continue;
    out[key] =
      isPlainObject(value) && isPlainObject(out[key]) ? merge(out[key], value) : structuredClone(value);
  }
  return out;
}

/** Read `site.config.json`, falling back to enough to render. */
export async function load() {
  try {
    const response = await fetch('site.config.json', { cache: 'no-cache' });
    if (response.ok) return merge(FALLBACK, await response.json());
  } catch {
    /* The fallback above is enough to render. */
  }
  return FALLBACK;
}

function applyFonts(theme) {
  const picked = [theme.display, theme.body, theme.mono].filter(
    (name, index, all) => FONT_SPECS[name] && all.indexOf(name) === index,
  );
  const link = document.createElement('link');
  link.rel = 'stylesheet';
  link.href =
    'https://fonts.googleapis.com/css2?' +
    picked.map((name) => `family=${FONT_SPECS[name]}`).join('&') +
    '&display=swap';
  document.head.append(link);

  const root = document.documentElement.style;
  root.setProperty('--font-display', `"${theme.display}", ${fallbackStack(theme.display)}`);
  root.setProperty('--font-body', `"${theme.body}", ${fallbackStack(theme.body)}`);
  root.setProperty('--font-mono', `"${theme.mono}", ${fallbackStack(theme.mono)}`);
  // A display face set in mono wants looser tracking than a didone does.
  root.setProperty('--wordmark-tracking', MONO_FONTS.has(theme.display) ? '-0.02em' : '-0.035em');
}

/** Push the palette and the faces into custom properties on `:root`. */
export function applyTheme(theme) {
  const root = document.documentElement;
  for (const [name, value] of Object.entries(TOKENS)) {
    root.style.setProperty(name, value);
  }
  // An accent is the one colour worth overriding from the config file, since
  // it is the only one that carries a brand.
  if (theme.accent) {
    root.style.setProperty('--accent', theme.accent);
    root.style.setProperty('--accent-ink', contrastInk(theme.accent));
  }
  root.style.setProperty('--grain-opacity', theme.grain === false ? '0' : TOKENS['--grain']);
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
