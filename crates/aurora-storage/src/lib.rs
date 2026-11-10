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

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
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
    #[error("invalid encryption key")]
    EncryptionKey,
    #[error("encrypted data could not be processed")]
    Encryption,
    #[error("value could not be serialized: {0}")]
    Serialization(#[from] serde_json::Error),
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
    let options = |journal: SqliteJournalMode| {
        SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(journal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
    };

    // WAL is the preferred mode, but some PaaS container filesystems
    // (Render's native runtime overlay on /tmp) reject the wal/shm
    // sidecar files with SQLITE_CANTOPEN. Fall back to the rollback
    // journal: slower writes, same durability guarantees, works
    // everywhere.
    let pool = match SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(options(SqliteJournalMode::Wal))
        .await
    {
        Ok(pool) => pool,
        Err(_) => {
            SqlitePoolOptions::new()
                .max_connections(4)
                .connect_with(options(SqliteJournalMode::Delete))
                .await?
        }
    };

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

/// Grace period before a *consumed* OAuth transaction row is deleted.
///
/// The row is what keeps a replayed `state` answerable as "already used"
/// rather than "unknown"; deleting it immediately on the first callback would
/// erase the difference. It also bounds how long the verifier, which is a
/// credential stored in the clear, survives at rest.
pub const CONSUMED_GRACE_SECS: i64 = 600;

#[derive(Debug, Clone)]
pub struct OAuthSession {
    pub csrf_hash: String,
    pub expires_at: String,
}
#[derive(Debug, Clone)]
pub struct OAuthTransaction {
    pub provider: String,
    pub session_id_hash: String,
    pub verifier: String,
    pub redirect_uri: String,
    pub expires_at: String,
}
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OAuthAccount {
    pub subject: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub capabilities: Vec<String>,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub auth_state: String,
    pub last_auth_error: Option<String>,
}

pub fn encrypt_oauth_tokens(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, StorageError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| StorageError::EncryptionKey)?;
    let nonce_bytes = uuid::Uuid::new_v4();
    let nonce = Nonce::from_slice(&nonce_bytes.as_bytes()[..12]);
    let encrypted = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| StorageError::Encryption)?;
    let mut result = nonce.to_vec();
    result.extend(encrypted);
    Ok(result)
}
pub fn decrypt_oauth_tokens(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, StorageError> {
    if ciphertext.len() < 12 {
        return Err(StorageError::Encryption);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| StorageError::EncryptionKey)?;
    cipher
        .decrypt(Nonce::from_slice(&ciphertext[..12]), &ciphertext[12..])
        .map_err(|_| StorageError::Encryption)
}
pub async fn health_check(pool: &SqlitePool) -> Result<(), StorageError> {
    sqlx::query("SELECT 1").execute(pool).await?;
    Ok(())
}

/// Opportunistic cleanup of expired OAuth state.
///
/// Consumed transactions are retained for `CONSUMED_GRACE_SECS` so a replay
/// still distinguishes "already used" from "never existed"; both tables are
/// pruned in one transaction so the admin path never sees half-cleaned state.
///
/// Returns `(sessions_deleted, transactions_deleted)`.
pub async fn cleanup_expired_oauth(
    pool: &SqlitePool,
    now: &str,
) -> Result<(u64, u64), StorageError> {
    let mut tx = pool.begin().await?;
    let sessions = sqlx::query("DELETE FROM oauth_sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let cutoff = consumed_cutoff(now);
    let transactions = sqlx::query(
        "DELETE FROM oauth_transactions \
         WHERE expires_at <= ? OR (consumed_at IS NOT NULL AND consumed_at <= ?)",
    )
    .bind(now)
    .bind(&cutoff)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok((sessions, transactions))
}

/// Timestamp before which consumed transactions may be deleted.
///
/// Falls back to `now` (delete everything consumed) when `now` cannot be
/// parsed — the safe direction for a cleanup, never for an authorization check.
fn consumed_cutoff(now: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(now)
        .map(|t| {
            (t - chrono::Duration::seconds(CONSUMED_GRACE_SECS))
                .to_utc()
                .to_rfc3339()
        })
        .unwrap_or_else(|_| now.to_owned())
}

pub async fn validate_oauth_csrf(
    pool: &SqlitePool,
    id_hash: &str,
    csrf_hash: &str,
    now: &str,
) -> Result<bool, StorageError> {
    let row = sqlx::query(
        "SELECT 1 FROM oauth_sessions WHERE id_hash=? AND csrf_hash=? AND expires_at>?",
    )
    .bind(id_hash)
    .bind(csrf_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

pub async fn save_oauth_session(
    pool: &SqlitePool,
    id_hash: &str,
    csrf_hash: &str,
    expires_at: &str,
) -> Result<(), StorageError> {
    sqlx::query("INSERT INTO oauth_sessions (id_hash, csrf_hash, expires_at, created_at) VALUES (?, ?, ?, ?) ON CONFLICT(id_hash) DO UPDATE SET csrf_hash=excluded.csrf_hash, expires_at=excluded.expires_at").bind(id_hash).bind(csrf_hash).bind(expires_at).bind(chrono::Utc::now().to_rfc3339()).execute(pool).await?;
    Ok(())
}
pub async fn get_oauth_session(
    pool: &SqlitePool,
    id_hash: &str,
    now: &str,
) -> Result<Option<OAuthSession>, StorageError> {
    let r = sqlx::query(
        "SELECT csrf_hash, expires_at FROM oauth_sessions WHERE id_hash=? AND expires_at>?",
    )
    .bind(id_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(r.map(|x| OAuthSession {
        csrf_hash: x.try_get("csrf_hash").unwrap_or_default(),
        expires_at: x.try_get("expires_at").unwrap_or_default(),
    }))
}
pub async fn save_oauth_transaction(
    pool: &SqlitePool,
    state_hash: &str,
    tx: &OAuthTransaction,
) -> Result<(), StorageError> {
    sqlx::query("INSERT INTO oauth_transactions (state_hash, provider, session_id_hash, verifier, redirect_uri, expires_at) VALUES (?, ?, ?, ?, ?, ?)").bind(state_hash).bind(&tx.provider).bind(&tx.session_id_hash).bind(&tx.verifier).bind(&tx.redirect_uri).bind(&tx.expires_at).execute(pool).await?;
    Ok(())
}
pub async fn claim_oauth_transaction(
    pool: &SqlitePool,
    state_hash: &str,
    provider: &str,
    session_hash: &str,
    now: &str,
) -> Result<Option<OAuthTransaction>, StorageError> {
    let r=sqlx::query("UPDATE oauth_transactions SET consumed_at=? WHERE state_hash=? AND provider=? AND session_id_hash=? AND consumed_at IS NULL AND expires_at>? RETURNING provider, session_id_hash, verifier, redirect_uri, expires_at").bind(now).bind(state_hash).bind(provider).bind(session_hash).bind(now).fetch_optional(pool).await?;
    Ok(r.map(|x| OAuthTransaction {
        provider: x.try_get("provider").unwrap_or_default(),
        session_id_hash: x.try_get("session_id_hash").unwrap_or_default(),
        verifier: x.try_get("verifier").unwrap_or_default(),
        redirect_uri: x.try_get("redirect_uri").unwrap_or_default(),
        expires_at: x.try_get("expires_at").unwrap_or_default(),
    }))
}
/// Erases the PKCE verifier of a consumed OAuth transaction.
///
/// The consumed row is retained (replay detection needs it; see
/// [`cleanup_expired_oauth`]) but the verifier is a credential: once the
/// transaction is claimed the code is single-use and the verifier can never
/// redeem anything again, so leaving it in the clear at rest is pure
/// exposure. An unconsumed row is left untouched - its verifier is still
/// live for a pending callback. Idempotent: scrubbing twice is fine.
pub async fn clear_oauth_verifier(pool: &SqlitePool, state_hash: &str) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE oauth_transactions SET verifier='' WHERE state_hash=? AND consumed_at IS NOT NULL",
    )
    .bind(state_hash)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn save_oauth_account(
    pool: &SqlitePool,
    user_hash: &str,
    provider: &str,
    account: &OAuthAccount,
    keyring_account_id: &str,
    token_ciphertext: &[u8],
) -> Result<(), StorageError> {
    let mut tx = pool.begin().await?;
    // Serialization failure aborts the transaction: persisting an empty
    // capability list would silently downgrade the account's granted scopes,
    // and the UI reads capabilities as the truth about what the grant covers.
    let capabilities_json = serde_json::to_string(&account.capabilities)?;
    sqlx::query("INSERT INTO oauth_accounts (user_hash, provider, subject, display_name, avatar_url, capabilities_json, connected_at, keyring_account_id, token_ciphertext, auth_state, last_auth_error) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(user_hash, provider) DO UPDATE SET subject=excluded.subject, display_name=excluded.display_name, avatar_url=excluded.avatar_url, capabilities_json=excluded.capabilities_json, connected_at=excluded.connected_at, keyring_account_id=excluded.keyring_account_id, token_ciphertext=excluded.token_ciphertext, auth_state='connected', last_auth_error=NULL").bind(user_hash).bind(provider).bind(&account.subject).bind(&account.display_name).bind(&account.avatar_url).bind(capabilities_json).bind(account.connected_at.to_rfc3339()).bind(keyring_account_id).bind(token_ciphertext).bind("connected").bind(Option::<String>::None).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
pub async fn get_oauth_account(
    pool: &SqlitePool,
    user_hash: &str,
    provider: &str,
) -> Result<Option<OAuthAccount>, StorageError> {
    let r=sqlx::query("SELECT subject, display_name, avatar_url, capabilities_json, connected_at, auth_state, last_auth_error FROM oauth_accounts WHERE user_hash=? AND provider=?").bind(user_hash).bind(provider).fetch_optional(pool).await?;
    Ok(r.and_then(|x| {
        Some(OAuthAccount {
            subject: x.try_get("subject").ok()?,
            display_name: x.try_get("display_name").ok()?,
            avatar_url: x.try_get("avatar_url").ok()?,
            capabilities: serde_json::from_str(&x.try_get::<String, _>("capabilities_json").ok()?)
                .ok()?,
            connected_at: x.try_get::<String, _>("connected_at").ok()?.parse().ok()?,
            auth_state: x
                .try_get::<String, _>("auth_state")
                .unwrap_or_else(|_| "connected".into()),
            last_auth_error: x.try_get("last_auth_error").ok(),
        })
    }))
}
/// Read the stored (encrypted) token blob for one account.
///
/// `Ok(None)` means exactly one thing: no row for `(user_hash, provider)`.
/// A row that exists but whose column cannot be read is a hard error — the
/// caller must not be told the account is absent, because "absent" maps to a
/// 404 for the user and would present a corrupt account as a disconnected one.
pub async fn get_oauth_token_ciphertext(
    pool: &SqlitePool,
    user_hash: &str,
    provider: &str,
) -> Result<Option<Vec<u8>>, StorageError> {
    let row =
        sqlx::query("SELECT token_ciphertext FROM oauth_accounts WHERE user_hash=? AND provider=?")
            .bind(user_hash)
            .bind(provider)
            .fetch_optional(pool)
            .await?;
    match row {
        Some(x) => Ok(Some(x.try_get::<Vec<u8>, _>("token_ciphertext")?)),
        None => Ok(None),
    }
}

pub async fn update_oauth_tokens(
    pool: &SqlitePool,
    user_hash: &str,
    provider: &str,
    token_ciphertext: &[u8],
) -> Result<bool, StorageError> {
    let result = sqlx::query("UPDATE oauth_accounts SET token_ciphertext=?, auth_state='connected', last_auth_error=NULL WHERE user_hash=? AND provider=?")
        .bind(token_ciphertext).bind(user_hash).bind(provider).execute(pool).await?;
    Ok(result.rows_affected() == 1)
}

pub async fn mark_oauth_reauth_required(
    pool: &SqlitePool,
    user_hash: &str,
    provider: &str,
    error: &str,
) -> Result<(), StorageError> {
    sqlx::query("UPDATE oauth_accounts SET auth_state='reauthorization_required', last_auth_error=? WHERE user_hash=? AND provider=?")
        .bind(error.chars().take(200).collect::<String>()).bind(user_hash).bind(provider).execute(pool).await?;
    Ok(())
}

pub async fn delete_oauth_account(
    pool: &SqlitePool,
    user_hash: &str,
    provider: &str,
) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM oauth_accounts WHERE user_hash=? AND provider=?")
        .bind(user_hash)
        .bind(provider)
        .execute(pool)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Email/password accounts (migration 0007)
// ---------------------------------------------------------------------------

/// A row of the `users` table.
#[derive(Debug, Clone)]
pub struct User {
    pub id: i64,
    pub email: String,
}

/// Insert a new user; `Ok(None)` when the email is already registered.
///
/// The UNIQUE constraint is the arbiter (not a pre-read), so two concurrent
/// registrations of the same email cannot both succeed.
pub async fn insert_user(
    pool: &SqlitePool,
    email: &str,
    password_hash: &str,
) -> Result<Option<i64>, StorageError> {
    // INSERT OR IGNORE: a UNIQUE hit on `email` is the "already registered"
    // outcome, not an error -- surfaced as `Ok(None)` instead of an Err so a
    // taken address is a 409 rather than a 500.
    let result = sqlx::query(
        "INSERT OR IGNORE INTO users (email, password_hash, created_at) VALUES (?, ?, ?)",
    )
    .bind(email)
    .bind(password_hash)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    // SQLite reuses rowids after a DELETE; LAST_INSERT_ROWID is only
    // meaningful when a row was actually inserted (rows_affected == 1).
    Ok((result.rows_affected() == 1).then(|| result.last_insert_rowid()))
}

pub async fn get_user_by_email(
    pool: &SqlitePool,
    email: &str,
) -> Result<Option<User>, StorageError> {
    let row = sqlx::query("SELECT id, email FROM users WHERE email=?")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| User {
        id: r.try_get("id").unwrap_or_default(),
        email: r.try_get("email").unwrap_or_default(),
    }))
}

/// Insert a login session; upsertes by id hash so a re-mint replaces cleanly.
pub async fn insert_login_session(
    pool: &SqlitePool,
    id_hash: &str,
    user_id: i64,
    csrf_hash: &str,
    expires_at: &str,
) -> Result<(), StorageError> {
    sqlx::query(
        "INSERT INTO login_sessions (id_hash, user_id, csrf_hash, expires_at, created_at) 
         VALUES (?, ?, ?, ?, ?) ON CONFLICT(id_hash) DO UPDATE SET 
         user_id=excluded.user_id, csrf_hash=excluded.csrf_hash, 
         expires_at=excluded.expires_at",
    )
    .bind(id_hash)
    .bind(user_id)
    .bind(csrf_hash)
    .bind(expires_at)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// CSRF validation for login sessions: the header token's hash must match the
/// row's `csrf_hash` while the session is live. Mirrors
/// `validate_oauth_csrf` exactly.
pub async fn validate_login_csrf(
    pool: &SqlitePool,
    id_hash: &str,
    csrf_hash: &str,
    now: &str,
) -> Result<bool, StorageError> {
    let row = sqlx::query(
        "SELECT 1 FROM login_sessions WHERE id_hash=? AND csrf_hash=? AND expires_at>?",
    )
    .bind(id_hash)
    .bind(csrf_hash)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

/// The user a live login session belongs to. `Ok(None)` means no such session
/// or it is expired; the caller cannot distinguish the two, by design.
pub async fn get_login_session(
    pool: &SqlitePool,
    id_hash: &str,
    now: &str,
) -> Result<Option<i64>, StorageError> {
    let row = sqlx::query("SELECT user_id FROM login_sessions WHERE id_hash=? AND expires_at>?")
        .bind(id_hash)
        .bind(now)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.try_get("user_id").ok()))
}

/// The stored password hash (PHC string) for a user id. Separate from the
/// `User` read so login can fetch exactly the one credential it verifies.
pub async fn get_user_password_hash(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Option<String>, StorageError> {
    let row = sqlx::query("SELECT password_hash FROM users WHERE id=?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.and_then(|r| r.try_get("password_hash").ok()))
}

pub async fn get_user_by_id(pool: &SqlitePool, id: i64) -> Result<Option<User>, StorageError> {
    let row = sqlx::query("SELECT id, email FROM users WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| User {
        id: r.try_get("id").unwrap_or_default(),
        email: r.try_get("email").unwrap_or_default(),
    }))
}

pub async fn delete_login_session(pool: &SqlitePool, id_hash: &str) -> Result<(), StorageError> {
    sqlx::query("DELETE FROM login_sessions WHERE id_hash=?")
        .bind(id_hash)
        .execute(pool)
        .await?;
    Ok(())
}

/// Moves every stored OAuth grant from one user hash to another, in one
/// transaction. Idempotent: re-claiming an already-moved hash is a no-op.
/// Grants already owned by `new_user_hash` stay exactly where they are.
pub async fn claim_grants(
    pool: &SqlitePool,
    old_user_hash: &str,
    new_user_hash: &str,
) -> Result<u64, StorageError> {
    let mut tx = pool.begin().await?;
    let affected =
        sqlx::query("UPDATE oauth_accounts SET user_hash=? WHERE user_hash=? AND user_hash<>?")
            .bind(new_user_hash)
            .bind(old_user_hash)
            .bind(new_user_hash)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    tx.commit().await?;
    Ok(affected)
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
    track.lyrics = row.try_get::<Option<String>, _>("lyrics").ok().flatten();
    track.lyrics_language = row
        .try_get::<Option<String>, _>("lyrics_language")
        .ok()
        .flatten();
    track.lyrics_translation = row
        .try_get::<Option<String>, _>("lyrics_translation")
        .ok()
        .flatten();
    track.lyrics_translation_language = row
        .try_get::<Option<String>, _>("lyrics_translation_language")
        .ok()
        .flatten();
    track.artwork_data_url = row
        .try_get::<Option<String>, _>("artwork_data_url")
        .ok()
        .flatten();
    track.source_url = row
        .try_get::<Option<String>, _>("source_url")
        .ok()
        .flatten();
    track.source_provider = row
        .try_get::<Option<String>, _>("source_provider")
        .ok()
        .flatten();
    track.video_url = row.try_get::<Option<String>, _>("video_url").ok().flatten();
    track.metadata_source = row
        .try_get::<Option<String>, _>("metadata_source")
        .ok()
        .flatten();
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
            format, duration_ms, replaygain_track_db, replaygain_album_db, bpm,
            lyrics, lyrics_language, lyrics_translation, lyrics_translation_language,
            artwork_data_url, source_url, source_provider, video_url, metadata_source
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
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
            bpm = COALESCE(excluded.bpm, tracks.bpm),
            lyrics = COALESCE(excluded.lyrics, tracks.lyrics),
            lyrics_language = COALESCE(excluded.lyrics_language, tracks.lyrics_language),
            lyrics_translation = COALESCE(excluded.lyrics_translation, tracks.lyrics_translation),
            lyrics_translation_language = COALESCE(excluded.lyrics_translation_language, tracks.lyrics_translation_language),
            artwork_data_url = COALESCE(excluded.artwork_data_url, tracks.artwork_data_url),
            source_url = COALESCE(excluded.source_url, tracks.source_url),
            source_provider = COALESCE(excluded.source_provider, tracks.source_provider),
            video_url = COALESCE(excluded.video_url, tracks.video_url),
            metadata_source = COALESCE(excluded.metadata_source, tracks.metadata_source)
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
    .bind(&track.lyrics)
    .bind(&track.lyrics_language)
    .bind(&track.lyrics_translation)
    .bind(&track.lyrics_translation_language)
    .bind(&track.artwork_data_url)
    .bind(&track.source_url)
    .bind(&track.source_provider)
    .bind(&track.video_url)
    .bind(&track.metadata_source)
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

/// User-edited or provider-enriched lyrics and media links for a track.
#[derive(Debug, Clone, Default)]
pub struct TrackEnrichment {
    pub lyrics: Option<String>,
    pub lyrics_language: Option<String>,
    pub lyrics_translation: Option<String>,
    pub lyrics_translation_language: Option<String>,
    pub artwork_data_url: Option<String>,
    pub source_url: Option<String>,
    pub source_provider: Option<String>,
    pub video_url: Option<String>,
    pub metadata_source: Option<String>,
}

pub async fn set_track_enrichment(
    pool: &SqlitePool,
    track_id: &str,
    enrichment: &TrackEnrichment,
) -> Result<(), StorageError> {
    sqlx::query(
        "UPDATE tracks SET lyrics = ?, lyrics_language = ?, lyrics_translation = ?, \
         lyrics_translation_language = ?, artwork_data_url = ?, source_url = ?, \
         source_provider = ?, video_url = ?, metadata_source = ? WHERE id = ?",
    )
    .bind(&enrichment.lyrics)
    .bind(&enrichment.lyrics_language)
    .bind(&enrichment.lyrics_translation)
    .bind(&enrichment.lyrics_translation_language)
    .bind(&enrichment.artwork_data_url)
    .bind(&enrichment.source_url)
    .bind(&enrichment.source_provider)
    .bind(&enrichment.video_url)
    .bind(&enrichment.metadata_source)
    .bind(track_id)
    .execute(pool)
    .await?;
    Ok(())
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

/// Measured loudness (ReplayGain 2.0 / R128) persistence.
mod loudness;
pub use loudness::set_track_replaygain;

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
    async fn track_enrichment_roundtrip_survives_rescan() {
        let pool = pool().await;
        let t = track("m/enriched.flac", Some("Enriched"), Some("Aurora"));
        upsert_track(&pool, &t).await.unwrap();

        let enrichment = TrackEnrichment {
            lyrics: Some("[00:01.00] hello".into()),
            lyrics_language: Some("en".into()),
            lyrics_translation: Some("привет".into()),
            lyrics_translation_language: Some("ru".into()),
            artwork_data_url: Some("data:image/jpeg;base64,abc".into()),
            source_url: Some("https://example.test/track".into()),
            source_provider: Some("example".into()),
            video_url: Some("https://example.test/video".into()),
            metadata_source: Some("user".into()),
        };
        set_track_enrichment(&pool, &t.id.0.to_string(), &enrichment)
            .await
            .unwrap();

        let saved = all_tracks(&pool).await.unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].lyrics.as_deref(), Some("[00:01.00] hello"));
        assert_eq!(saved[0].lyrics_translation.as_deref(), Some("привет"));
        assert_eq!(
            saved[0].artwork_data_url.as_deref(),
            Some("data:image/jpeg;base64,abc")
        );
        assert_eq!(saved[0].source_provider.as_deref(), Some("example"));
        assert_eq!(
            saved[0].video_url.as_deref(),
            Some("https://example.test/video")
        );

        // A normal rescan/upsert with no enrichment must retain the saved fields.
        upsert_track(&pool, &t).await.unwrap();
        let rescanned = all_tracks(&pool).await.unwrap();
        assert_eq!(rescanned[0].lyrics.as_deref(), Some("[00:01.00] hello"));
        assert_eq!(rescanned[0].metadata_source.as_deref(), Some("user"));
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

    #[tokio::test]
    async fn clear_oauth_verifier_scrubs_only_consumed_rows() {
        let pool = pool().await;
        let now = chrono::Utc::now().to_rfc3339();
        let tx = OAuthTransaction {
            provider: "spotify".into(),
            session_id_hash: "session-hash".into(),
            verifier: "live-verifier".into(),
            redirect_uri: "https://api.example/api/oauth/spotify/callback".into(),
            expires_at: (chrono::Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
        };
        save_oauth_transaction(&pool, "hash-consumed", &tx)
            .await
            .unwrap();
        save_oauth_transaction(&pool, "hash-pending", &tx)
            .await
            .unwrap();

        // Scrubbing before the claim must not touch the pending verifier.
        clear_oauth_verifier(&pool, "hash-pending").await.unwrap();
        assert_eq!(
            stored_verifier(&pool, "hash-pending").await,
            "live-verifier"
        );

        let claimed =
            claim_oauth_transaction(&pool, "hash-consumed", "spotify", "session-hash", &now)
                .await
                .unwrap()
                .expect("the transaction is claimable");
        assert_eq!(
            claimed.verifier, "live-verifier",
            "the claim still returns the verifier"
        );

        clear_oauth_verifier(&pool, "hash-consumed").await.unwrap();
        assert_eq!(
            stored_verifier(&pool, "hash-consumed").await,
            "",
            "the consumed verifier must be erased"
        );
        // Idempotent: a second scrub is a no-op, not an error.
        clear_oauth_verifier(&pool, "hash-consumed").await.unwrap();
        // An unknown state hash is also fine (never existed or already pruned).
        clear_oauth_verifier(&pool, "hash-unknown").await.unwrap();
    }

    async fn stored_verifier(pool: &SqlitePool, state_hash: &str) -> String {
        let row: (String,) =
            sqlx::query_as("SELECT verifier FROM oauth_transactions WHERE state_hash=?")
                .bind(state_hash)
                .fetch_one(pool)
                .await
                .expect("transaction row exists");
        row.0
    }
}
