/** Per-track DSP profiles layered over the global DSP configuration.
 * Profiles are keyed by the stable queue path (web://id or desktop path).
 * A missing profile deliberately falls back to the global configuration. */
import { invoke, platform } from "./invoke";
import { kvGet, kvPut } from "./library-db";
import { app } from "./state";
import type { DspConfig } from "./types";

const KEY = "dsp:profiles";
type Profiles = Record<string, DspConfig>;
let applyGeneration = 0;

function clone(config: DspConfig): DspConfig {
  return structuredClone(config);
}

export async function getDspProfiles(): Promise<Profiles> {
  if (platform !== "web") return {};
  try { return (await kvGet<Profiles>(KEY)) ?? {}; } catch { return {}; }
}

export async function saveDspProfile(trackId: string, config = app.dsp): Promise<void> {
  if (!trackId.trim() || !config) throw new Error("A track is required");
  const saved = clone(config);
  if (platform === "web") {
    const profiles = await getDspProfiles();
    profiles[trackId] = saved;
    await kvPut(KEY, profiles);
  } else {
    await invoke("set_track_dsp", { trackPath: trackId, config: saved });
  }
}

export async function resetDspProfile(trackId: string): Promise<void> {
  // Invalidate an in-flight profile read before changing storage. Without
  // this, a slow apply can overwrite the reset with the deleted profile.
  ++applyGeneration;
  if (platform === "web") {
    const profiles = await getDspProfiles();
    delete profiles[trackId];
    await kvPut(KEY, profiles);
  } else {
    await invoke("clear_track_dsp", { trackPath: trackId });
  }
  if (app.dsp && app.selectedPath === trackId) await invoke("set_dsp", { config: clone(app.dsp) });
}

/** Apply a profile, or the already-loaded global DSP when none exists. */
export async function applyDspProfile(trackId: string | null): Promise<void> {
  const generation = ++applyGeneration;
  if (!app.dsp) return;
  const profile = trackId ? (await getDspProfiles())[trackId] : undefined;
  if (generation !== applyGeneration || app.selectedPath !== trackId) return;
  // Keep app.dsp as the global fallback. Applying a track profile must not
  // replace it, otherwise the next track without a profile inherits the
  // previous track's settings instead of the user's global DSP.
  await invoke("set_dsp", { config: profile ? clone(profile) : clone(app.dsp) });
}

export function profileKeyForPath(path: string | null): string | null {
  return path?.trim() || null;
}
