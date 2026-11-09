/* tslint:disable */
/* eslint-disable */

/**
 * The web shell's kernel handle: one per page.
 */
export class WebKernel {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * R128 loudness measurement (W8): integrated LUFS of decoded
     * audio bytes. The JS shell runs this at ingest and stores the
     * value; playback feeds it through the existing ReplayGain gain
     * stage (mode "track") — normalized loudness with zero new
     * audio-path code. Returns None honestly when the audio is too
     * short/quiet to measure.
     */
    analyze_lufs(bytes: Uint8Array | number[], extension: string): number | undefined;
    deck_fader(deck: number, value: number): any;
    deck_jump_cue(deck: number, slot: number): void;
    deck_load(deck: number, path: string, title?: string | null, artist?: string | null): Promise<any>;
    deck_play(deck: number): any;
    deck_seek(deck: number, position_s: number): void;
    deck_set_cue(deck: number, slot: number, position?: number | null): any;
    deck_status(): any;
    deck_stop(deck: number): any;
    deck_tempo(deck: number, speed: number): any;
    get_dsp(): any;
    get_queue(): any;
    library_status(): Promise<any>;
    constructor();
    next_track(): any | undefined;
    output_devices(): any;
    pause(): any;
    play_queue_index(index: number): any;
    play_track(request: any): any;
    playback_position(): number;
    playback_spectrum(): any;
    playback_status(): any;
    /**
     * Bridge honesty report (ADR 0002): seconds of scheduled-but-unheard
     * audio the browser dropped (pause/track change killed scheduled
     * nodes). Negative values roll the position back to the audible
     * truth; the seek rebase covers jumps.
     */
    position_correction_s(seconds: number): void;
    prev_track(): any;
    probe_exclusive(): any;
    /**
     * The queue snapshot DTO (for shells that need the raw shape).
     */
    queue_dto(queue: any): any;
    /**
     * Register a file for playback: `id` + bytes + extension (no dot).
     * The queue uses "web://<id>" as the path; the decoder resolves it
     * through the registry (the browser has no filesystem).
     *
     * Re-registering is the normal flow for replays — the registry
     * consumes entries on play.
     */
    register_file(id: string, bytes: Uint8Array, extension: string): void;
    /**
     * Remove a user convolution IR by id.
     */
    remove_convolution_ir(id: number): void;
    resume(): any;
    seek(position_s: number): any;
    /**
     * Register a user convolution-reverb impulse response (web IR
     * upload): f32 planar channels + source sample rate. The right
     * channel may be an empty array (mono upmix happens in Rust).
     * JS hands over plain `number[]` or Float32Array contents — copy
     * into wasm memory once; the IR store is global to the wasm module.
     */
    set_convolution_ir(id: number, left: Float32Array | number[], right: Float32Array | number[], sample_rate: number): void;
    set_crossfader(value: number): any;
    set_dsp(config: any): any;
    set_loop(start: number, end: number): any;
    /**
     * The browser output channel count (W7 gapless): with it, the
     * engine remixes mono<->stereo itself, so a mixed-channel queue
     * never changes the bridge's stream format (a format change would
     * restart the loop = an audible gap between tracks).
     */
    set_output_channels(channels: number): void;
    /**
     * Tell the engine the browser's output sample rate (AudioContext.
     * sampleRate). With it, playback resamples to the DEVICE rate in the
     * engine's DSP pipeline (one continuous stream, reported honestly);
     * without it the browser resamples each buffer independently —
     * phase-discontinuous at chunk boundaries ("frequencies jumping").
     * Call once at app start, before the first play.
     */
    set_output_rate(rate: number): void;
    set_queue(tracks: any): any;
    stop(): any;
    /**
     * Subscribe to kernel events. The callback receives the serialized
     * `KernelEvent` (same wire shape as the Tauri event payloads).
     */
    subscribe(callback: Function): number;
    track_bpm(path: string): Promise<number | undefined>;
    /**
     * Pull decoded+processed samples from the engine ring as f32
     * interleaved. Returns an empty array when nothing is playing or
     * the ring ran dry (the worklet fills silence then).
     *
     * The engine pushes wire-format bytes (its negotiated f32 target).
     * Allocation discipline: the samples live in wasm memory; the view
     * (`Float32Array::view`) borrows that memory with ZERO copying and
     * zero JS-side allocation — the pull loop runs ~20x/s and the old
     * copy-into-fresh-array form was measurable GC pressure. The
     * buffer is only borrowed for the duration of the synchronous
     * call; JS copies into the AudioBuffer immediately.
     */
    web_pull(max_bytes: number): Float32Array;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_webkernel_free: (a: number, b: number) => void;
    readonly webkernel_analyze_lufs: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly webkernel_deck_fader: (a: number, b: number, c: number, d: number) => void;
    readonly webkernel_deck_jump_cue: (a: number, b: number, c: number, d: number) => void;
    readonly webkernel_deck_load: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => any;
    readonly webkernel_deck_play: (a: number, b: number, c: number) => void;
    readonly webkernel_deck_seek: (a: number, b: number, c: number, d: number) => void;
    readonly webkernel_deck_set_cue: (a: number, b: number, c: number, d: number, e: number, f: number) => void;
    readonly webkernel_deck_status: (a: number, b: number) => void;
    readonly webkernel_deck_stop: (a: number, b: number, c: number) => void;
    readonly webkernel_deck_tempo: (a: number, b: number, c: number, d: number) => void;
    readonly webkernel_get_dsp: (a: number, b: number) => void;
    readonly webkernel_get_queue: (a: number, b: number) => void;
    readonly webkernel_library_status: (a: number) => any;
    readonly webkernel_new: () => number;
    readonly webkernel_next_track: (a: number, b: number) => void;
    readonly webkernel_output_devices: (a: number, b: number) => void;
    readonly webkernel_pause: (a: number, b: number) => void;
    readonly webkernel_play_queue_index: (a: number, b: number, c: number) => void;
    readonly webkernel_play_track: (a: number, b: number, c: any) => void;
    readonly webkernel_playback_position: (a: number, b: number) => void;
    readonly webkernel_playback_spectrum: (a: number, b: number) => void;
    readonly webkernel_playback_status: (a: number, b: number) => void;
    readonly webkernel_position_correction_s: (a: number, b: number, c: number) => void;
    readonly webkernel_prev_track: (a: number, b: number) => void;
    readonly webkernel_probe_exclusive: (a: number, b: number) => void;
    readonly webkernel_queue_dto: (a: number, b: number, c: any) => void;
    readonly webkernel_register_file: (a: number, b: number, c: number, d: any, e: number, f: number) => void;
    readonly webkernel_remove_convolution_ir: (a: number, b: number, c: number) => void;
    readonly webkernel_resume: (a: number, b: number) => void;
    readonly webkernel_seek: (a: number, b: number, c: number) => void;
    readonly webkernel_set_convolution_ir: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
    readonly webkernel_set_crossfader: (a: number, b: number, c: number) => void;
    readonly webkernel_set_dsp: (a: number, b: number, c: any) => void;
    readonly webkernel_set_loop: (a: number, b: number, c: number, d: number) => void;
    readonly webkernel_set_output_channels: (a: number, b: number) => void;
    readonly webkernel_set_output_rate: (a: number, b: number) => void;
    readonly webkernel_set_queue: (a: number, b: number, c: any) => void;
    readonly webkernel_stop: (a: number, b: number) => void;
    readonly webkernel_subscribe: (a: number, b: any) => number;
    readonly webkernel_track_bpm: (a: number, b: number, c: number) => any;
    readonly webkernel_web_pull: (a: number, b: number) => any;
    readonly __wasm_bindgen_func_elem_283: (a: number, b: number, c: number, d: any) => void;
    readonly __wasm_bindgen_func_elem_300: (a: number, b: number, c: any, d: any) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number, b: number, c: number) => void;
    readonly __wbindgen_export4: (a: number) => void;
    readonly __wbindgen_export5: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_export6: (a: number, b: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export7: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
