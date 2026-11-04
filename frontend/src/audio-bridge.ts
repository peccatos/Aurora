/** Web Audio output bridge — AudioWorklet edition (W9).
 *
 * ONE continuous AudioWorklet stream instead of per-chunk
 * AudioBufferSourceNodes: node seams joined with sub-sample jitter
 * were the "noise/fizz" on sustained audio. Latency drops from
 * ~200ms (schedule-ahead) to ~1.5 render quanta + fill margin
 * (~50ms). No SharedArrayBuffer needed (message-transfer push), so
 * plain hosting (GitHub Pages) still works.
 *
 * The main thread keeps a small pull loop: top the worklet's ring up
 * to a fill margin from the kernel's ring; on pause/seek/track change
 * FLUSH the worklet buffer (those bytes were scheduled-not-heard).
 * The kernel-side position correction (ADR 0002) covers the pause
 * case as before.
 *
 * Fallback: if AudioWorklet is unavailable, the legacy
 * AudioBufferSourceNode path runs (kept, marked; seams remain —
 * old browsers only).
 */

import { platform, webKernel } from "./invoke";

let ctx: AudioContext | null = null;
let running = false;
let sampleRate = 0;
let channels = 2;

/** Worklet node + its port state. */
let node: AudioWorkletNode | null = null;
/** Fill margin in frames: keep this much queued in the worklet. */
const MARGIN_MS = 120;
/** Legacy fallback scheduled nodes. */
const scheduled: AudioBufferSourceNode[] = [];
/** True when the worklet path is active. */
let usingWorklet = false;

let generation = 0;

export async function startOutput(rate: number, ch: number): Promise<void> {
  if (platform !== "web") return;
  if (!ctx) {
    ctx = new AudioContext();
  }
  const k0 = await webKernel();
  if (k0 && !deviceTold) {
    deviceTold = true;
    try {
      k0.set_output_rate(ctx.sampleRate);
      k0.set_output_channels(2);
    } catch {
      /* older binary — browser resampling remains */
    }
  }
  if (running && rate !== sampleRate) {
    stopOutput(); // rate change = stale worklet format
  }
  sampleRate = rate;
  channels = ch;
  if (running) return;
  running = true;
  if (ctx.state === "suspended") {
    try {
      await ctx.resume();
    } catch {
      /* retried on the next user click */
    }
  }
  // Try the worklet path once per context.
  if (!node) {
    try {
      const url = new URL("./worklet-processor.js", import.meta.url).href;
      await ctx.audioWorklet.addModule(url);
      node = new AudioWorkletNode(ctx, "aurora-processor", {
        numberOfInputs: 0,
        numberOfOutputs: 1,
        outputChannelCount: [Math.min(2, channels)],
        processorOptions: { channels: Math.min(2, channels) },
      });
      node.port.onmessage = (e) => {
        const msg = e.data;
        if (msg.type === "state") {
          workletAvailable = msg.available;
          workletUnderruns = msg.underruns;
        } else if (msg.type === "ack") {
          workletAvailable = msg.frames;
        } else if (msg.type === "flushed") {
          // The worklet buffer is empty NOW — the main-thread fill loop
          // must know, or it thinks the margin is full and leaves a
          // gap of silence after every seek/pause (audible click).
          workletAvailable = 0;
        }
      };
      node.connect(ctx.destination);
      usingWorklet = true;
    } catch {
      // No AudioWorklet (ancient browser): legacy path.
      node = null;
      usingWorklet = false;
    }
  }
  scheduleNext(generation);
}

let deviceTold = false;
let workletAvailable = 0;
let workletUnderruns = 0;

export function unlockAudio(): void {
  if (platform !== "web") return;
  if (!ctx) ctx = new AudioContext();
  if (ctx.state === "suspended") void ctx.resume();
}

/** Pause: flush the worklet buffer (scheduled-not-heard bytes must
 *  not play after resume); the kernel reports the position honestly. */
export function pauseOutput(): void {
  dropScheduledAudio("pause");
}

/** Seek / track change: same flush; engine rebases position. */
export function flushOutput(): void {
  dropScheduledAudio("flush");
}

export function stopOutput(): void {
  generation++;
  running = false;
  dropScheduledAudio("stop");
}

export function outputRunning(): boolean {
  return running;
}

function dropScheduledAudio(reason: "pause" | "flush" | "stop"): void {
  if (ctx && reason === "pause") {
    const now = ctx.currentTime;
    if (nextTime > now && usingWorklet) {
      // Worklet path: the unheard span is the worklet's fill margin.
      const unheard = Math.min(nextTime - now, MARGIN_MS / 1000);
      webKernel().then((k) => {
        if (k) {
          try {
            k.position_correction_s(-unheard);
          } catch {
            /* engine gone — stop will reset */
          }
        }
      });
    }
  }
  if (node) {
    node.port.postMessage({ type: "flush" });
    workletAvailable = 0;
  }
  while (scheduled.length > 0) {
    const src = scheduled.pop();
    try {
      src?.stop();
      src?.disconnect();
    } catch {
      /* ended */
    }
  }
  nextTime = 0;
}

// ---------------------------------------------------------------------------
// Fill loop: pull from the kernel ring, push into the worklet (or
// schedule legacy nodes when the worklet is unavailable).
// ---------------------------------------------------------------------------

let nextTime = 0;

function scheduleNext(token: number): void {
  if (token !== generation) return;
  const kernel = webKernel();
  void kernel.then((k) => {
    if (!k || !ctx || token !== generation) return;
    if (!running) return;
    if (usingWorklet && node) {
      const marginFrames = Math.round((MARGIN_MS / 1000) * sampleRate);
      // Top up in ~50ms pulls: bounded so one pull can't freeze the UI.
      // Pull size never exceeds the free space: the worklet drops a
      // full ring's tail (a click), and oversizing the transfer wastes
      // copies.
      while (workletAvailable < marginFrames) {
        const free = marginFrames - workletAvailable;
        const chunkFrames = Math.min(4096, free);
        const samples = k.web_pull(chunkFrames * channels * 4);
        const frames = Math.floor(samples.length / channels);
        if (frames === 0) break;
        // Transfer the buffer copy (the view borrows wasm memory).
        const copy = new Float32Array(frames * channels);
        copy.set(samples.subarray(0, frames * channels));
        node.port.postMessage({ type: "push", samples: copy }, [copy.buffer]);
        workletAvailable += frames;
      }
      node.port.postMessage({ type: "query" });
    } else if (ctx) {
      // Legacy: AudioBufferSourceNode scheduling (seams = known noise).
      const now = ctx.currentTime;
      if (nextTime < now + 0.05) nextTime = now + 0.05;
      if (nextTime > now + 0.3) nextTime = now + 0.3;
      const horizon = now + 0.2;
      while (nextTime < horizon) {
        const chunkFrames = 4096;
        const samples = k.web_pull(chunkFrames * channels * 4);
        const frames = Math.floor(samples.length / channels);
        if (frames === 0) {
          nextTime = Math.max(nextTime, now + 0.05);
          break;
        }
        const buf = ctx.createBuffer(channels, frames, sampleRate);
        for (let ch = 0; ch < channels; ch++) {
          const target = buf.getChannelData(ch);
          for (let i = 0; i < frames; i++) {
            target[i] = samples[i * channels + ch];
          }
        }
        const src = ctx.createBufferSource();
        src.buffer = buf;
        src.connect(ctx.destination);
        src.start(nextTime);
        scheduled.push(src);
        nextTime += frames / sampleRate;
        if (scheduled.length > 256) {
          scheduled.splice(0, scheduled.length - 256).forEach((n) => {
            try {
              n.disconnect();
            } catch {
              /* ended */
            }
          });
        }
      }
    }
    if (running && token === generation) {
      setTimeout(() => scheduleNext(token), 25);
    }
  });
}

/** Teardown (page unload). */
export function disposeOutput(): void {
  generation++;
  running = false;
  dropScheduledAudio("stop");
  try {
    node?.disconnect();
  } catch {
    /* gone */
  }
  node = null;
  void ctx?.close();
  ctx = null;
}
