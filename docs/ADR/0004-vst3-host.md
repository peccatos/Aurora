# ADR 0004: VST3 plugin host

Status: Accepted (Phase 9)

## Context

ROADMAP Phase 9 requires VST3 hosting. Constraints that shaped the design:

1. **RT safety (ARCHITECTURE.md invariant):** the audio callback performs
   bounded, preallocated work only. Third-party plugin code is the least
   trusted code in the process — it must never run in the callback.
2. **Crash containment (Phase 8 precedent):** a misbehaving plugin must not
   take the player down.
3. **Honesty (ADR 0002):** any active plugin is an `AudioTransformation`;
   bit-perfect exclusive mode is mutually exclusive with plugin processing.

## Decision

Use `vst3-host` 0.10 (MIT) — a Rust VST3 host library that provides exactly
the isolation architecture the roadmap demands:

- **Process isolation**: plugins load into a short-lived helper process
  (`vst3-host-helper`), communicated with over stdin/stdout. A plugin that
  aborts, segfaults or hangs dies in the helper; the host reports it and the
  player survives. Discovery uses a separate probe binary
  (`vst3-host-probe`) for the same reason.
- **Real-time bridge**: `RealtimePluginRunner::process` drains a lock-free
  bounded command queue plus the plugin render call — no locks a control
  thread could hold.

### Where plugins run

**NOT in the audio callback and NOT on the decode worker's hot chunk loop.**
Aurora adds a dedicated **plugin worker thread** (`aurora-vst`):

```
decode worker --ring--> [plugin worker: FIFO -> RealtimePluginRunner::process
                          (fixed 512-frame blocks) -> FIFO] --ring--> audio engine
```

The engine's DSP chunk path gains an optional VST stage between
`pipeline.process` and wire conversion. The plugin worker pulls decoded f32
samples, deinterleaves into per-channel `AudioBuffers`, runs the plugin
off-process at fixed block size, re-interleaves, and pushes on. Bounded FIFOs
cap latency at one block; the audio callback still only pops from the
SPSC ring.

Actually — simpler and equally honest: the VST stage runs **inline on the
existing decode worker** between `pipeline.process` and dither, with the
plugin itself out-of-process. The decode worker is already non-RT (it sleeps
in back-pressure loops), and plugin processing over the process boundary
is a blocking IPC call — acceptable on the worker, catastrophic in the
callback. One thread fewer, one FIFO fewer, the RT contract intact.

### Honesty contract

- A loaded+active plugin reports `AudioTransformation::ExternalPlugin`
  (new domain variant) and forces integrity to `Transformed` on the shared
  path; `play_exclusive` refuses while a plugin is active
  (`FormatUnsupported("plugin active")` — honest fallback, not a claim).
- Latency introduced by plugin `latency_samples()` (if reported) is an
  honest transformation too, surfaced in diagnostics.

### Helper binaries

`vst3-host-helper` and `vst3-host-probe` are workspace binaries
(`aurora-vst-helper`, aliased name for discovery: the crates build
`vst3-host-helper` bins via the dependency's `[[bin]]` — Cargo installs
dependency binaries into `target/<profile>/`, where the library's
`running_from_cargo_target` heuristic finds them; deployed builds ship the
helper next to the app exe, per the library's search order).

## Consequences

- Plugin GUI hosting is out of scope for this phase (parameters are exposed
  by ID/value; a GUI needs window embedding — later).
- Discovery scans standard VST3 paths (`C:\Program Files\Common Files\VST3`,
  `~/.vst3`) plus a user-configurable folder.
- Windows/macOS/Linux all supported by the library; we validate Windows live
  and others by CI compile.
