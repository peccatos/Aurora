-- Email/password accounts and their login sessions.
--
-- `login_sessions` mirrors the shape of `oauth_sessions` but lives much longer
-- (30 days): the id is the cookie value (hashed at rest, like every other
-- token here), the csrf hash is the readable-cookie CSRF binding used by the
-- logout endpoint. Rows are keyed by the hash of the cookie value, never the
-- cookie itself, so a database leak exposes no usable credentials.
CREATE TABLE users (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT UNIQUE NOT NULL,
    password_hash TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE TABLE login_sessions (
    id_hash TEXT PRIMARY KEY,
    user_id INTEGER NOT NULL,
    csrf_hash TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX login_sessions_user ON login_sessions(user_id);
CREATE INDEX login_sessions_expires ON login_sessions(expires_at);
