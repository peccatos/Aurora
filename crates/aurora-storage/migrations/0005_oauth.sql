CREATE TABLE oauth_sessions (
    id_hash TEXT PRIMARY KEY,
    csrf_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE oauth_transactions (
    state_hash TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    session_id_hash TEXT NOT NULL,
    verifier TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    consumed_at TEXT
);
CREATE INDEX oauth_transactions_session_provider ON oauth_transactions(session_id_hash, provider);
CREATE INDEX oauth_transactions_expires ON oauth_transactions(expires_at);
CREATE TABLE oauth_accounts (
    session_id_hash TEXT NOT NULL,
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
    PRIMARY KEY (session_id_hash, provider)
);
