/** Central application state + typed invoke wrappers.
 *  Single source of truth every page module subscribes to. */

import { invoke } from "@tauri-apps/api/core";
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

export const app = {
  /** Path of the track the transport acts on (file picker / queue current). */
  selectedPath: null as string | null,
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
// Transport (audio.ts would re-export these; kept here for cohesion)
// ---------------------------------------------------------------------------

export async function loadStatus(): Promise<PlaybackStatus> {
  const s = await invoke<PlaybackStatus>("playback_status");
  app.status = s;
  if (s.duration_s && s.duration_s > 0) app.durationS = s.duration_s;
  if (s.state === "idle") app.exclusiveActive = false;
  notify();
  return s;
}

export async function playQueueIndex(idx: number): Promise<QueueDto> {
  const q = await invoke<QueueDto>("play_queue_index", { index: idx });
  app.queueCurrent = q.current;
  const cur = q.tracks[q.current ?? 0];
  if (cur) app.selectedPath = cur.path;
  await loadStatus();
  return q;
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
