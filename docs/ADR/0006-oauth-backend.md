# ADR 0006: OAuth backend (BFF) and yt-dlp media surface

Status: Accepted (Phase — browser shell OAuth runtime v0.2.0)

## Context

The browser shell (ADR 0005's second shell) needs Spotify and YouTube
Music sign-in and playlist access. OAuth client secrets, authorization
codes, PKCE verifiers, access and refresh tokens must never reach the
browser: anything in the browser is readable by anything running in it.
The browser also cannot spawn processes, so the desktop yt-dlp sidecar
commands are unreachable from it — yet the same operations (media
metadata, downloads) are part of the product.

A separately hosted HTTPS backend already exists as a deployment premise
(docs/PROVIDERS.md, "Web OAuth deployment"): the browser must not be
trusted with provider credentials, and the desktop loopback adapter
cannot serve a browser origin.

## Decision

### OAuth: a server-side backend-for-frontend

`aurora-api` is an axum server that owns the entire OAuth flow:

- **Authorization code + PKCE (S256)** exchange. `POST
  /api/oauth/{provider}/begin` mints the verifier and state, stores the
  transaction, and returns only the authorization URL. The provider
  callback (`GET /api/oauth/{provider}/callback`) lands on the server;
  the browser is redirected back to the frontend with no tokens in the
  URL.
- **Server sessions.** An opaque token in an HttpOnly, Secure,
  SameSite=Strict `__Host-aurora_session` cookie. Sessions expire after
  8 hours; expiry is swept opportunistically (piggybacked on `begin`
  traffic, at most every 300 s).
- **CSRF.** A paired readable `__Host-aurora_csrf` cookie echoed in the
  `X-CSRF-Token` header. The authoritative check is the *stored*
  SHA-256 hash of the token bound to the session's hash (constant-time
  compare in storage); the cookie/header echo is a well-formedness gate
  in front of it. Every state-changing endpoint (`begin` with a live
  session, `disconnect`, media) requires the pair.
- **Exact-Origin CORS.** Only origins on a configured allowlist, with
  credentials enabled. A missing or foreign Origin is rejected before
  any handler logic.
- **Rate limiting.** Fixed 60-second windows, in-process: 30 per client
  IP and 120 per provider per window, enforced independently, stricter
  one wins. Over-budget requests get 429 with `Retry-After` rounded up
  to the window close. In-process is deliberate: the process is the
  unit of scale in the single-instance deployment model.
- **Tokens at rest.** AES-256-GCM, key from
  `AURORA_OAUTH_ENCRYPTION_KEY` (32 bytes, base64url, required at
  startup). Only ciphertext reaches SQLite (`oauth_accounts.
  token_ciphertext`).
- **One-time `state`.** The callback claims the transaction with a
  guarded `UPDATE ... WHERE consumed_at IS NULL` — a replayed or
  concurrent callback gets nothing. Session ids, CSRF tokens and state
  values are stored as SHA-256 hashes, never plaintext.
- **Refresh classification.** A definitive `invalid_grant` marks the
  account `reauthorization_required` (401, user must re-connect);
  transient refresh failures return 502, leave the stored grant
  untouched, and never disconnect the account. Rotated tokens are
  persisted as soon as the refresh happened, independent of whether the
  follow-up playlist request succeeded.
- **Input bounds.** Every caller-supplied value (session, CSRF, state,
  code, playlist id, cursor, URL, body size, provider response size) is
  length- and charset-validated before it is hashed, bound to SQL, or
  put into an outbound request (`aurora-api/src/limits.rs`).

Provider configuration absent or a non-HTTPS/provider-mismatched
redirect URI configured: the endpoints fail closed with 503.

### yt-dlp media: the same operations, two exposure paths

Metadata and download are exposed twice with one policy:

- **Desktop (Tauri):** `yt_dlp_metadata` / `yt_dlp_download` commands
  invoke the staged sidecar binary directly; the download lands on the
  user's chosen absolute directory.
- **Browser:** `POST /api/media/metadata` (session + CSRF, charged to
  the `media` rate-limit bucket, not any provider's). The URL must pass
  the same allowlist; the response is a sanitized camelCase DTO
  (title/uploader/durationSeconds/webpageUrl/thumbnailUrl) — no raw
  yt-dlp JSON passthrough, every field bounded and control-character
  checked server-side. `GET /api/media/ping` (unauthenticated,
  existence check only) tells the UI whether this deployment has a
  yt-dlp binary at all.

Downloads are desktop-only by design: a downloaded file belongs on the
user's disk next to their library, and a server is not that disk. (A
future streaming download is a deployment decision, tracked in
plan.md.)

Shared policy across both paths: HTTPS-only host allowlist
(`archive.org`, `www.archive.org`, `youtube.com`, `www.youtube.com`,
`youtu.be`) defined once in `aurora-api/src/limits.rs` and mirrored by
the Tauri commands and the frontend validator; no userinfo in URLs;
bounded URL length; `--no-playlist`; combined output capped at 2 MiB;
45-second hard timeout. No DRM or protected-stream extraction is
attempted — the tool is invoked for metadata on public pages only, and
widening the allowlist is a product decision, not a workaround.

## Consequences

- In-process rate limiting is per-instance: a second process holds its
  own counters. Acceptable while the deployment model is one instance;
  it is a blast-radius limit, not a distributed quota.
- The `__Host-` prefix demands HTTPS and a single host per cookie:
  the API must be served from one origin, no path-split deployments.
- Widening the media allowlist is a product decision about what Aurora
  will fetch; it must not be widened to work around a failure.
- Browser metadata depends on the server finding a yt-dlp binary; a
  deployment without one serves 503 on metadata and the UI is expected
  to hide the feature via `/api/media/ping` rather than show a
  featureless error.
- Session-bound state means the OAuth tables hold no reusable
  plaintext: a database leak yields hashed sessions/CSRF/state and
  AES-256-GCM ciphertext, not grants. There is no key-rotation
  procedure yet (tracked in plan.md).
- The desktop flow is unchanged: native loopback + OS credential
  storage (docs/PROVIDERS.md, "Aurora 0.2.0") remains the desktop path.
