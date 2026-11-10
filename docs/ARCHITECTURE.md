# Architecture

## Dependency direction

`aurora-core` is the dependency root. It must not depend on Tauri, SQLite,
CPAL, filesystem scanners, or UI code.

- Decoder and DSP depend on core.
- Audio composes decoder, DSP and CPAL output.
- Metadata and storage depend on core.
- Library composes metadata and storage.
- **Kernel** (ADR 0005) composes the audio crates into the music player —
  engine, decks, queue, VST chain, library orchestration, events. It has no
  UI/Tauri/HTTP dependencies; shells (Tauri, future web, third-party players)
  call it and translate its DTOs.
- Tauri adapter is a thin shell: one command = one kernel call + DTO
  translation; kernel events forward to the frontend.
- App is the composition root (kernel + Tauri shell).
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
11. Business logic lives in the kernel, never in a shell (ADR 0005): two
    shells cannot drift apart on what `next_track` means — there is one
    answer, in the kernel.

## Audio modes

- `BitPerfect`: permitted only when all transformations are disabled and the
  selected output path is verified compatible with the source representation.
- `Dsp`: explicit processing path. The diagnostics must enumerate conversions
  and active processors.
- `Auto`: may select a compatible path, but must expose why it selected it.

A CPAL stream alone does not guarantee exclusive or bit-perfect output.

## Deployment & server surface (2026-11)

- **`aurora-api`** (Axum, single-origin on Render): serves the web shell
  from `/` (SPA fallback) plus `/api/oauth/*`, `/api/media/*`, `/api/auth/*`.
  Cookies are first-party because there is one origin: 8h session (CSRF),
  180d anonymous user, 30d login session. OAuth grants in `oauth_accounts`
  are keyed by `user_hash` (anonymous cookie hash, or `user:{id}` when
  logged in) — see ADR 0006 and the README Deployment section.
- **`aurora-providers`** remains the provider-neutral OAuth/playlist
  boundary used by both the desktop adapter and `aurora-api`.
- **`aurora-loudness`**: EBU R128 (LUFS) measurement; writes results into
  the existing `replaygain_track_db` column.
