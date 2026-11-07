/** Platform invoke layer (ADR 0005 in practice).
 *
 * One `invoke(cmd, args)` shape, two implementations:
 *  - Tauri (desktop): @tauri-apps/api IPC to the command surface.
 *  - Web: the wasm WebKernel (aurora-web) called directly.
 *
 * Pages and state modules import from HERE, never from
 * @tauri-apps/api/core directly — that keeps every page shell-free.
 */

export type InvokeArgs = Record<string, unknown>;

// Detect the environment once. In the browser build __TAURI_INTERNALS__
// is absent; in the Tauri webview it is always present.
declare global {
  // eslint-disable-next-line no-var
  var __TAURI_INTERNALS__: unknown | undefined;
}

const isTauri = typeof globalThis.__TAURI_INTERNALS__ !== "undefined";
export const platform: "tauri" | "web" = isTauri ? "tauri" : "web";

// ---------------------------------------------------------------------------
// Tauri implementation
// ---------------------------------------------------------------------------

type TauriInvoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
let tauriInvoke: TauriInvoke | null = null;

async function loadTauriInvoke(): Promise<TauriInvoke> {
  const core = await import("@tauri-apps/api/core");
  return core.invoke as TauriInvoke;
}

// ---------------------------------------------------------------------------
// Web (wasm kernel) implementation
// ---------------------------------------------------------------------------

type WebKernel = {
  // Mirrors crates/aurora-web/src/lib.rs (mostly sync on the wasm
  // boundary; the invoke layer returns/awaits uniformly via unknown).
  new (): WebKernel;
  playback_status(): unknown;
  playback_position(): number;
  play_track(request: unknown): unknown;
  pause(): unknown;
  resume(): unknown;
  stop(): unknown;
  seek(position_s: number): unknown;
  position_correction_s(seconds: number): void;
  set_output_rate(rate: number): void;
  set_output_channels(channels: number): void;
  analyze_lufs(bytes: Uint8Array, extension: string): number | null;
  set_queue(tracks: unknown): unknown;
  get_queue(): unknown;
  play_queue_index(index: number): unknown;
  next_track(): unknown;
  prev_track(): unknown;
  get_dsp(): unknown;
  set_dsp(config: unknown): unknown;
  playback_spectrum(): unknown;
  set_loop(start: number, end: number): unknown;
  deck_load(deck: number, path: string, title: string | null, artist: string | null): unknown;
  deck_play(deck: number): unknown;
  deck_stop(deck: number): unknown;
  deck_seek(deck: number, position_s: number): void;
  deck_fader(deck: number, value: number): unknown;
  set_crossfader(value: number): unknown;
  deck_set_cue(deck: number, slot: number, position: number | null): unknown;
  deck_jump_cue(deck: number, slot: number): void;
  deck_tempo(deck: number, speed: number): unknown;
  deck_status(): unknown;
  track_bpm(path: string): number | null;
  library_status(): unknown;
  probe_exclusive(): unknown;
  subscribe(callback: (payload: string) => void): number;
  // web output bridge
  web_pull(maxBytes: number): Float32Array;
  register_file(id: string, bytes: Uint8Array, extension: string): void;
};

let kernelPromise: Promise<WebKernel> | null = null;

async function loadKernel(): Promise<WebKernel> {
  if (!kernelPromise) {
    kernelPromise = (async () => {
      // Vite resolves the wasm-bindgen glue via the `?init` module
      // produced by wasm-pack. The pkg name: aurora-web.
      // (wasm-pack output is copied into src/wasm/ during build.)
      const mod = await import("./wasm/aurora_web");
      await mod.default();
      const kernel = new mod.WebKernel() as unknown as WebKernel;
      // Device rate BEFORE the first play: the engine's negotiator must
      // know the AudioContext rate when play_queue_index runs, or 44.1k
      // files pass through at 44.1k and the BROWSER resamples each
      // buffer independently (phase-discontinuous "jumping" audio).
      // Creating an AudioContext here is allowed (it starts suspended);
      // playback unlocks it from the user gesture later.
      try {
        const probe = new AudioContext();
        kernel.set_output_rate(probe.sampleRate);
        try {
          kernel.set_output_channels(2); // browser outputs are stereo
        } catch {
          /* older binary — rate alone still fixes resampling */
        }
        void probe.close();
      } catch {
        // No audio yet (rare): the engine falls back to source-rate
        // passthrough and the bridge keeps working.
      }
      return kernel;
    })();
  }
  return kernelPromise;
}

/** Map wasm kernel commands to WebKernel methods. */
async function webInvoke<T>(cmd: string, args: InvokeArgs): Promise<T> {
  const k = await loadKernel();
  switch (cmd) {
    case "playback_status":
      return k.playback_status() as unknown as Promise<T>;
    case "playback_position":
      return k.playback_position() as unknown as Promise<T>;
    case "play_track":
      return k.play_track(args.request) as unknown as Promise<T>;
    case "pause_playback":
      return k.pause() as unknown as Promise<T>;
    case "resume_playback":
      return k.resume() as unknown as Promise<T>;
    case "stop_playback":
      return k.stop() as unknown as Promise<T>;
    case "seek":
      return k.seek(args.positionS as number) as unknown as Promise<T>;
    case "position_correction_s":
      return k.position_correction_s(args.seconds as number) as unknown as T;
    case "set_queue":
      return k.set_queue(args.tracks) as unknown as Promise<T>;
    case "get_queue":
      return k.get_queue() as unknown as Promise<T>;
    case "queue_add_tracks":
      // Web kernel: read-modify-write through the queue DTO.
      {
        const q = (await k.get_queue()) as { tracks: unknown[]; current: number | null };
        const add = args.tracks as unknown[];
        const tracks = [...q.tracks, ...add];
        return k.set_queue(tracks) as unknown as Promise<T>;
      }
    case "play_queue_index":
      return k.play_queue_index(args.index as number) as unknown as Promise<T>;
    case "next_track":
      return k.next_track() as unknown as Promise<T>;
    case "prev_track":
      return k.prev_track() as unknown as Promise<T>;
    case "get_dsp":
      return k.get_dsp() as unknown as Promise<T>;
    case "set_dsp":
      return k.set_dsp(args.config) as unknown as Promise<T>;
    case "playback_spectrum":
      return k.playback_spectrum() as unknown as Promise<T>;
    case "set_loop":
      return k.set_loop(
        (args.loopStart as number | null) ?? 0,
        (args.loopEnd as number | null) ?? 0,
      ) as unknown as Promise<T>;
    case "deck_load":
      return k.deck_load(
        args.deck as number,
        args.path as string,
        (args.title as string | null) ?? null,
        (args.artist as string | null) ?? null,
      ) as unknown as Promise<T>;
    case "deck_play":
      return k.deck_play(args.deck as number) as unknown as Promise<T>;
    case "deck_stop":
      return k.deck_stop(args.deck as number) as unknown as Promise<T>;
    case "deck_seek":
      return k.deck_seek(args.deck as number, args.positionS as number) as unknown as Promise<T>;
    case "deck_fader":
      return k.deck_fader(args.deck as number, args.value as number) as unknown as Promise<T>;
    case "set_crossfader":
      return k.set_crossfader(args.value as number) as unknown as Promise<T>;
    case "deck_set_cue":
      return k.deck_set_cue(
        args.deck as number,
        args.slot as number,
        (args.position as number | null) ?? null,
      ) as unknown as Promise<T>;
    case "deck_jump_cue":
      return k.deck_jump_cue(args.deck as number, args.slot as number) as unknown as Promise<T>;
    case "deck_tempo":
      return k.deck_tempo(args.deck as number, args.speed as number) as unknown as Promise<T>;
    case "deck_status":
      return k.deck_status() as unknown as Promise<T>;
    case "track_bpm":
      return k.track_bpm(args.path as string) as unknown as Promise<T>;
    case "library_status":
      return k.library_status() as unknown as Promise<T>;
    case "probe_exclusive":
      return k.probe_exclusive() as unknown as Promise<T>;
    case "app_version":
      return "0.2.0-web" as T;
    case "app_name":
      return "Aurora Web" as T;
    case "list_output_devices":
      // The browser has exactly one output — the kernel's resolve_device
      // returns the same "web:default" the device selector expects.
      return [{ key: "web:default", name: "Browser output" }] as T;
    default:
      throw new Error(`invoke: unsupported command in web build: ${cmd}`);
  }
}

// ---------------------------------------------------------------------------
// Public surface
// ---------------------------------------------------------------------------

export async function invoke<T = unknown>(cmd: string, args: InvokeArgs = {}): Promise<T> {
  if (platform === "web") {
    return webInvoke<T>(cmd, args);
  }
  if (!tauriInvoke) {
    tauriInvoke = await loadTauriInvoke();
  }
  return tauriInvoke<T>(cmd, args);
}

/** Kernel event subscription (desktop: Tauri events; web: callback). */
export async function onKernelEvent(
  event: string,
  handler: (payload: unknown) => void,
): Promise<void> {
  if (platform === "web") {
    const k = await loadKernel();
    k.subscribe((raw: string) => {
      try {
        const parsed = JSON.parse(raw) as { type?: string };
        if (parsed.type === "LibraryChanged" || event === "*") {
          handler(JSON.parse(raw));
        }
      } catch {
        /* malformed event */
      }
    });
    return;
  }
  const { listen } = await import("@tauri-apps/api/event");
  await listen(event, (e) => handler(e.payload));
}

/** The raw web kernel handle (web build only) — for the AudioWorklet
 *  bridge and future web-only APIs. */
export async function webKernel(): Promise<WebKernel | null> {
  if (platform !== "web") return null;
  return loadKernel();
}

/** Register a browser File for kernel playback (web build only).
 *  Returns the "web://<id>" path the queue should carry. */
export async function registerWebFile(file: File): Promise<string> {
  if (platform !== "web") throw new Error("registerWebFile: web build only");
  const k = await loadKernel();
  const id = crypto.randomUUID();
  const ext = (file.name.split(".").pop() ?? "").toLowerCase();
  const bytes = new Uint8Array(await file.arrayBuffer());
  k.register_file(id, bytes, ext);
  return `web://${id}`;
}

/** Register raw bytes under a STABLE id (restored-from-IndexedDB tracks:
 *  the id persists across reloads, so queue paths stay valid).
 *  Returns the "web://<id>" path. */
export async function registerWebBytes(
  id: string,
  bytes: Uint8Array,
  extension: string,
): Promise<string> {
  if (platform !== "web") throw new Error("registerWebBytes: web build only");
  const k = await loadKernel();
  k.register_file(id, bytes, extension);
  return `web://${id}`;
}

/** R128 loudness measurement (W8): integrated LUFS of the track's
 *  bytes, computed by the kernel (same decode path as playback). None
 *  when unmeasurable. JS holds no DSP — the measurement is honest
 *  because it measures exactly what will play. */
export async function analyzeLufs(bytes: Uint8Array, extension: string): Promise<number | null> {
  if (platform !== "web") return null;
  const k = await loadKernel();
  try {
    return k.analyze_lufs(bytes, extension) ?? null;
  } catch {
    return null; // honest: no measurement, no normalization
  }
}
