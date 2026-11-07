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

## Future web deployment

Web OAuth is intentionally not enabled by this desktop-only stage. A future web deployment needs a separately hosted HTTPS callback, registered per environment, with server-side code exchange and secret storage. Do not reuse the desktop loopback URI or put a client secret in a Vite bundle.

## Configuration

Copy `.env.example` for local shape only. Empty IDs and disabled providers are expected: the UI must show `Not configured` and fail closed. Never commit real IDs, secrets, tokens, or provider approval data. Endpoints and scopes are allow-listed by `aurora-providers`; arbitrary URLs and scopes are rejected.

Canonical provider IDs are `spotify` and `youtube-music`. `google-youtube` is not a public account ID.
