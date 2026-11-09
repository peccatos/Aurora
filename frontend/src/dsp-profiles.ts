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

const defaultCrossfeed = () => ({ enabled: false, cutoff_hz: 700, feed_db: -6.5 });

const defaultConvolution = () => ({
  enabled: false,
  ir_id: 0,
  wet: 0.3,
  dry: 0.8,
  predelay_ms: 0,
  ir_gain_db: 0,
});

/** Legacy profiles (pre-convolution) round-trip with disabled defaults. */
function ensureConvolution(d: DspConfig): DspConfig {
  d.convolution ??= defaultConvolution();
  d.convolution.enabled = Boolean(d.convolution.enabled);
  d.convolution.ir_id = Number.isFinite(d.convolution.ir_id)
    ? Math.max(0, Math.trunc(d.convolution.ir_id))
    : 0;
  const clamp01 = (v: number, fb: number) =>
    Number.isFinite(v) ? Math.max(0, Math.min(1, v)) : fb;
  d.convolution.wet = clamp01(d.convolution.wet, 0.3);
  d.convolution.dry = clamp01(d.convolution.dry, 0.8);
  d.convolution.predelay_ms = Number.isFinite(d.convolution.predelay_ms)
    ? Math.max(0, Math.min(250, d.convolution.predelay_ms))
    : 0;
  d.convolution.ir_gain_db = Number.isFinite(d.convolution.ir_gain_db)
    ? Math.max(-24, Math.min(12, d.convolution.ir_gain_db))
    : 0;
  return d;
}

/** Legacy profiles (pre-crossfeed) round-trip with disabled defaults. */
function ensureCrossfeed(d: DspConfig): DspConfig {
  d.crossfeed ??= defaultCrossfeed();
  d.crossfeed.enabled = Boolean(d.crossfeed.enabled);
  d.crossfeed.cutoff_hz = Number.isFinite(d.crossfeed.cutoff_hz)
    ? Math.max(20, Math.min(5000, d.crossfeed.cutoff_hz))
    : 700;
  d.crossfeed.feed_db = Number.isFinite(d.crossfeed.feed_db)
    ? Math.max(-24, Math.min(0, d.crossfeed.feed_db))
    : -6.5;
  return d;
}

export async function getDspProfiles(): Promise<Profiles> {
  try {
    let profiles: Profiles = {};
    if (platform === "web") {
      profiles = (await kvGet<Profiles>(KEY)) ?? {};
    } else {
      const raw = await invoke<string | null>("get_setting_cmd", { key: "dsp.track.v1" });
      profiles = raw ? (JSON.parse(raw) as Profiles) : {};
    }
    Object.values(profiles).forEach((d) => ensureConvolution(ensureCrossfeed(d)));
    return profiles;
  } catch { return {}; }
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
