# Provider infrastructure

## Aurora 0.2.0

Adds native desktop OAuth for Spotify and YouTube Music using PKCE, an ephemeral loopback callback, and OS credential storage. Provider client configuration is loaded by the desktop process; live sign-in requires configured provider credentials and registered redirect URIs. Account metadata is persisted locally, while token secrets remain outside the frontend and SQLite.

Provider configuration is loaded by the native desktop process. The frontend never receives client secrets, authorization codes, PKCE verifiers, access tokens, or refresh tokens. SQLite stores account metadata only; credentials belong in the operating-system credential store.

## Desktop redirect registration

Register the exact redirect URI used by the running desktop coordinator. The URI is loopback-only and includes the ephemeral port and path:

```text
http://127.0.0.1:<ephemeral-port>/oauth/callback/spotify
http://127.0.0.1:<ephemeral-port>/oauth/callback/youtube-music
```

The provider dashboard must allow the exact URI, including path and trailing slash semantics. The coordinator must reject non-loopback hosts, `0.0.0.0`, arbitrary paths, and HTTP redirects outside development. The callback listener is not the frontend development server.

## Web OAuth deployment

Web OAuth is supported only through a separately hosted HTTPS backend. The backend owns the provider callback, state and PKCE verifier, code exchange, token refresh, and credential storage. The browser receives only an authorization URL and sanitized account metadata through a secure session-cookie API; it never receives client secrets, authorization codes, PKCE verifiers, access tokens, or refresh tokens.

Set `VITE_OAUTH_API_BASE_URL` to the backend’s HTTPS origin (or HTTPS path prefix) at build time; leaving it empty disables the browser adapters and keeps sign-in unavailable. Register an exact HTTPS callback independently for each web environment. Do not reuse the desktop loopback URI or put a client secret in a Vite bundle. The backend must bind authorization attempts to the user session, protect begin/revoke endpoints against CSRF, consume callbacks once, and fail closed when provider configuration is absent. Desktop continues using the native ephemeral loopback/Tauri adapter.

## Configuration

Copy `.env.example` for local shape only. Empty IDs and disabled providers are expected: the UI must show `Not configured` and fail closed. Never commit real IDs, secrets, tokens, or provider approval data. Endpoints and scopes are allow-listed by `aurora-providers`; arbitrary URLs and scopes are rejected.

Canonical provider IDs are `spotify` and `youtube-music`. `google-youtube` is not a public account ID.

## Browser media metadata (yt-dlp)

The browser cannot spawn processes, so the same yt-dlp operations the Tauri shell exposes as sidecar commands are also served by the OAuth backend. Metadata only: downloads stay desktop-only, because a downloaded file belongs on the user's disk and a server is not that disk.

`POST /api/media/metadata` — request body `{"url": "..."}`. The URL must be HTTPS, host on the shared allowlist (`archive.org`, `www.archive.org`, `youtube.com`, `www.youtube.com`, `youtu.be`), no userinfo, length-bounded; anything else is rejected with 403. The request requires the browser session (no OAuth account needed — media lookup is not a provider grant operation) plus the CSRF pair: session cookie, and the readable `__Host-aurora_csrf` cookie value echoed in the `X-CSRF-Token` header, validated against the stored hash bound to the session. It is charged to the `media` rate-limit bucket.

The response is a sanitized camelCase DTO — `title`, `uploader`, `durationSeconds`, `webpageUrl`, `thumbnailUrl` — not yt-dlp's raw JSON. Fields are bounded and control-character-checked server-side; an off-allowlist `webpageUrl` or `thumbnailUrl` is rejected/dropped. Invocation uses `--no-playlist`, a 45-second timeout, and a 2 MiB output cap.

`GET /api/media/ping` is unauthenticated and reports whether the deployment has a yt-dlp binary at all (`200 {"available":true}` / `503 {"available":false}`), so the UI can hide the feature instead of failing on first use. No DRM or protected-stream extraction is attempted; widening the allowlist is a product decision. Design details in docs/ADR/0006-oauth-backend.md.

## Rate limiting

The OAuth backend rate-limits every endpoint with fixed 60-second windows, enforced in-process: 30 requests per client IP and 120 per provider per window (media requests use the `media` bucket, not a provider's). Both counters are charged and the stricter one wins. Over-budget requests get `429` with a body of `{"error":"rate_limited","retryAfterSeconds":N}` and a `Retry-After` header rounded up so the retry lands in the next window.

The client IP is the TCP peer address; `X-Forwarded-For` is not consulted. The counters are in-process state — per-instance. A deployment running more than one backend process gets independent limits, not a shared quota. Requests are also body-capped (8 KiB) before any handler runs.

## Encrypted token storage

Web OAuth tokens are stored encrypted, never plaintext: `AURORA_OAUTH_ENCRYPTION_KEY` must be set for the backend to start, must be base64url, and must decode to exactly 32 bytes. Token sets are sealed with AES-256-GCM (random 96-bit nonce prepended to the ciphertext) and only the ciphertext reaches SQLite.

Sessions, CSRF tokens, and one-time `state` values are stored as SHA-256 hashes, not plaintext; the `state` transaction is claimed with a guarded `UPDATE` so a callback or replay can only succeed once. There is no key-rotation procedure yet — losing the key loses the stored grants (users re-connect); tracked in plan.md.
