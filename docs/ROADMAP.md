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

Exit: unit tests and serialization round-trip tests.

## Phase 2 — Decoder
- Symphonia-backed decoding.
- Typed PCM frame representation.
- Corrupt/truncated input handling.

Exit: fixtures decode with correct duration, rate, channels and sample count.

## Phase 3 — Playback
- CPAL device discovery and stream lifecycle.
- Worker-to-callback SPSC buffering.
- Play, pause, stop, seek and device-loss recovery.

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
