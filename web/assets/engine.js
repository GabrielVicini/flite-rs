// Talking to the synthesiser.
//
// Each engine on the page is its own `.wasm` file, because a voice is mostly
// data and there is no reason to make a visitor download the 16 kHz diphone
// database to hear the 8 kHz one. Modules are fetched the first time their
// engine is picked and kept afterwards.

const decoder = new TextDecoder();
const encoder = new TextEncoder();

/** @type {Map<string, Promise<Synth>>} */
const cache = new Map();

class Synth {
  /**
   * @param {WebAssembly.Instance} instance
   * @param {object} info the engine's own description of itself
   */
  constructor(instance, info) {
    this.exports = instance.exports;
    this.info = info;
  }

  /**
   * A view of linear memory, read fresh every time.
   *
   * Any call can allocate, allocating can grow the memory, and growing
   * detaches every existing view — so a cached `Uint8Array` here would turn
   * into a zero-length one partway through a session.
   */
  get #bytes() {
    return new Uint8Array(this.exports.memory.buffer);
  }

  /** Copy a string in, run `fn` over its (pointer, length), and free it. */
  #withString(text, fn) {
    const bytes = encoder.encode(text);
    const ptr = this.exports.fr_alloc(bytes.length);
    try {
      this.#bytes.set(bytes, ptr);
      return fn(ptr, bytes.length);
    } finally {
      this.exports.fr_free(ptr, bytes.length);
    }
  }

  /** Read the module's text buffer, `length` bytes from `fr_out_ptr`. */
  #readOut(length) {
    if (!length) return '';
    const ptr = this.exports.fr_out_ptr();
    return decoder.decode(this.#bytes.subarray(ptr, ptr + length));
  }

  describe() {
    const json = this.#readOut(this.exports.fr_describe());
    return json ? JSON.parse(json) : null;
  }

  selectVoice(name) {
    return this.#withString(name, (ptr, len) => this.exports.fr_select_voice(ptr, len) === 1);
  }

  setRate(value) {
    this.exports.fr_set_duration_stretch(value);
  }

  setPitch(value) {
    this.exports.fr_set_f0_shift(value);
  }

  get sampleRate() {
    return this.exports.fr_sample_rate();
  }

  /**
   * Synthesise, returning the samples and how long the engine took.
   *
   * The samples are copied out of linear memory rather than viewed in place:
   * the next call overwrites that buffer, and the page keeps the audio around
   * to replay and to save.
   */
  speak(text) {
    const started = performance.now();
    const count = this.#withString(text, (ptr, len) => this.exports.fr_synthesize(ptr, len));
    const elapsed = performance.now() - started;
    const base = this.exports.fr_pcm_ptr();
    const samples = new Int16Array(this.exports.memory.buffer, base, count).slice();
    return { samples, sampleRate: this.sampleRate, elapsed };
  }

  /** The phone sequence behind the same text, for the track under the wave. */
  phones(text) {
    const json = this.#withString(text, (ptr, len) => this.#readOut(this.exports.fr_phones(ptr, len)));
    try {
      return json ? JSON.parse(json) : [];
    } catch {
      return [];
    }
  }
}

/**
 * Fetch a module, reporting progress, because several megabytes over a slow
 * connection is long enough that a silent page looks broken.
 */
async function fetchWasm(url, onProgress) {
  const response = await fetch(url);
  if (!response.ok) throw new Error(`${response.status} fetching ${url}`);

  // `content-length` is the compressed length when the server is gzipping, so
  // it under-reports against the bytes we actually decode. Prefer the
  // uncompressed size the build manifest recorded, when there is one.
  const declared = Number(response.headers.get('content-length')) || 0;
  if (!response.body || !onProgress) return new Uint8Array(await response.arrayBuffer());

  const reader = response.body.getReader();
  const chunks = [];
  let received = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    received += value.length;
    onProgress(declared ? Math.min(received / declared, 1) : null, received);
  }
  const bytes = new Uint8Array(received);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  return bytes;
}

/**
 * Load one engine module and build its engine.
 *
 * @param {{name: string, file: string}} module from the build manifest
 * @param {(fraction: number|null, bytes: number) => void} [onProgress]
 * @returns {Promise<Synth>}
 */
export function load(module, onProgress) {
  const existing = cache.get(module.name);
  if (existing) return existing;

  const pending = (async () => {
    const bytes = await fetchWasm(module.file, onProgress);
    // No imports: the module is a plain wasm32 target with no bindgen glue and
    // nothing to call back into. An empty object keeps that explicit.
    const { instance } = await WebAssembly.instantiate(bytes, {});

    const length = instance.exports.fr_init();
    const ptr = instance.exports.fr_out_ptr();
    const json = decoder.decode(
      new Uint8Array(instance.exports.memory.buffer).subarray(ptr, ptr + length),
    );
    if (!length) throw new Error(json || 'the engine would not start');

    return new Synth(instance, JSON.parse(json));
  })();

  cache.set(module.name, pending);
  pending.catch(() => cache.delete(module.name));
  return pending;
}

/** Whether a module is already in memory, so the UI can skip the spinner. */
export function isLoaded(name) {
  return cache.has(name);
}

/** Wrap mono 16-bit samples in a RIFF header. */
export function toWav(samples, sampleRate) {
  const buffer = new ArrayBuffer(44 + samples.length * 2);
  const view = new DataView(buffer);
  const text = (offset, value) => {
    for (let i = 0; i < value.length; i += 1) view.setUint8(offset + i, value.charCodeAt(i));
  };

  text(0, 'RIFF');
  view.setUint32(4, 36 + samples.length * 2, true);
  text(8, 'WAVE');
  text(12, 'fmt ');
  view.setUint32(16, 16, true); // PCM header length
  view.setUint16(20, 1, true); // uncompressed
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // bytes per second
  view.setUint16(32, 2, true); // bytes per frame
  view.setUint16(34, 16, true); // bits per sample
  text(36, 'data');
  view.setUint32(40, samples.length * 2, true);
  new Int16Array(buffer, 44).set(samples);

  return new Blob([buffer], { type: 'audio/wav' });
}
