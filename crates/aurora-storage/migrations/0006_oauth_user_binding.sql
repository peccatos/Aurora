-- OAuth accounts are keyed by the user cookie hash instead of the short-lived
-- session hash. The session (8h) remains the CSRF carrier only; tokens survive
-- session expiry, rotation by a second tab, and cookie cleanup — the user just
-- re-authenticates the session, the stored grant is still usable.
--
-- Existing rows are session-bound and therefore unreachable after this change
-- (their sessions cannot be re-minted to the same id); they are dropped rather
-- than migrated with a fabricated user id. Users reconnect once.
DROP TABLE IF EXISTS oauth_accounts;
CREATE TABLE oauth_accounts (
    user_hash TEXT NOT NULL,
    provider TEXT NOT NULL,
    subject TEXT NOT NULL,
    display_name TEXT NOT NULL,
    avatar_url TEXT,
    capabilities_json TEXT NOT NULL,
    connected_at TEXT NOT NULL,
    keyring_account_id TEXT NOT NULL,
    token_ciphertext BLOB NOT NULL,
    auth_state TEXT NOT NULL DEFAULT 'connected',
    last_auth_error TEXT,
    PRIMARY KEY (user_hash, provider)
);
