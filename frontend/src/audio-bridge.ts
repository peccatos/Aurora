/** Web Audio output bridge.
 *
 * The wasm kernel decodes + processes audio into its ring; the browser
 * owns the device (ADR 0006). This module drains the ring through
 * `web_pull` and feeds a Web Audio graph.
 *
 * Implementation: ONE scheduled pull loop + AudioBufferSourceNodes
 * (zero hosting requirements; an AudioWorklet upgrade needs
 * SharedArrayBuffer/COOP-COEP and is a drop-in later). Latency is
 * honest: ~200ms scheduling slack.
 *
 * Loop lifecycle (the bugs this design prevents):
 *  - Exactly one loop, ever: `running` is claimed SYNCHRONOUSLY at the
 *    top of startOutput, BEFORE any await — two overlapping calls
 *    (autoplay-blocked resume + the 500ms status poll both calling
 *    startOutput) previously both passed the `!running` check and
 *    started two loops over one ring: doubled, 2x-speed audio.
 *  - Pause/seek/track change kill only the SCHEDULED NODES, not the
 *    loop: the ring is empty then (cleared engine-side), the loop's
 *    dry branch just parks the cursor, and playback continues when the
 *    ring refills. Killing the loop there left `running=true` with no
 *    consumer — silence or stale overlap.
 *  - Full teardown (stop) uses the generation token so a superseded
 *    loop iteration can never reschedule itself.
 */

import { platform, webKernel } from "./invoke";

let ctx: AudioContext | null = null;
/** Claimed synchronously in startOutput; released only in stopOutput. */
let running = false;
let sampleRate = 0;
let channels = 2;

/** Sources currently scheduled on the Web Audio clock. Stopped and
 *  dropped on transport interruptions (pause/seek/track change). */
const scheduled: AudioBufferSourceNode[] = [];

/** Generation token: bumped on full teardown; a loop iteration from a
 *  superseded generation never reschedules. */
let generation = 0;

/** Start (or parameter-refresh) the output bridge for the current
 *  stream. Call from a user gesture when possible — browsers gate
 *  AudioContext.resume() on interaction. */
export async function startOutput(rate: number, ch: number): Promise<void> {
  if (platform !== "web") return;
  if (running && (rate !== sampleRate || ch !== channels)) {
    // Format change (track with a different rate/channels): the old
    // loop's scheduled buffers carry OLD-format bytes — playing them
    // at the new rate is pitch-shift ("frequencies jumping"). Full
    // restart: drop everything, re-claim with the new parameters.
    stopOutput();
  }
  sampleRate = rate;
  channels = ch;
  if (running) {
    // Same format, loop already alive: nothing to do.
    return;
  }
  // Claim BEFORE the first await: concurrent callers must not both
  // pass the guard while resume() is pending.
  running = true;
  if (!ctx) {
    ctx = new AudioContext();
  }
  if (ctx.state === "suspended") {
    try {
      await ctx.resume();
    } catch {
      // Retried on the next user click (see unlockAudio). The loop
      // still runs; while suspended the clock stands still, so it just
      // parks — nothing is scheduled or lost.
    }
  }
  scheduleNext(generation);
}

/** Called from any user gesture: unlock/resume the audio context so
 *  later programmatic starts (status poll) are not blocked by the
 *  browser's autoplay policy. */
export function unlockAudio(): void {
  if (platform !== "web") return;
  if (!ctx) ctx = new AudioContext();
  if (ctx.state === "suspended") void ctx.resume();
}

/** Pause: kill the scheduled audio NOW (the kernel ring is already
 *  cleared engine-side). The LOOP stays alive — it parks on the dry
 *  ring and picks up exactly where the engine left off on resume. */
export function pauseOutput(): void {
  dropScheduledAudio();
}

/** Seek / track change / EQ drain: everything scheduled is stale; the
 *  loop re-reads the (re-filled) ring on its next tick. */
export function flushOutput(): void {
  dropScheduledAudio();
}

/** Full stop: teardown of the loop AND the scheduled audio. */
export function stopOutput(): void {
  generation++;
  running = false;
  dropScheduledAudio();
}

export function outputRunning(): boolean {
  return running;
}

/** Stop and drop scheduled sources. Does NOT touch the loop or the
 *  generation token. */
function dropScheduledAudio(): void {
  while (scheduled.length > 0) {
    const node = scheduled.pop();
    try {
      node?.stop();
      node?.disconnect();
    } catch {
      // Already ended — fine.
    }
  }
  nextTime = 0; // rebase the cursor to the live audio clock
}

// ---------------------------------------------------------------------------
// Pull loop: each iteration queues ~200ms of audio ahead.
// ---------------------------------------------------------------------------

const AHEAD_MS = 200;
let nextTime = 0;

function scheduleNext(token: number): void {
  if (token !== generation) return; // superseded by a full stop
  const kernel = webKernel();
  void kernel.then((k) => {
    if (!k || !ctx || token !== generation) return;
    if (!running) return;
    const now = ctx.currentTime;
    // Cursor rebase: never schedule in the past (overlapping starts =
    // audible doubling), never more than the schedule window ahead.
    if (nextTime < now + 0.05) nextTime = now + 0.05;
    if (nextTime > now + AHEAD_MS / 1000) nextTime = now + AHEAD_MS / 1000;
    // THE horizon is on the AUDIO CLOCK, not the cursor: keep AHEAD_MS
    // of audio queued ahead of `now`. (Scheduling until
    // nextTime + AHEAD_MS instead let the cursor advance 200ms every
    // 50ms tick — pulling 4x faster than realtime: the position raced
    // to ~4x wall time, "skipping seconds" with every tick.)
    const horizon = now + AHEAD_MS / 1000;
    while (nextTime < horizon) {
      const chunkFrames = 4096;
      const maxBytes = chunkFrames * channels * 4;
      const samples = k.web_pull(maxBytes);
      const frames = Math.floor(samples.length / channels);
      if (frames === 0) {
        // Ring dry: paused / decoding / between tracks. Park the cursor
        // just ahead of the clock — do NOT advance past the horizon
        // (that would fast-forward the timeline).
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
        // Bound the list; the browser GCs ended nodes on its own.
        scheduled.splice(0, scheduled.length - 256).forEach((n) => {
          try {
            n.disconnect();
          } catch {
            /* ended */
          }
        });
      }
    }
    if (running && token === generation) {
      setTimeout(() => scheduleNext(token), 50);
    }
  });
}

/** Teardown (page unload). */
export function disposeOutput(): void {
  generation++;
  running = false;
  dropScheduledAudio();
  void ctx?.close();
  ctx = null;
}
