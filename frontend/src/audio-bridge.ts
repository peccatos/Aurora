/** Web Audio output bridge.
 *
 * The wasm kernel decodes + processes audio into its ring; the browser
 * owns the device (ADR 0006). This module drains the ring through
 * `web_pull` and feeds a Web Audio graph.
 *
 * Implementation choice: an AudioWorklet is the RT-correct path, but it
 * needs SharedArrayBuffer (COOP/COEP) to share the ring. This bridge
 * uses a ScriptProcessorNode-free approach instead: a scheduled pull
 * loop + AudioBufferSourceNodes. It is honest about latency (~100ms
 * scheduling slack) and keeps hosting requirements zero (plain GitHub
 * Pages works). A worklet upgrade is a drop-in later.
 */

import { platform, webKernel } from "./invoke";

let ctx: AudioContext | null = null;
let running = false;
let sampleRate = 0;
let channels = 2;

/** Start (or restart) the output bridge for the current stream. */
export async function startOutput(rate: number, ch: number): Promise<void> {
  if (platform !== "web") return;
  sampleRate = rate;
  channels = ch;
  if (!ctx) {
    ctx = new AudioContext();
  }
  if (ctx.state === "suspended") {
    await ctx.resume();
  }
  if (!running) {
    running = true;
    scheduleNext();
  }
}

export function stopOutput(): void {
  running = false;
}

export function outputRunning(): boolean {
  return running;
}

// ---------------------------------------------------------------------------
// Pull loop: each iteration queues ~200ms of audio ahead.
// ---------------------------------------------------------------------------

const AHEAD_MS = 200;
let nextTime = 0;
let pullTimer: ReturnType<typeof setTimeout> | null = null;

function scheduleNext(): void {
  if (!running || !ctx) return;
  const kernel = webKernel();
  void kernel.then((k) => {
    if (!k || !ctx) return;
    const now = ctx.currentTime;
    if (nextTime < now + 0.05) nextTime = now + 0.05;
    // Fill up to AHEAD_MS ahead of the schedule cursor.
    const horizon = nextTime + AHEAD_MS / 1000;
    while (nextTime < horizon) {
      const chunkFrames = 4096;
      const maxBytes = chunkFrames * channels * 4;
      const samples = k.web_pull(maxBytes);
      const frames = Math.floor(samples.length / channels);
      if (frames === 0) {
        // Ring dry: the worker is decoding (or idle). Keep the clock.
        nextTime += 0.05;
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
      nextTime += frames / sampleRate;
    }
    pullTimer = setTimeout(scheduleNext, 50);
  });
}

/** Teardown (page unload). */
export function disposeOutput(): void {
  running = false;
  if (pullTimer) clearTimeout(pullTimer);
  void ctx?.close();
  ctx = null;
}
