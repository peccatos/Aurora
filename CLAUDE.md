# Aurora — project guidance for Claude

## Current state (2026-11-09)
- **Ф7 (HTTPS deploy) — ✅ CLOSED, live on Render**: `aurora-api-3.onrender.com`
- **Ф5 (playlist import) — ✅ CLOSED 2026-11-09, verified live end-to-end**: begin → Google → callback → grant under user_hash → Connected in UI → playlist import succeeded (7-track playlist). Three prod bugs fixed on the way: hash-router frontend URL (`5b8b3b7`), same-origin 403 via Referer fallback in `origin_ok` (`b93b154`), YouTube playlistItems parse — `deny_unknown_fields` rejected the real wire envelope (kind/etag/pageInfo) → tolerant parse (`06d2156`).
- **Ф1–Ф4, Ф6, Ф8** — code + tests closed.
- 88 tests green, clippy clean.

## Roadmap (next, in order)
1. **User authentication** in the service (email+password; replace anonymous 180d cookie as the grant key).
2. **UX fixes**: 401-cycle (import/флоу сами перевыпускают сессию), Settings page error/retry states.
3. **UI pass** over the web shell.
4. Lyrics (legal sources only: user files/LRC, no site scraping) + translation via LibreTranslate/Argos.
5. Audio effects for audiophiles (custom reverbs, DSP).
6. Legal: privacy policy, ToS.
7. Security hardening: headers, log scrubbing of state tokens.

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