# Aurora

A local-first music player you can trust. Rust + Tauri 2 + TypeScript, Windows/Linux/macOS.

Aurora's core idea (ADR 0002): **bit-perfect is a measured property, not a claim.**
Every transformation in the signal path is reported honestly — and when the path
is clean, physical loopback verification can prove it byte-for-byte.

## Kernel / shells (ADR 0005)

The music player itself lives in `aurora-kernel`: a shell-free crate with zero
Tauri/UI dependencies. The desktop app is one shell over it; a web shell (and
any third-party player built on Aurora) composes the same kernel with its own
transport. Honesty contracts (bit-perfect verdicts, transformation reports,
"measured, never fabricated" BPM) travel with the kernel — a shell can display
them, never fake them.

## What it does

- **Playback**: FLAC/WAV/MP3/M4A/AAC/OGG/Opus/AIFF/WavPack via Symphonia; gapless
  queue with auto-advance, seek, shuffle, drag-n-drop reorder.
- **Bit-perfect mode** (Windows): WASAPI exclusive output — no mixer, no DSP,
  exact source bytes. **Verify** captures the endpoint's rendered loopback and
  compares it against the reference wire bytes: a real measurement, with a
  percentage and frame count.
- **Honest DSP**: 10-band parametric EQ, Freeverb reverb, speed, ReplayGain —
  all live-recompilable without stopping the stream. Flat EQ = bit-exact
  passthrough, and the signal-path report says exactly that.
- **VST3 hosting** (ADR 0004): out-of-process plugin chain. Plugins load into
  isolated helper processes — a crashing plugin never takes the player down.
  Per-slot bypass (instant A/B), live parameter panels, presets, chain
  persistence across restarts. Editor GUIs run in their own process too.
- **DJ decks** (Pioneer DDJ-1000-style): two independent playback streams, each
  with its own engine, jog waveform, 8 hot cues, tempo and channel fader;
  equal-power crossfader for live mixing.
- **Measured BPM**: the decks know each track's tempo — detected from the
  content (onset envelope + autocorrelation), persisted per track, shown as
  native and effective (× tempo) values. No confident beat = honestly `--`.
- **Library**: SQLite (WAL) with content-hash idempotent rescans, search,
  playlists, embedded cover art — plus a **folder watcher**: added/removed
  files rescan automatically and the UI updates in place.
- **Playlists in/out**: export as M3U8 (universal); import M3U/M3U8, PLS,
  Spotify JSON exports and CSV (Last.fm, Apple Music, generic) — service
  lists match the local library by title/artist, with an honest
  matched/unmatched report.
- **Remote playlists via OAuth (Spotify / YouTube Music)**: a server-side
  backend-for-frontend (`aurora-api`) performs the authorization-code + PKCE
  exchange, so client secrets and tokens never reach the browser. Tokens are
  AES-256-GCM encrypted at rest, refresh/rotate automatically, and a
  per-session CSRF token plus rate limiting guard every state-changing call.
  Remote playlists import into the local library through the same
  matched/unmatched report as file imports.
- **yt-dlp media sidecar (metadata + authorized downloads)**: shipped as a
  Tauri external binary. Metadata lookups and downloads are restricted to
  HTTPS sources on an allowlist (archive.org, youtube.com, youtu.be); anything
  else is refused before the process is spawned. No DRM/protected-stream
  extraction — refused by design.
- **Player page**: waveform seek bar, real-time 16-band spectrum (true FFT of
  what the wire carries), A/B loop, full signal-path diagnostics.
- **System media keys** work globally (keyboard, Bluetooth headphones, remotes).

## Architecture

```
aurora-core      domain types; AudioTransformation; integrity contracts
aurora-decoder   Symphonia decoding; native format detection (F32 MP3/FLAC!);
                 waveform peaks; measured tempo (BPM)
aurora-dsp       EQ/Freeverb/gain/TPDF-dither/resampler/speed; live recompilation
aurora-vst       VST3 hosting: scan/load/chain, editor GUI (own process),
                 probe instances for params/presets
aurora-audio     engine: worker -> SPSC ring -> CPAL; WASAPI exclusive;
                 loopback verification; spectrum (FFT, worker-side); A/B loop
aurora-metadata  lofty tags; embedded artwork
aurora-storage   SQLite WAL, idempotent rescans, settings, measured BPM
aurora-library   scanner/search/playlists
aurora-kernel    THE MUSIC PLAYER: engine, decks, queue, DSP, VST chain,
                 library, watcher, playlist import/export, events — no UI deps
aurora-providers OAuth token + playlist clients (Spotify, YouTube Music);
                 fixed HTTPS endpoints, allowlisted transports
aurora-api       browser BFF: axum server — OAuth begin/callback/status,
                 disconnect, remote playlists; sessions, CSRF, rate limits
aurora-tauri     thin Tauri shell: command = one kernel call + DTO translation
                 (plus the sandboxed yt-dlp media sidecar commands)
aurora-app       composition root; crash-safe file logging
apps/desktop     Tauri binary (Wry); yt-dlp staged as an external binary
frontend/        modular TS: router + pages (player/queue/library/decks/eq/
                 plugins/settings), central state, styles as tokens
```

Detailed docs: `docs/ARCHITECTURE.md`, `docs/AUDIO_PIPELINE.md`, ADRs in
`docs/ADR/` (output model, bit-perfect claims, library identity, VST3 hosting,
kernel/shell split).

## Requirements

- Rust stable (edition 2024) — `rustup` recommended.
- Node.js LTS + npm.
- Tauri 2 prerequisites: <https://v2.tauri.app/start/prerequisites/>
  (Windows: MSVC Build Tools + WebView2; Linux: webkit2gtk etc.; macOS: Xcode CLT).

## Run (development)

Two terminals — Vite must be listening first:

```bash
cd frontend && npm install && npm run dev     # Vite on 127.0.0.1:1420
```

```bash
cd apps/desktop && TAURI_DEV=true cargo run   # the desktop shell
```

Crash logs survive the process: `%APPDATA%/dev.aurora.player/logs/aurora.log.<date>`
(or the platform equivalent). When a window dies, read the last lines there.

## Build (release bundles)

```bash
cd frontend && npm run tauri:build
```

Installers: `.msi`/NSIS on Windows, `.deb`/`.AppImage` on Linux, `.app`/`.dmg`
on macOS.

## Validation gate (required before commits)

One command runs everything (fmt + clippy + tests + tsc + vite build):

```bash
powershell -ExecutionPolicy Bypass -File check.ps1
```

Or the long form:

```bash
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd frontend && npx tsc --noEmit && npm run build
```

## Deployment (single-origin on Render)

The web deployment is a single service (`render.yaml`): the Docker image
builds the Vite bundle, and `aurora-api` serves it from `/usr/share/aurora/web`
with SPA fallback — so the browser stays on **one origin** and all cookies are
first-party. (A split frontend/API deployment does not work: `*.onrender.com`
subdomains are distinct sites, so the cookies would be third-party and blocked.)

### Cookie model

| Cookie | Lifetime | Role |
|---|---|---|
| `__Host-aurora_session` | 8h | OAuth handshake session; carries the CSRF pairing |
| `__Host-aurora_user` | 180d (HttpOnly, SameSite=Lax) | Long-lived identity key for stored OAuth grants |
| `__Host-aurora_auth` | 30d (HttpOnly, SameSite=Lax) | Login session for email/password accounts |
| `__Host-aurora_csrf` | per-session | Readable echo token; every state-changing call must send the matching `x-csrf-token` header |

All are `Secure; Path=/`. OAuth **grants are keyed by `user_hash`** (the hash
of the 180d user cookie), not by the 8h session — so session expiry or cookie
cleanup never orphans a stored grant: an unknown/expired session cookie is
re-minted on `begin`, the user cookie is untouched.

### Deep links (hash router)

The web shell uses a hash router. Deep links must be `/#/settings`, never
`/settings/accounts`. The canonical post-OAuth redirect target is
`AURORA_API_FRONTEND_URL` (e.g. `https://aurora-api-3.onrender.com/#/settings`).
Legal docs live at `/#/legal?doc=privacy` and `/#/legal?doc=tos` — set the
privacy-policy URL in the Google Cloud Console OAuth consent screen to
`https://<your-host>/#/legal?doc=privacy`.

### Environment variables

| Variable | Required | Purpose |
|---|---|---|
| `AURORA_API_ALLOWED_ORIGINS` | yes | Comma-separated allowed origins; the server refuses to start with open CORS |
| `AURORA_API_FRONTEND_URL` | yes | Post-OAuth redirect target (hash-router deep link) |
| `AURORA_OAUTH_ENCRYPTION_KEY` | yes | base64url of 32 random bytes; encrypts provider tokens at rest (AES-256-GCM) |
| `AURORA_DATABASE_PATH` | no | SQLite path (default `aurora.db`); on Render: `/data/aurora.db` on the attached disk so grants survive restarts |
| `AURORA_REGISTRATION_OPEN` | no | Whether `POST /api/auth/register` accepts new accounts (default `true`) |
| `AURORA_SPOTIFY_ENABLED` / `AURORA_SPOTIFY_CLIENT_ID` / `AURORA_SPOTIFY_CLIENT_SECRET` / `AURORA_SPOTIFY_REDIRECT_URI` | per provider | Spotify OAuth app credentials |
| `AURORA_YOUTUBE_MUSIC_ENABLED` / `AURORA_YOUTUBE_MUSIC_CLIENT_ID` / `AURORA_YOUTUBE_MUSIC_CLIENT_SECRET` / `AURORA_YOUTUBE_MUSIC_REDIRECT_URI` | per provider | YouTube Music OAuth app credentials |

Generate the encryption key with
`openssl rand -base64 32 | tr '+/' '-_' | tr -d '='`.
Redirect URIs: `https://<your-host>/api/oauth/spotify/callback` and
`https://<your-host>/api/oauth/youtube-music/callback`.

### Accounts and grant claiming

Accounts are **email + Argon2id** (passwords stored as PHC strings; login
performs the same Argon2 work whether or not the user exists, so timing
cannot reveal registered emails). Logging in (or registering) from a browser
that carries the anonymous `__Host-aurora_user` cookie **claims** that
cookie's OAuth grants, moving them to the account's stable hash
(`hash("user:{id}")`). After that the account's grants survive across
browsers and logins.

## Project status & known gaps

`plan.md` (git-ignored, local) tracks every weak or unfinished area of the
codebase with a severity and a next step. The validation gate below is what
"done" means for a commit; the plan is what is honestly *not* done yet.

## Audio integrity policy

Aurora never claims bit-perfect output from the source format or the selected
device alone. The complete path — decoder representation, sample conversion,
DSP state, VST chain, backend mode, OS mixer — is reported per stream. Any
active transformation drops integrity to `transformed`; unproven paths are
`unverified`, never claimed. Exclusive mode + loopback measurement is the only
route to `verified`.

The real-time callback performs no allocation, no locks, no I/O, no logging.
All decode/DSP/VST work happens on the worker thread; plugins run in separate
processes.
