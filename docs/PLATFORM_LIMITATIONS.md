# Platform limitations

_This document is part of Phase 8 (production hardening). It states, per
platform, what Aurora can and cannot honestly do — aligned with ADR 0002
(unverified paths are never reported as bit-perfect) and ADR 0003
(library identity). It must be updated whenever a platform's capability
changes._

## Output path / bit-perfect

| Platform | Shared mode | Exclusive mode | Verification |
|---|---|---|---|
| Windows (WASAPI) | Works; OS mixer resamples/mixes → `Transformed`/`Unverified`, never claimed bit-perfect | Probe implemented (Phase 5): Realtek-class devices accept 16/24-bit 44.1/48/96k. Full exclusive streaming and loopback capture are the next layer | `unverified` until loopback capture proves identity |
| Linux (ALSA via CPAL) | Works; dmix path resamples | Not probed | `unverified` |
| macOS (CoreAudio) | Works; AudioConverter inserts transformations when formats mismatch | Not applicable the WASAPI way | `unverified` |

Consequences for users:

- The integrity badge in the UI reflects the *measured* chain. On all
  platforms today the honest ceiling in shared mode is `unverified` or
  `transformed` — by design, not by omission.
- Device loss (USB unplugged, Bluetooth disconnect): playback faults are
  caught (Phase 8), the stream is stopped, and "Recover & Replay" restarts
  the current track on the default device. The process never panics.

## DSP

- Reverb (Freeverb) is stereo-only: mono sources pass through without
  reverb (reported as not-applied, not as an error).
- ReplayGain applies track/album tags when present (Phase 6+); files
  without tags play at unity — no fake normalization.
- Speed (0.5–2.0x) engages the resampler; combinations of speed + device
  rate mismatch are unit-tested but see fewer real-device runs.

## Library

- Track identity is content-derived (blake3): two byte-identical files
  are one library entry; the path points to the most recently scanned
  copy. A moved file updates its path on rescan (no duplicate).
- Scans are idempotent and resumable: a scan interrupted by crash/quit
  leaves the database consistent; the next scan completes the work.
- Corrupt/unreadable files are skipped and counted (`failed` in the scan
  summary), never fatal to a scan.
- SQLite runs in WAL with `synchronous=NORMAL`: a power cut may lose the
  last transaction, which the next scan repairs. Clean shutdown
  (window close) checkpoints the WAL explicitly.

## Known gaps (post-1.0 candidates)

- Exclusive-mode full streaming and physical loopback verification
  (upgrades `unverified` → `verified` where the hardware allows).
- VST3 hosting (Phase 9): out-of-process, never in the audio callback.
- Release artifacts: CI bundles installers per OS (draft GitHub release).
  Code-signing and notarization require the maintainer's secrets
  (`TAURI_SIGNING_PRIVATE_KEY`, Apple notarization credentials); without
  them, artifacts are unsigned and macOS GateKeeper will warn.

## MSRV

Rust 1.90 (enforced in CI on the MSRV job). Dependencies are audited via
`cargo audit` in CI (Sprint 0 setup).
