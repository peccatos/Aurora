//! SQLite persistence for the library: migrations, track rows, playlists.
//!
//! This crate is the only place SQL runs (ARCHITECTURE.md invariant 6).
//! Domain types never leak SQLite types; the repositories map rows to
//! `aurora_core::{Track, TrackId, Playlist}`.
//!
//! Idempotency contract (Phase 6 exit criterion): the tracks table is
//! keyed by `TrackId` (content hash); inserts use `ON CONFLICT ... DO
//! UPDATE` so a rescan updates paths/metadata in place and never
//! duplicates a track.

use std::path::Path;

use aurora_core::{Album, Artist, AudioFormat, Playlist, Track, TrackId};
use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub use sqlx::SqlitePool;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("database error: {0}")]
    Sql(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

/// Content hash of a file (blake3, 32-byte digest).
///
/// Lives in storage (not the scanner) because the identity contract is a
/// persistence concern: the digest is what makes rescan idempotent.
pub fn content_digest(path: &Path) -> std::io::Result<[u8; 32]> {
    let mut hasher = blake3::Hasher::new();
    std::io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
    Ok(*hasher.finalize().as_bytes())
}

/// Open (and migrate) the library database at `path`.
///
/// WAL + NORMAL sync: the library tolerates losing the last transaction
/// on a power cut (a rescan fixes that) in exchange for much faster
/// writes during scanning.
pub async fn open(path: &Path) -> Result<SqlitePool, StorageError> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await?;

    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

/// In-memory database (tests, tooling).
pub async fn open_in_memory() -> Result<SqlitePool, StorageError> {
    let options = SqliteConnectOptions::new()
        .filename(":memory:")
        .journal_mode(SqliteJournalMode::Memory)
        .synchronous(SqliteSynchronous::Off)
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

// ---------------------------------------------------------------------------
// Tracks
// ---------------------------------------------------------------------------

fn format_to_db(format: AudioFormat) -> &'static str {
    match format {
        AudioFormat::Flac => "flac",
        AudioFormat::Wav => "wav",
        AudioFormat::Alac => "alac",
        AudioFormat::Aiff => "aiff",
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Aac => "aac",
        AudioFormat::OggVorbis => "ogg_vorbis",
        AudioFormat::Opus => "opus",
        AudioFormat::WavPack => "wavpack",
        AudioFormat::Unknown => "unknown",
    }
}

fn format_from_db(raw: &str) -> AudioFormat {
    match raw {
        "flac" => AudioFormat::Flac,
        "wav" => AudioFormat::Wav,
        "alac" => AudioFormat::Alac,
        "aiff" => AudioFormat::Aiff,
        "mp3" => AudioFormat::Mp3,
        "aac" => AudioFormat::Aac,
        "ogg_vorbis" => AudioFormat::OggVorbis,
        "opus" => AudioFormat::Opus,
        "wavpack" => AudioFormat::WavPack,
        _ => AudioFormat::Unknown,
    }
}

fn parse_uuid(raw: &str) -> Option<uuid::Uuid> {
    uuid::Uuid::parse_str(raw).ok()
}

fn row_to_track(row: &sqlx::sqlite::SqliteRow) -> Track {
    let id = parse_uuid(row.try_get::<String, _>("id").as_deref().unwrap_or(""))
        .unwrap_or_else(uuid::Uuid::nil);
    let mut track = Track::new(
        TrackId(id),
        row.try_get::<String, _>("path").unwrap_or_default(),
    );
    track.title = row
        .try_get::<Option<String>, _>("title")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    track.artist = row
        .try_get::<Option<String>, _>("artist")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(Artist::new);
    track.album = row
        .try_get::<Option<String>, _>("album")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(|title| Album {
            title,
            artist: None,
        });
    track.genre = row
        .try_get::<Option<String>, _>("genre")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty());
    track.year = row
        .try_get::<Option<i64>, _>("year")
        .ok()
        .flatten()
        .and_then(|y| i32::try_from(y).ok());
    track.track_number = row
        .try_get::<Option<i64>, _>("track_number")
        .ok()
        .flatten()
        .and_then(|t| u32::try_from(t).ok());
    track.format = format_from_db(
        row.try_get::<String, _>("format")
            .as_deref()
            .unwrap_or("unknown"),
    );
    track.duration_ms = row
        .try_get::<Option<i64>, _>("duration_ms")
        .ok()
        .flatten()
        .and_then(|d| u64::try_from(d).ok());
    track.replaygain_track_db = row
        .try_get::<Option<f64>, _>("replaygain_track_db")
        .ok()
        .flatten();
    track.replaygain_album_db = row
        .try_get::<Option<f64>, _>("replaygain_album_db")
        .ok()
        .flatten();
    track.bpm = row.try_get::<Option<f64>, _>("bpm").ok().flatten();
    track
}

/// Upsert a track keyed by content identity.
///
/// `ON CONFLICT(id) DO UPDATE`: a rescan refreshes path/metadata in place.
/// Returns `true` when the track was newly inserted, `false` when an
/// existing row was updated (lets the scanner report added vs refreshed).
pub async fn upsert_track(pool: &SqlitePool, track: &Track) -> Result<bool, StorageError> {
    let existing = sqlx::query("SELECT 1 FROM tracks WHERE id = ?")
        .bind(track.id.0.to_string())
        .fetch_optional(pool)
        .await?;
    let was_new = existing.is_none();
    sqlx::query(
        r#"
        INSERT INTO tracks (
            id, path, title, artist, album, genre, year, track_number,
            format, duration_ms, replaygain_track_db, replaygain_album_db, bpm
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ON CONFLICT(id) DO UPDATE SET
            path = excluded.path,
            title = excluded.title,
            artist = excluded.artist,
            album = excluded.album,
            genre = excluded.genre,
            year = excluded.year,
            track_number = excluded.track_number,
            format = excluded.format,
            duration_ms = excluded.duration_ms,
            replaygain_track_db = excluded.replaygain_track_db,
            replaygain_album_db = excluded.replaygain_album_db,
            bpm = COALESCE(excluded.bpm, tracks.bpm)
        "#,
    )
    .bind(track.id.0.to_string())
    .bind(&track.path)
    .bind(&track.title)
    .bind(track.artist.as_ref().map(|a| a.name.as_str()))
    .bind(track.album.as_ref().map(|a| a.title.as_str()))
    .bind(&track.genre)
    .bind(track.year.map(i64::from))
    .bind(track.track_number.map(i64::from))
    .bind(format_to_db(track.format))
    .bind(track.duration_ms.map(|d| d as i64))
    .bind(track.replaygain_track_db)
    .bind(track.replaygain_album_db)
    .bind(track.bpm)
    .execute(pool)
    .await?;
    Ok(was_new)
}

/// Remove tracks whose paths are absent from `keep` (files deleted or
/// moved out of the scanned roots).
pub async fn prune_missing(pool: &SqlitePool, keep: &[String]) -> Result<u64, StorageError> {
    let result =
        sqlx::query("DELETE FROM tracks WHERE path NOT IN (SELECT value FROM json_each(?))")
            .bind(serde_json::json!(keep).to_string())
            .execute(pool)
            .await?;
    Ok(result.rows_affected())
}

/// Total tracks in the library.
pub async fn track_count(pool: &SqlitePool) -> Result<u64, StorageError> {
    let row = sqlx::query("SELECT COUNT(*) AS n FROM tracks")
        .fetch_one(pool)
        .await?;
    Ok(row.try_get::<i64, _>("n").map(|n| n as u64).unwrap_or(0))
}

/// All tracks, ordered artist/album/track for stable listing.
pub async fn all_tracks(pool: &SqlitePool) -> Result<Vec<Track>, StorageError> {
    let rows = sqlx::query(
        r#"
        SELECT * FROM tracks
        ORDER BY artist IS NULL, artist, album IS NULL, album, track_number, title, path
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_track).collect())
}

/// Full-library search: case-insensitive substring across title, artist,
/// album and path (the last catches untagged files by file name).
pub async fn search_tracks(pool: &SqlitePool, query: &str) -> Result<Vec<Track>, StorageError> {
    let like = format!("%{}%", query.replace('%', ""));
    let rows = sqlx::query(
        r#"
        SELECT * FROM tracks
        WHERE title LIKE ? ESCAPE '\'
           OR artist LIKE ? ESCAPE '\'
           OR album LIKE ? ESCAPE '\'
           OR path LIKE ? ESCAPE '\'
        ORDER BY artist IS NULL, artist, album IS NULL, album, track_number, title, path
        "#,
    )
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .bind(&like)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_track).collect())
}

// ---------------------------------------------------------------------------
// Measured tempo
// ---------------------------------------------------------------------------

/// Find a track by its library path (the deck flow knows the path).
///
/// Returns the domain Track (with id and current bpm) when present.
pub async fn track_by_path(pool: &SqlitePool, path: &str) -> Result<Option<Track>, StorageError> {
    let row = sqlx::query("SELECT * FROM tracks WHERE path = ?")
        .bind(path)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_track))
}

/// Persist a measured BPM for one track.
///
/// Write-only from the analyzer side: this records a measurement that was
/// actually performed on the content. A NULL `bpm` is a no-op (the
/// measurement failed; we do not store guesses).
pub async fn set_track_bpm(
    pool: &SqlitePool,
    path: &str,
    bpm: Option<f64>,
) -> Result<(), StorageError> {
    if let Some(bpm) = bpm {
        sqlx::query("UPDATE tracks SET bpm = ? WHERE path = ?")
            .bind(bpm)
            .bind(path)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Playlists
// ---------------------------------------------------------------------------
/// Create a playlist; returns its id.
pub async fn create_playlist(pool: &SqlitePool, name: &str) -> Result<String, StorageError> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO playlists (id, name) VALUES (?, ?)")
        .bind(&id)
        .bind(name)
        .execute(pool)
        .await?;
    Ok(id)
}

/// Rename a playlist.
pub async fn rename_playlist(
    pool: &SqlitePool,
    id: &str,
    new_name: &str,
) -> Result<(), StorageError> {
    sqlx::query("UPDATE playlists SET name = ? WHERE id = ?")
        .bind(new_name)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete a playlist (cascade removes its entries).
pub async fn delete_playlist(pool: &SqlitePool, id: &str) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM playlists WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// All playlists with denormalized track counts.
pub async fn all_playlists(pool: &SqlitePool) -> Result<Vec<Playlist>, StorageError> {
    let rows = sqlx::query(
        r#"
        SELECT p.id, p.name, COUNT(pe.track_id) AS track_count
        FROM playlists p
        LEFT JOIN playlist_entries pe ON pe.playlist_id = p.id
        GROUP BY p.id
        ORDER BY p.name
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .iter()
        .map(|row| Playlist {
            id: row.try_get::<String, _>("id").unwrap_or_default(),
            name: row.try_get::<String, _>("name").unwrap_or_default(),
            track_count: row.try_get::<i64, _>("track_count").unwrap_or(0) as u64,
        })
        .collect())
}

/// Set the playlist contents, preserving order.
pub async fn set_playlist_tracks(
    pool: &SqlitePool,
    playlist_id: &str,
    track_ids: &[TrackId],
) -> Result<(), StorageError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM playlist_entries WHERE playlist_id = ?")
        .bind(playlist_id)
        .execute(&mut *tx)
        .await?;
    for (position, track_id) in track_ids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO playlist_entries (playlist_id, track_id, position) VALUES (?, ?, ?)",
        )
        .bind(playlist_id)
        .bind(track_id.0.to_string())
        .bind(i64::try_from(position).unwrap_or(0))
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Playlist tracks, in saved order.
pub async fn playlist_tracks(
    pool: &SqlitePool,
    playlist_id: &str,
) -> Result<Vec<Track>, StorageError> {
    let rows = sqlx::query(
        r#"
        SELECT t.* FROM playlist_entries pe
        JOIN tracks t ON t.id = pe.track_id
        WHERE pe.playlist_id = ?
        ORDER BY pe.position
        "#,
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_track).collect())
}

// ---------------------------------------------------------------------------
// Settings (key-value, Phase 7)
// ---------------------------------------------------------------------------

/// Read a setting value by key.
pub async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, StorageError> {
    let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.try_get::<String, _>("value").ok()))
}

/// Write a setting value by key (upsert).
pub async fn set_setting(pool: &SqlitePool, key: &str, value: &str) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(key)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a setting by key.
pub async fn delete_setting(pool: &SqlitePool, key: &str) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM settings WHERE key = ?")
        .bind(key)
        .execute(pool)
        .await?;
    Ok(())
}

/// Folders the user has scanned (library roots), oldest first.
pub async fn library_roots(pool: &SqlitePool) -> Result<Vec<String>, StorageError> {
    Ok(get_setting(pool, "library_roots")
        .await?
        .map(|raw| serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default())
        .unwrap_or_default())
}

/// Append a scan root (idempotent: no duplicate entries).
pub async fn add_library_root(pool: &SqlitePool, root: &str) -> Result<(), StorageError> {
    let mut roots = library_roots(pool).await?;
    if !roots.iter().any(|r| r == root) {
        roots.push(root.to_string());
        set_setting(pool, "library_roots", &serde_json::json!(roots).to_string()).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    async fn pool() -> SqlitePool {
        open_in_memory().await.expect("open memory db")
    }

    fn track(path: &str, title: Option<&str>, artist: Option<&str>) -> Track {
        let mut t = Track::new(TrackId(Uuid::new_v4()), path);
        t.title = title.map(Into::into);
        t.artist = artist.map(|a| Artist { name: a.into() });
        t.format = AudioFormat::Flac;
        t
    }

    #[tokio::test]
    async fn rescan_upsert_is_idempotent() {
        let pool = pool().await;
        let mut t = track("m/a.flac", Some("First"), Some("Beth"));
        upsert_track(&pool, &t).await.unwrap();
        // Same content id, new path + metadata (rescan after move/re-tag).
        t.path = "moved/a.flac".into();
        t.title = Some("Renamed".into());
        upsert_track(&pool, &t).await.unwrap();
        assert_eq!(track_count(&pool).await.unwrap(), 1);
        let all = all_tracks(&pool).await.unwrap();
        assert_eq!(all[0].path, "moved/a.flac");
        assert_eq!(all[0].title.as_deref(), Some("Renamed"));
    }

    #[tokio::test]
    async fn search_matches_title_artist_and_filename() {
        let pool = pool().await;
        upsert_track(
            &pool,
            &track("m/nightcall.flac", Some("Nightcall"), Some("Kavinsky")),
        )
        .await
        .unwrap();
        upsert_track(&pool, &track("m/untagged_file.flac", None, None))
            .await
            .unwrap();
        assert_eq!(search_tracks(&pool, "night").await.unwrap().len(), 1);
        assert_eq!(search_tracks(&pool, "kavinsky").await.unwrap().len(), 1);
        assert_eq!(search_tracks(&pool, "untagged").await.unwrap().len(), 1);
        assert_eq!(search_tracks(&pool, "zzz").await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn prune_removes_only_missing_paths() {
        let pool = pool().await;
        let a = track("m/a.flac", None, None);
        let b = track("m/b.flac", None, None);
        upsert_track(&pool, &a).await.unwrap();
        upsert_track(&pool, &b).await.unwrap();
        let removed = prune_missing(&pool, &["m/a.flac".into()]).await.unwrap();
        assert_eq!(removed, 1);
        let all = all_tracks(&pool).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].path, "m/a.flac");
    }

    #[tokio::test]
    async fn playlist_roundtrip_preserves_order_and_counts() {
        let pool = pool().await;
        let a = track("m/a.flac", Some("A"), None);
        let b = track("m/b.flac", Some("B"), None);
        let c = track("m/c.flac", Some("C"), None);
        for t in [&a, &b, &c] {
            upsert_track(&pool, t).await.unwrap();
        }
        let pl = create_playlist(&pool, "Road Trip").await.unwrap();
        set_playlist_tracks(&pool, &pl, &[c.id, a.id, b.id])
            .await
            .unwrap();
        let listed = all_playlists(&pool).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "Road Trip");
        assert_eq!(listed[0].track_count, 3);
        let tracks = playlist_tracks(&pool, &pl).await.unwrap();
        assert_eq!(
            tracks.iter().map(|t| t.path.as_str()).collect::<Vec<_>>(),
            vec!["m/c.flac", "m/a.flac", "m/b.flac"]
        );

        // Re-setting replaces, does not append.
        set_playlist_tracks(&pool, &pl, &[a.id]).await.unwrap();
        assert_eq!(playlist_tracks(&pool, &pl).await.unwrap().len(), 1);

        rename_playlist(&pool, &pl, "Late Night").await.unwrap();
        delete_playlist(&pool, &pl).await.unwrap();
        assert!(all_playlists(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn settings_roundtrip_and_idempotent_roots() {
        let pool = pool().await;
        assert_eq!(get_setting(&pool, "output_device").await.unwrap(), None);
        set_setting(&pool, "output_device", "wasapi:default")
            .await
            .unwrap();
        assert_eq!(
            get_setting(&pool, "output_device")
                .await
                .unwrap()
                .as_deref(),
            Some("wasapi:default")
        );
        // Upsert overwrites.
        set_setting(&pool, "output_device", "wasapi:speakers")
            .await
            .unwrap();
        assert_eq!(
            get_setting(&pool, "output_device")
                .await
                .unwrap()
                .as_deref(),
            Some("wasapi:speakers")
        );

        add_library_root(&pool, "C:/music").await.unwrap();
        add_library_root(&pool, "C:/music").await.unwrap(); // no dupe
        add_library_root(&pool, "D:/flac").await.unwrap();
        assert_eq!(
            library_roots(&pool).await.unwrap(),
            vec!["C:/music".to_string(), "D:/flac".to_string()]
        );
    }

    #[tokio::test]
    async fn replaygain_columns_roundtrip() {
        let pool = pool().await;
        let mut t = track("m/rg.flac", Some("RG"), None);
        t.replaygain_track_db = Some(-6.53);
        t.replaygain_album_db = Some(-5.8);
        upsert_track(&pool, &t).await.unwrap();
        let all = all_tracks(&pool).await.unwrap();
        assert!((all[0].replaygain_track_db.unwrap() + 6.53).abs() < 1e-9);
        assert!((all[0].replaygain_album_db.unwrap() + 5.8).abs() < 1e-9);
    }
}
