---
title: Aurora API
emoji: 🎵
colorFrom: indigo
colorTo: purple
sdk: docker
app_port: 8080
pinned: false
---

# Aurora API (OAuth BFF)

OAuth backend-for-frontend for the Aurora music player. Docker Space;
the image builds the `aurora-api` binary from the repo root (same
`Dockerfile` used for Render/other PaaS) and listens on
`0.0.0.0:$PORT` (default 8080).

Required Space secrets (Settings → Variables and secrets):

- `AURORA_API_ALLOWED_ORIGINS` — https://aurora-web-vsqh.onrender.com
- `AURORA_API_FRONTEND_URL` — https://aurora-web-vsqh.onrender.com/settings/accounts
- `AURORA_OAUTH_ENCRYPTION_KEY` — base64url of 32 random bytes
- `AURORA_DATABASE_PATH` — optional, defaults to a writable path in the container
- `AURORA_SPOTIFY_*` / `AURORA_YOUTUBE_MUSIC_*` — provider credentials (optional)

Health: `GET /health`
