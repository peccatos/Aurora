# ADR 0005: Kernel / shell split

Status: Accepted (Phase 12)

## Context

Aurora grew a feature wave (decks, library watch, playlist import, BPM)
inside the Tauri adapter. `aurora-tauri/commands.rs` reached 2200+ lines
of orchestration — queue advance logic, deck state machines, import
matching, tempo measurement, the folder watcher. That code is the music
player; Tauri is just one way to talk to it.

Two concrete pressures force the split now:

1. **A web player** (planned): the same engine must serve a browser
   shell. If the logic lives behind `#[tauri::command]`, a web shell
   would have to either re-implement it or drag Tauri along — both wrong.
2. **Third-party players on Aurora**: the goal is that another
   application, under another name, can be built *on* Aurora — with
   Aurora as the musical kernel, not a library they fork. That requires
   a published, Tauri-free API boundary.

## Decision

Extract `aurora-kernel`: the music kernel, depending only on the audio
crates (`aurora-core`, `aurora-decoder`, `aurora-dsp`, `aurora-vst`,
`aurora-audio`, `aurora-metadata`, `aurora-storage`, `aurora-library`)
and tokio. It has **zero knowledge of Tauri, HTTP, or any UI toolkit**.

### Kernel surface

- `PlayerKernel` — the state that used to be `AppState`: main engine,
  DJ decks + crossfader, queue, DSP config, VST chain, library, output
  device selection, settings.
- Orchestration **methods** with domain DTOs as inputs/outputs:
  `deck_load`, `advance_queue`, `import_playlist`, `measure_bpm`,
  `start_library_watcher`, … — everything the adapter used to compute
  inline.
- `KernelEvent` — an enum (`LibraryChanged { added, pruned }`,
  `PlaybackChanged`, …) plus a subscription mechanism. Shells subscribe
  and project; they never poll kernel internals.

### Shells

- `aurora-tauri` becomes a thin adapter: one `#[tauri::command]` = one
  kernel call + DTO serialization; kernel events forwarded as Tauri
  events. No business logic.
- `aurora-app` composes kernel + Tauri shell for the desktop binary.
- A future web shell composes the same kernel with a wasm/HTTP surface.

### Honesty contracts travel with the kernel

Bit-perfect verification (ADR 0002), transformation reporting, and the
"measured, never fabricated" rule for BPM are kernel properties. A shell
can only *display* them — it cannot claim, weaken or bypass them. This
is the point of the split: a player built on Aurora inherits the
honesty, not just the plumbing.

## Consequences

- Deck/queue/import logic moves out of `commands.rs`; the adapter file
  shrinks to translation only.
- Cross-shell duplication is structurally impossible: two shells cannot
  drift apart on "what does next_track do" — there is one answer, in
  the kernel.
- The kernel API is the seam for the web version (ADR to follow when
  the web shell work starts): wasm bindings wrap `PlayerKernel`, the
  browser's unprovable audio path is reported honestly (`bit-perfect:
  N/A (browser)`) rather than faked.
- `audio callback` invariants (ARCHITECTURE.md 1–2) are unchanged: the
  kernel owns the engine and keeps the RT contract.
