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

## Web (Aurora Web, wasm) — known limitations

- **Background-tab stutter (architectural)**: the output bridge is a
  `setTimeout(50ms)` pull loop; browsers clamp background-tab timers to
  ~1s while only ~200ms of audio is scheduled ahead — hidden tabs get
  dry windows (gaps). The AudioWorklet upgrade (needs
  SharedArrayBuffer + COOP/COEP headers) is the designed fix and a
  drop-in for the ring drain.
- **Latency**: ~200ms scheduling slack by design (honest, displayed);
  native builds have no such gap.
- **Decks are native-only**: the wasm bridge drains only the main
  engine's ring; deck engines have no consumer on the web, so the Decks
  route is hidden (a "playing" deck with no sound would be a dishonest
  status per ADR 0002). A ring mixer (main + 2 decks + crossfader) is
  the planned unblock.
- **No SQLite/library**: the wasm kernel has no storage crate; the JS
  shell owns IndexedDB (tracks + DSP settings). Bit-perfect exclusive
  and loopback verification honestly report N/A here — the browser
  audio path cannot be measured from wasm.
- **Position honesty**: the engine counts frames handed to the bridge,
  which schedules ~200ms ahead of the audible clock; pause reports the
  unheard tail (position correction, ADR 0002), and the auto-advance
  threshold compensates for the schedule window.
- **Deezer catalog**: api.deezer.com sends no CORS headers — its
  results arrive only via CORS-friendly networks/proxies; iTunes Search
  is the reliable browser-side source.
