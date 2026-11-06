/** Central application state + typed invoke wrappers.
 *  Single source of truth every page module subscribes to. */

import { invoke, platform } from "./invoke";
import {
  startOutput,
  stopOutput,
  outputRunning,
  pauseOutput,
  flushOutput,
} from "./audio-bridge";
import type {
  DeviceDto,
  DspConfig,
  LibraryStatus,
  PlaybackStatus,
  QueueDto,
  QueueTrack,
  TrackDto,
} from "./types";

// ---------------------------------------------------------------------------
// Mutable state
// ---------------------------------------------------------------------------

export type RepeatMode = "off" | "all" | "one";

export const app = {
  /** Path of the track the transport acts on (file picker / queue current). */
  selectedPath: null as string | null,
  /** Transport modes (persisted in kv — spec 3.1). */
  repeat: "off" as RepeatMode,
  shuffle: false,
  /** Current queue (frontend copy; backend owns the truth). */
  queueTracks: [] as QueueTrack[],
  queueCurrent: null as number | null,
  /** Last status poll result. */
  status: null as PlaybackStatus | null,
  /** Track duration for the seek bar. */
  durationS: 0,
  /** True while an exclusive-mode stream plays. */
  exclusiveActive: false,
  /** Device selector choice (persisted backend-side too). */
  device: "" as string,
  devices: [] as DeviceDto[],
  /** DSP config (EQ/reverb/speed/volume) — mirrors backend. */
  dsp: null as DspConfig | null,
  /** Library. */
  library: {
    open: false,
    tracks: [] as TrackDto[],
    selectedIds: new Set<string>(),
  },
  /** Global one-line toast (bottom of player bar area). */
  toast: "",
};

// ---------------------------------------------------------------------------
// Subscriptions: pages register render callbacks, state pushes updates.
// ---------------------------------------------------------------------------

type Listener = () => void;
const listeners = new Set<Listener>();

export function subscribe(fn: Listener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function notify() {
  for (const fn of listeners) fn();
}

export function setToast(text: string) {
  app.toast = text;
  const el = document.getElementById("toast");
  if (el) el.textContent = text;
}

// ---------------------------------------------------------------------------
// Transport modes: repeat / shuffle (spec 3.1) — persisted to kv
// ---------------------------------------------------------------------------

/** Saved-state boot: call once at startup, before the first render. */
export async function loadTransportModes(): Promise<void> {
  try {
    const { kvGet } = await import("./library-db");
    const r = await kvGet<RepeatMode>("aw:repeat");
    if (r === "off" || r === "all" || r === "one") app.repeat = r;
    const sh = await kvGet<boolean>("aw:shuffle");
    if (typeof sh === "boolean") app.shuffle = sh;
  } catch {
    /* storage unavailable — defaults stand */
  }
  notify();
}

export async function setRepeat(mode: RepeatMode): Promise<void> {
  app.repeat = mode;
  notify();
  try {
    const { kvPut } = await import("./library-db");
    await kvPut("aw:repeat", mode);
  } catch {
    /* best-effort persistence */
  }
}

export async function setShuffle(on: boolean): Promise<void> {
  app.shuffle = on;
  notify();
  try {
    const { kvPut } = await import("./library-db");
    await kvPut("aw:shuffle", on);
  } catch {
    /* best-effort persistence */
  }
}

// ---------------------------------------------------------------------------
// Transport (audio.ts would re-export these; kept here for cohesion)
// ---------------------------------------------------------------------------

/** Path of the track the output bridge last saw playing (web track-change
 *  detection — scheduled stale audio must die on a track switch). */
let lastTrackPath: string | null = null;

export async function loadStatus(): Promise<PlaybackStatus> {
  const s = await invoke<PlaybackStatus>("playback_status");
  const prevState = app.status?.state ?? "idle";
  app.status = s;
  if (s.duration_s && s.duration_s > 0) app.durationS = s.duration_s;
  if (s.state === "idle") app.exclusiveActive = false;
  // Web build: drive the browser output while the engine plays.
  if (platform === "web") {
    // Track change while playing: the previous track's scheduled audio
    // must die before the new track's bytes mix with it (doubling).
    const cur = app.queueTracks[app.queueCurrent ?? 0]?.path ?? null;
    if (s.state === "playing" && cur !== lastTrackPath) {
      if (cur) lastTrackPath = cur;
      flushOutput();
    }
    if (s.state === "playing" && s.sample_rate) {
      if (!outputRunning()) void startOutput(s.sample_rate, s.channels ?? 2);
    } else if (s.state === "paused") {
      // Transition into pause: kill browser-scheduled audio immediately
      // (the kernel ring is cleared engine-side).
      if (prevState === "playing") pauseOutput();
    } else if (s.state === "idle" && outputRunning()) {
      stopOutput();
    }
  }
  notify();
  return s;
}

export async function playQueueIndex(idx: number): Promise<number | null> {
  // The kernel returns a PlaybackStatus (NOT a QueueDto) here — the old
  // code read q.tracks[q.current] from it and crashed with
  // "Cannot read properties of undefined (reading '0')", which silently
  // killed the whole post-play chain (status refresh, title update).
  await invoke("play_queue_index", { index: idx });
  app.queueCurrent = idx;
  const cur = app.queueTracks[idx];
  if (cur) app.selectedPath = cur.path;
  await loadStatus();
  void import("./dsp-profiles").then(({ applyDspProfile }) => applyDspProfile(cur?.path ?? null)).catch(() => undefined);
  notify();
  return app.queueCurrent;
}

export async function setQueue(tracks: QueueTrack[]): Promise<QueueDto> {
  const q = await invoke<QueueDto>("set_queue", { tracks });
  app.queueTracks = q.tracks;
  app.queueCurrent = q.current;
  notify();
  return q;
}

export async function nextTrack(): Promise<QueueDto | null> {
  const q = await invoke<QueueDto | null>("next_track");
  if (q) {
    app.queueTracks = q.tracks;
    app.queueCurrent = q.current;
    const cur = q.tracks[q.current ?? 0];
    if (cur) app.selectedPath = cur.path;
    void import("./dsp-profiles").then(({ applyDspProfile }) => applyDspProfile(cur?.path ?? null)).catch(() => undefined);
    notify();
  } else {
    app.queueCurrent = null;
    notify();
  }
  await loadStatus();
  return q;
}

export async function prevTrack(): Promise<QueueDto> {
  const q = await invoke<QueueDto>("prev_track");
  app.queueTracks = q.tracks;
  app.queueCurrent = q.current;
  const cur = q.tracks[q.current ?? 0];
  if (cur) app.selectedPath = cur.path;
  void import("./dsp-profiles").then(({ applyDspProfile }) => applyDspProfile(cur?.path ?? null)).catch(() => undefined);
  notify();
  await loadStatus();
  return q;
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

export async function refreshLibraryStatus(): Promise<LibraryStatus> {
  try {
    const st = await invoke<LibraryStatus>("library_status");
    app.library.open = st.open;
    notify();
    return st;
  } catch {
    app.library.open = false;
    notify();
    return { open: false, path: null, track_count: 0 };
  }
}

export function trackToQueueTrack(t: TrackDto): QueueTrack {
  return {
    path: t.path,
    title: t.title,
    artist: t.artist,
    replaygain_track_db: t.replaygain_track_db,
    replaygain_album_db: t.replaygain_album_db,
  };
}
