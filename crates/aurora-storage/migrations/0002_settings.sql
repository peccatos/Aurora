-- Phase 7: key-value settings (device selection, UI prefs).
-- Stored in the library database: it is already the user-scoped, always
-- open store; a separate config file would race the same lifecycle.
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
