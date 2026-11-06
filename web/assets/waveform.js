// Drawing the waveform.
//
// A min/max envelope per pixel column, on paper ruled like a chart recorder's.
// The colours are read from the theme's custom properties on every draw, so
// switching preset redraws in the new palette without this file knowing the
// names of any of them.

const PAD = 2; // leaves the tallest peak a hair short of the frame

export class Scope {
  /** @param {HTMLCanvasElement} canvas */
  constructor(canvas) {
    this.canvas = canvas;
    this.context = canvas.getContext('2d');
    /** @type {Int16Array|null} */
    this.samples = null;
    /** The loudest sample in `samples`, which sets the vertical scale. */
    this.peak = 32768;
    this.progress = 1; // 0..1, for the draw-in after a render

    this.observer = new ResizeObserver(() => this.draw());
    this.observer.observe(canvas);
  }

  #token(name, fallback) {
    const value = getComputedStyle(this.canvas).getPropertyValue(name).trim();
    return value || fallback;
  }

  /** Show a waveform, sweeping it in from the left. */
  show(samples) {
    this.samples = samples;
    // Fit the frame to the loudest sample, the way an editor draws a clip.
    // The two backends differ by a good 10 dB in level and a fixed full-scale
    // axis would draw one of them as a hairline; there is no amplitude scale
    // printed here for that to contradict.
    let peak = 0;
    for (let i = 0; i < samples.length; i += 1) {
      const value = Math.abs(samples[i]);
      if (value > peak) peak = value;
    }
    this.peak = Math.max(peak, 1024);
    this.progress = 0;
    const started = performance.now();
    const duration = 420;

    const step = (now) => {
      const t = Math.min((now - started) / duration, 1);
      // Ease out, so the sweep arrives rather than stops.
      this.progress = 1 - (1 - t) ** 3;
      this.draw();
      if (t < 1) requestAnimationFrame(step);
    };
    requestAnimationFrame(step);

    // A tab the compositor has stopped drawing never runs a frame callback,
    // and a sweep that never starts is a waveform that never appears. Drawing
    // it whole costs the animation and keeps the picture.
    setTimeout(() => {
      if (this.samples === samples && this.progress === 0) {
        this.progress = 1;
        this.draw();
      }
    }, duration);
  }

  clear() {
    this.samples = null;
    this.draw();
  }

  draw() {
    const { canvas, context } = this;
    const ratio = Math.min(window.devicePixelRatio || 1, 2);
    const width = canvas.clientWidth;
    const height = canvas.clientHeight;
    if (!width || !height) return;

    if (canvas.width !== Math.round(width * ratio) || canvas.height !== Math.round(height * ratio)) {
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
    }
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);

    // An empty scope is empty paper. Ruling it anyway drew a line straight
    // through the "press Speak" message sitting on top of the canvas.
    if (!this.samples || !this.samples.length) return;

    const rule = this.#token('--rule', '#ddd');
    const mid = height / 2;

    // Chart-recorder ruling: quarter lines faint, the baseline solid.
    context.strokeStyle = rule;
    context.lineWidth = 1;
    context.globalAlpha = 0.6;
    context.beginPath();
    for (const fraction of [0.25, 0.75]) {
      const y = Math.round(height * fraction) + 0.5;
      context.moveTo(0, y);
      context.lineTo(width, y);
    }
    context.stroke();
    context.globalAlpha = 1;

    context.beginPath();
    context.moveTo(0, Math.round(mid) + 0.5);
    context.lineTo(width, Math.round(mid) + 0.5);
    context.stroke();

    const samples = this.samples;
    const columns = Math.max(1, Math.floor(width));
    const perColumn = samples.length / columns;
    const scale = (mid - PAD) / (this.peak || 32768);
    const drawn = Math.ceil(columns * this.progress);

    context.fillStyle = this.#token('--signal', '#333');
    for (let column = 0; column < drawn; column += 1) {
      const from = Math.floor(column * perColumn);
      const to = Math.min(samples.length, Math.floor((column + 1) * perColumn));
      let low = 0;
      let high = 0;
      for (let i = from; i < to; i += 1) {
        const value = samples[i];
        if (value < low) low = value;
        else if (value > high) high = value;
      }
      const top = mid - high * scale;
      const bottom = mid - low * scale;
      // A silent column still gets a hairline, so the baseline stays unbroken.
      context.fillRect(column, top, 1, Math.max(bottom - top, 1));
    }
  }
}

/** Format seconds the way a timeline labels them: 0.5 s, 12 s, 1:04. */
export function formatTime(seconds) {
  if (seconds < 1) return `${seconds.toFixed(2)}s`;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0)}s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes}:${String(Math.round(seconds % 60)).padStart(2, '0')}`;
}

/**
 * Fill a time ruler with ticks at a round interval.
 *
 * The interval is chosen so the labels never crowd: roughly one every 90
 * pixels, snapped to something a person would actually write down.
 */
export function drawRuler(element, duration, width) {
  element.replaceChildren();
  if (!duration || !width) return;

  const target = duration / Math.max(2, Math.floor(width / 90));
  const steps = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60];
  const step = steps.find((candidate) => candidate >= target) ?? 60;

  for (let t = 0; t <= duration + 1e-6; t += step) {
    const tick = document.createElement('span');
    tick.className = 'ruler__tick';
    tick.style.left = `${(t / duration) * 100}%`;
    tick.textContent = formatTime(t);
    element.append(tick);
  }
}
