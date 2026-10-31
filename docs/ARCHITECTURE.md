# Architecture

## Dependency direction

`aurora-core` is the dependency root. It must not depend on Tauri, SQLite,
CPAL, filesystem scanners, or UI code.

- Decoder and DSP depend on core.
- Audio composes decoder, DSP and CPAL output.
- Metadata and storage depend on core.
- Library composes metadata and storage.
- Tauri adapter exposes application use cases to the frontend.
- App is the composition root.
- Desktop owns the Tauri executable and generated application context.

## Invariants

1. The audio callback performs bounded, preallocated work only.
2. No file, network, database, or logging operations run in the callback.
3. Decoder workers produce audio frames ahead of playback.
4. DSP is explicitly bypassable.
5. Bit-perfect status is a verified runtime state, never a marketing assumption.
6. SQLite access is isolated in `aurora-storage`.
7. Frontend state is a projection of backend state; it is not authoritative.
8. Device loss is recoverable and must not panic the process.
9. Every schema change is a migration.
10. Cross-platform behavior is tested on Windows, Linux and macOS.

## Audio modes

- `BitPerfect`: permitted only when all transformations are disabled and the
  selected output path is verified compatible with the source representation.
- `Dsp`: explicit processing path. The diagnostics must enumerate conversions
  and active processors.
- `Auto`: may select a compatible path, but must expose why it selected it.

A CPAL stream alone does not guarantee exclusive or bit-perfect output.
