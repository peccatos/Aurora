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
