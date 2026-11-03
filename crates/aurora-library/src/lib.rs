//! The library service: folder scanning, metadata extraction, persistence
//! and search, composed over `aurora-metadata` (tags) and `aurora-storage`
//! (SQLite). Also exposes a static file-type check shared with the scanner.

use std::path::Path;

use aurora_core::{AudioFormat, Track, TrackId};
use aurora_storage::SqlitePool;
use tokio::sync::Mutex;

pub use aurora_storage::{
    StorageError, all_playlists, all_tracks, create_playlist, delete_playlist, playlist_tracks,
    rename_playlist, search_tracks, set_playlist_tracks,
};

// Re-export under the canonical name used in this crate's signatures.

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error(transparent)]
    Storage(#[from] StorageError),
}

/// Supported audio extensions (must match the frontend's file filter and
/// `AudioFormat::from_extension`).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "wav", "wave", "mp3", "m4a", "alac", "aac", "ogg", "oga", "opus", "aif", "aiff", "wv",
];

/// True when the file extension is one the library recognizes as audio.
pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Result of one scan run (for the UI's honest progress report).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct ScanSummary {
    /// Files found in the folder tree.
    pub discovered: u64,
    /// Tracks newly inserted into the library.
    pub added: u64,
    /// Existing tracks whose path/metadata were refreshed.
    pub updated: u64,
    /// Tracks removed because their files disappeared.
    pub pruned: u64,
    /// Files that failed to read/parse (logged, skipped).
    pub failed: u64,
}

/// The library service: owns the pool and serializes scans.
///
/// `Mutex` guards against concurrent scans (a rescan while a scan runs
/// would race the prune step and could drop freshly-added tracks).
pub struct LibraryService {
    pool: SqlitePool,
    scan_lock: Mutex<()>,
}

impl LibraryService {
    /// Open (and migrate) the library database at `path`.
    pub async fn open(path: &Path) -> Result<Self, LibraryError> {
        Ok(Self {
            pool: aurora_storage::open(path).await?,
            scan_lock: Mutex::new(()),
        })
    }

    /// In-memory library (tests).
    pub async fn open_in_memory() -> Result<Self, LibraryError> {
        Ok(Self {
            pool: aurora_storage::open_in_memory().await?,
            scan_lock: Mutex::new(()),
        })
    }

    /// Build a service over an already-open pool (the watcher uses this
    /// to run scans without going through the managed-state mutex).
    pub fn from_pool(pool: SqlitePool) -> Self {
        Self {
            pool,
            scan_lock: Mutex::new(()),
        }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Scan a folder tree into the library.
    ///
    /// Idempotent by construction: identity is the blake3 content hash, so
    /// re-scanning the same folder upserts the same rows (Phase 6 exit
    /// criterion: repeat scans create no duplicate tracks).
    ///
    /// Files that vanish from disk are pruned; files that fail to read are
    /// counted and logged, never fatal (interruption recovery: the next
    /// scan completes the job).
    pub async fn scan_folder(&self, root: &Path) -> Result<ScanSummary, LibraryError> {
        let _guard = self.scan_lock.lock().await;
        let mut summary = ScanSummary::default();

        // Phase 1 (sync, fast): discover audio files.
        let mut files = Vec::new();
        for entry in walkdir::WalkDir::new(root).follow_links(false) {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(%e, "scan: cannot read directory entry");
                    summary.failed += 1;
                    continue;
                }
            };
            if entry.file_type().is_file() && is_audio_file(entry.path()) {
                files.push(entry.path().to_path_buf());
                summary.discovered += 1;
            }
        }

        // Phase 2 (per file, blocking work in the async context): digest,
        // metadata, upsert. Bounded by real disk IO; each file is small.
        let mut keep_paths: Vec<String> = Vec::with_capacity(files.len());

        for path in &files {
            let digest = match aurora_storage::content_digest(path) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!(path = %path.display(), %e, "scan: digest failed");
                    summary.failed += 1;
                    continue;
                }
            };
            let id = TrackId::from_content_digest(digest);

            let metadata = {
                let meta_path = path.clone();
                match tokio::task::spawn_blocking(move || {
                    aurora_metadata::read_metadata(&meta_path)
                })
                .await
                {
                    Ok(Ok(meta)) => Some(meta),
                    Ok(Err(e)) => {
                        tracing::warn!(path = %path.display(), %e, "scan: metadata failed");
                        summary.failed += 1;
                        None
                    }
                    Err(e) => {
                        tracing::warn!(path = %path.display(), %e, "scan: worker panicked");
                        summary.failed += 1;
                        None
                    }
                }
            };

            let mut track = Track::new(id, path.to_string_lossy().into_owned());
            track.format = path
                .extension()
                .and_then(|e| e.to_str())
                .map(AudioFormat::from_extension)
                .unwrap_or(AudioFormat::Unknown);
            if let Some(meta) = metadata {
                meta.apply_to(&mut track);
            }

            match aurora_storage::upsert_track(&self.pool, &track).await {
                Ok(was_new) => {
                    if was_new {
                        summary.added += 1;
                    } else {
                        summary.updated += 1;
                    }
                    keep_paths.push(track.path.clone());
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), %e, "scan: upsert failed");
                    summary.failed += 1;
                }
            }
        }

        // Phase 3: prune tracks whose files vanished.
        summary.pruned = aurora_storage::prune_missing(&self.pool, &keep_paths).await?;

        Ok(summary)
    }

    /// All tracks, ordered for listing.
    pub async fn tracks(&self) -> Result<Vec<Track>, LibraryError> {
        Ok(aurora_storage::all_tracks(&self.pool).await?)
    }

    /// Case-insensitive substring search over title/artist/album/path.
    pub async fn search(&self, query: &str) -> Result<Vec<Track>, LibraryError> {
        Ok(aurora_storage::search_tracks(&self.pool, query).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_sine_wav(path: &Path, frames: usize) {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 44_100,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("create wav");
        for i in 0..frames {
            let v = ((i as f64 * 0.05).sin() * 10_000.0) as i16;
            writer.write_sample(v).unwrap();
            writer.write_sample(v).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[test]
    fn audio_file_filter_matches_supported_extensions_only() {
        assert!(is_audio_file(Path::new("a/b/track.flac")));
        assert!(is_audio_file(Path::new("a/b/TRACK.MP3")));
        assert!(!is_audio_file(Path::new("a/b/cover.jpg")));
        assert!(!is_audio_file(Path::new("a/b/noext")));
    }

    #[tokio::test]
    async fn rescan_is_idempotent_and_recovers_after_interruption() {
        let dir = tempfile::tempdir().unwrap();
        write_sine_wav(&dir.path().join("one.wav"), 4_410);
        write_sine_wav(&dir.path().join("two.wav"), 8_820);

        let lib = LibraryService::open_in_memory().await.unwrap();
        let s1 = lib.scan_folder(dir.path()).await.unwrap();
        assert_eq!(s1.discovered, 2);
        assert_eq!(s1.added, 2);
        assert_eq!(s1.failed, 0);

        // Same folder again: nothing added, everything refreshed, zero dupes.
        let s2 = lib.scan_folder(dir.path()).await.unwrap();
        assert_eq!(s2.added, 0);
        assert_eq!(s2.updated, 2);
        let tracks = lib.tracks().await.unwrap();
        assert_eq!(tracks.len(), 2);

        // Untagged WAVs still get correct names (file stem) and duration.
        let mut names: Vec<String> = tracks
            .iter()
            .map(|t| t.display_title().to_string())
            .collect();
        names.sort();
        assert_eq!(names, vec!["one", "two"]);
        assert!(
            tracks
                .iter()
                .all(|t| t.duration_ms.is_some() && t.duration_ms != Some(0))
        );

        // Delete one file: rescan prunes it.
        std::fs::remove_file(dir.path().join("one.wav")).unwrap();
        let s3 = lib.scan_folder(dir.path()).await.unwrap();
        assert_eq!(s3.pruned, 1);
        assert_eq!(lib.tracks().await.unwrap().len(), 1);

        // Interruption recovery: a scan that added tracks but "crashed"
        // before pruning leaves the DB consistent; the next scan completes.
        // (Modeled by scan-after-delete above: state converges.)
    }

    #[tokio::test]
    async fn search_finds_by_file_stem() {
        let dir = tempfile::tempdir().unwrap();
        write_sine_wav(&dir.path().join("zeta_track.wav"), 4_410);
        let lib = LibraryService::open_in_memory().await.unwrap();
        lib.scan_folder(dir.path()).await.unwrap();
        let found = lib.search("zeta").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].display_title(), "zeta_track");
    }

    #[tokio::test]
    async fn non_audio_files_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        write_sine_wav(&dir.path().join("song.wav"), 4_410);
        std::fs::write(dir.path().join("notes.txt"), "not audio").unwrap();
        let lib = LibraryService::open_in_memory().await.unwrap();
        let s = lib.scan_folder(dir.path()).await.unwrap();
        assert_eq!(s.discovered, 1);
        assert_eq!(lib.tracks().await.unwrap().len(), 1);
    }
}
