-- Phase 6: initial library schema.
--
-- Tracks are keyed by content-derived TrackId (uuid string), so rescans
-- update in place instead of duplicating (idempotency exit criterion).
CREATE TABLE tracks (
    id                   TEXT PRIMARY KEY,
    path                 TEXT NOT NULL,
    title                TEXT,
    artist               TEXT,
    album                TEXT,
    genre                TEXT,
    year                 INTEGER,
    track_number         INTEGER,
    format               TEXT NOT NULL DEFAULT 'unknown',
    duration_ms          INTEGER,
    replaygain_track_db  REAL,
    replaygain_album_db  REAL
);

CREATE INDEX idx_tracks_artist_album ON tracks (artist, album, track_number);
CREATE INDEX idx_tracks_path ON tracks (path);

CREATE TABLE playlists (
    id   TEXT PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE playlist_entries (
    playlist_id TEXT NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
    track_id    TEXT NOT NULL REFERENCES tracks (id)    ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, track_id)
);

CREATE INDEX idx_playlist_entries_order ON playlist_entries (playlist_id, position);
