# Aurora — project guidance for Claude

## Current state (2026-11-10)
- **Ф7 (HTTPS deploy) — ✅ CLOSED, live on Render**: `aurora-api-3.onrender.com`
- **Ф5 (playlist import) — ✅ CLOSED 2026-11-09, verified live end-to-end**.
- **User authentication — ✅ CLOSED 2026-11-10** (`61247e9` + `3e71d9d`): email+password (argon2id), `__Host-aurora_auth` 30d cookie, login/register page `/#/auth?mode=login`, Аккаунт card in Settings, anonymous grants claimed on login, `AURORA_REGISTRATION_OPEN` env gate, reset deferred.
- **Security hardening — ✅ mostly CLOSED** (`7e1b7c2`+`e723d37`): live Google secret scrubbed from git (ROTATION STILL REQUIRED — old secret in history), security headers (CSP/HSTS/X-Frame-Options/Referrer-Policy/Permissions-Policy). Verifier erasure + log-scrub test remain open.
- **Lyrics+translation — ✅ CLOSED** (`6f5d670`): embedded tags (USLT/syncText→LRC), LRC sidecars, LibreTranslate per-line translation (off by default, endpoint configurable), synced two-line player display.
- **Legal docs — ✅ CLOSED** (`c311dd4`): privacy policy + ToS (RU) at `/#/legal?doc=privacy|tos`. Google Cloud Console privacy-policy URL is a MANUAL step.
- **DSP audiophile block — ✅ quick wins + reverb CLOSED** (`78fa497`+`2573020`): bs2b crossfeed, surgical EQ (31 bands, Q≤12), partitioned time-domain convolution reverb with 3 synthetic IR presets + user IR upload (10s/5MB cap). Bit-perfect path provably untouched. Deferred: ReplayGain2/LUFS scan, room correction.
- **CI fast lane — ✅ CLOSED** (`5beb49a`): fast lane on push/PR; heavy matrix/MSRV/bundles gated by tag v*/dispatch/label `bundle`.
- **UX 401-cycle — ✅ CLOSED** (`e723d37`): single-flight session bootstrap + one retry in requestJson.
- 222 Rust + 109 frontend tests green, clippy -D warnings clean.

## Roadmap (next, in order)
1. REVOKE/rotate Google client secret (manual, blocking security).
2. Set privacy-policy URL in Google Cloud Console consent screen (manual).
3. Live smoke: register account → grant claims → login from second browser.
4. Verifier erasure after callback exchange + log-scrub regression test.
5. ReplayGain 2.0/LUFS loudness scan; room correction (deferred DSP).
6. Matching-quality metric for imports (offline harness).

## Architecture reminder
- Single-origin deployment: API serves web shell from `/` (SPA fallback), so cookies are first-party. `render.yaml` blueprint.
- Long-lived `__Host-aurora_user` cookie (180d, HttpOnly, SameSite=Lax) = token key. Session cookie (8h) = CSRF carrier only.
- Hash router: deep links must be `/#/settings` not `/settings/accounts`. `AURORA_API_FRONTEND_URL` = `https://aurora-api-3.onrender.com/#/settings`.

## Security constraints (non-negotiable)
- NO spotDL, Spotify2YouTubeMusic extraction, unofficial YouTube Music WEB_REMIX, yt-dlp for commercial music/DRM, LyricsGenius/lyrics-site scraping.
- yt-dlp ONLY for metadata + authorized HTTPS sources (archive.org, youtube.com, youtu.be).
- NO client secrets, auth codes, PKCE verifiers, access/refresh tokens in frontend/browser storage/Tauri IPC.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.

## Environment
- `AURORA_OAUTH_ENCRYPTION_KEY` — base64url of 32 random bytes (local backup in `.aurora_encryption_key_local.txt`).
- Render env vars: `AURORA_API_ALLOWED_ORIGINS`, `AURORA_API_FRONTEND_URL`, `AURORA_YOUTUBE_MUSIC_CLIENT_ID/SECRET/REDIRECT_URI`, `AURORA_DATABASE_PATH=/data/aurora.db`, `AURORA_YOUTUBE_MUSIC_ENABLED=true`.

## Next session
1. User reconnects YouTube Music at `https://aurora-api-3.onrender.com/#/settings` (Ctrl+Shift+R).
2. Import playlist → verify matching quality.
3. If Spotify needed — configure credentials.