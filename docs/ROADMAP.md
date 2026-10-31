# Roadmap

## Sprint 0 — Foundation
- Workspace and crate boundaries.
- Tauri desktop shell and frontend bootstrap.
- CI on Windows, Linux and macOS.
- Architecture and audio-path decisions.

Exit: metadata, check, test and lint commands pass on all CI targets.

## Phase 1 — Domain core
- Track, Artist, Album, AudioFormat, PlaybackState and PlaybackMode.
- Stable IDs and serialization contracts.

Exit: unit tests and roundtrip-serialization tests.

## Phase 2 — Decoder
- Symphonia-backed decoding.
- Typed PCM frame representation.
- Corrupt/truncated input handling.

Exit: fixtures decode with correct duration, rate, channels and sample count.

## Phase 3 — Playback
- CPAL device discovery and stream lifecycle.
- Worker-to-callback SPSC buffering.
- Play, pause, stop and device-loss recovery.

Exit: long-duration playback tests with observable underrun metrics.

## Phase 4 — DSP
- Gain, EQ, ReplayGain and resampling.
- Explicit bypass and processor ordering.
- Headroom and clipping tests.

Exit: bypass identity tests and signal-quality regression tests.

## Phase 5 — Output integrity
- Platform-specific output capabilities.
- Negotiation and diagnostics.
- Digital loopback / capture verification where available.

Exit: verified status per tested platform/device path. Unsupported paths report
unverified rather than claiming bit-perfect playback.

## Phase 6 — Library and storage
- SQLite migrations and repositories.
- Scanning, metadata extraction, playlists and search.
- Rescan idempotency and file-change detection.

Exit: repeat scans create no duplicate tracks and recover after interruption.

## Phase 7 — Desktop product
- Queue, library, playback controls, device selection and settings.
- Typed frontend API and event handling.

Exit: UI is a thin client of backend state.

## Phase 8 — Production hardening
- Crash recovery, packaging, signing, release automation and performance tests.

Exit: reproducible release artifacts and documented platform limitations.

## Phase 9 (post-1.0) — VST3 plugin host

Deferred until the core engine (Phases 4-7) is stable. Deliberately planned
late: hosting third-party plugins is a large, semi-independent subsystem.

Scope when started:
- VST3 (open SDK, MIT-licensed interface headers; the modern standard).
  VST2 is out of scope permanently: closed SDK, licenses no longer issued,
  legal risk.
- Host runs plugins out-of-process or sandboxed: a crashing plugin must not
  take the player down, and untrusted plugin code must not run in-process
  with our real-time guarantees.
- Real-time bridge: plugin processing happens on the DSP worker thread with
  bounded block sizes; the audio callback never calls into plugin code.
- Any active plugin is an `AudioTransformation` (ADR 0002): bit-perfect mode
  is mutually exclusive with plugin processing, reported honestly.
- ADR to be written before implementation starts, covering process isolation
  strategy, licensing obligations, and the block-size/latency contract.
