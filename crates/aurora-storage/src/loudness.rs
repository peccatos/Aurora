//! Measured loudness persistence (ReplayGain 2.0 / R128).
//!
//! Owns the `replaygain_track_db` column from the measurement side: the
//! value written here is a MEASUREMENT of the decoded content (integrated
//! LUFS, expressed as an rg2 dB gain against the -18 LUFS target), never
//! a tag echo. A NULL gain is a no-op - a failed measurement is not
//! stored as a guess.
//!
//! Native only (SQLite); the wasm shell persists loudness in IndexedDB.
#[cfg(not(target_family = "wasm"))]
mod imp {
    use sqlx::SqlitePool;

    /// Persist the measured ReplayGain 2.0 track gain for a track
    /// (path-keyed, the `set_track_bpm` pattern). `None` is a no-op:
    /// the measurement failed and we do not store guesses.
    pub async fn set_track_replaygain(
        pool: &SqlitePool,
        path: &str,
        gain_db: Option<f64>,
    ) -> Result<(), crate::StorageError> {
        if let Some(gain_db) = gain_db {
            sqlx::query("UPDATE tracks SET replaygain_track_db = ? WHERE path = ?")
                .bind(gain_db)
                .bind(path)
                .execute(pool)
                .await?;
        }
        Ok(())
    }
}

#[cfg(not(target_family = "wasm"))]
pub use imp::set_track_replaygain;
