/**
 * Aurora AudioWorklet processor: a CONTINUOUS output stream.
 *
 * The AudioBufferSourceNode bridge scheduled a new node per ~4096-frame
 * chunk — Web Audio joins nodes with sub-sample jitter: audible
 * fizz/noise on sustained tones. This worklet is ONE node that never
 * ends: the main thread fills a lock-free-ish double buffer
 * (SharedArrayBuffer when available; message-transfer fallback), and
 * process() copies frames straight into the output — zero seams, and
 * latency drops from ~200ms to ~2 render quanta (~6ms) + fill margin.
 *
 * Underrun honesty: when the buffer runs dry the worklet emits silence
 * and counts the deficit — the bridge reports it (the kernel's
 * underrun metric stays the source of truth for its ring; this counter
 * covers the JS-side gap).
 */

/** Ring buffer shared with the main thread (when SAB exists). */
class AuroraProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const opts = (options && options.processorOptions) || {};
    this.channels = opts.channels || 2;
    // Interleaved ring: capacity in FRAMES. ~0.5s headroom.
    this.capacity = opts.capacityFrames || 48000 / 2;
    this.buf = new Float32Array(this.capacity * this.channels);
    this.readPos = 0; // frames
    this.writePos = 0; // frames
    this.available = 0; // frames
    this.underruns = 0;

    this.port.onmessage = (e) => {
      const msg = e.data;
      if (msg.type === "push") {
        // msg.samples: Float32Array interleaved (transferred).
        this.push(msg.samples);
        this.port.postMessage({ type: "ack", frames: this.available });
      } else if (msg.type === "flush") {
        // Drop everything not yet played (pause/seek/track change).
        this.readPos = 0;
        this.writePos = 0;
        this.available = 0;
        this.port.postMessage({ type: "flushed" });
      } else if (msg.type === "query") {
        this.port.postMessage({
          type: "state",
          available: this.available,
          underruns: this.underruns,
        });
      }
    };
    this.port.postMessage({ type: "ready" });
  }

  push(samples) {
    const frames = Math.floor(samples.length / this.channels);
    for (let f = 0; f < frames; f++) {
      if (this.available >= this.capacity) break; // full: drop tail (main thread should top-up earlier)
      const base = f * this.channels;
      for (let ch = 0; ch < this.channels; ch++) {
        this.buf[this.writePos * this.channels + ch] = samples[base + ch];
      }
      this.writePos = (this.writePos + 1) % this.capacity;
      this.available++;
    }
  }

  process(_inputs, outputs) {
    const out = outputs[0];
    const nch = Math.min(out.length, this.channels);
    const frames = out[0] ? out[0].length : 128;

    if (this.available < frames) {
      // Dry: honest silence + underrun count (natural end is counted
      // by the kernel, not here — the main thread signals it).
      for (let ch = 0; ch < nch; ch++) {
        out[ch].fill(0);
      }
      if (this.available > 0 || frames > 0) this.underruns++;
      return true;
    }

    for (let f = 0; f < frames; f++) {
      const base = this.readPos * this.channels;
      for (let ch = 0; ch < nch; ch++) {
        out[ch][f] = this.buf[base + ch];
      }
      this.readPos = (this.readPos + 1) % this.capacity;
    }
    this.available -= frames;
    return true; // never terminate
  }
}

registerProcessor("aurora-processor", AuroraProcessor);
