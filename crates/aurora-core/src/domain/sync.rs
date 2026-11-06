//! Provider-neutral playlist synchronization contracts.
//!
//! This module deliberately contains no HTTP or persistence code. Providers map
//! their API responses into these DTOs, then execute the returned operations.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::library::TrackId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteProvider {
    Spotify,
    Youtube,
    Deezer,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTrack {
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePlaylist {
    pub provider: RemoteProvider,
    pub id: String,
    pub name: String,
    pub tracks: Vec<RemoteTrack>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalPlaylistTrack {
    pub id: TrackId,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub position: u32,
}

/// A provider page uses an opaque cursor. `None` means there is no next page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub snapshot: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncCapabilities {
    pub can_read: bool,
    pub can_create: bool,
    pub can_add_tracks: bool,
    pub can_remove_tracks: bool,
    pub can_reorder_tracks: bool,
    pub can_rename: bool,
}

impl SyncCapabilities {
    pub const READ_ONLY: Self = Self {
        can_read: true,
        can_create: false,
        can_add_tracks: false,
        can_remove_tracks: false,
        can_reorder_tracks: false,
        can_rename: false,
    };

    pub const FULL: Self = Self {
        can_read: true,
        can_create: true,
        can_add_tracks: true,
        can_remove_tracks: true,
        can_reorder_tracks: true,
        can_rename: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictPolicy {
    LocalWins,
    RemoteWins,
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay_ms: 250,
            max_delay_ms: 8_000,
        }
    }
}

impl RetryPolicy {
    pub fn delay_for_retry(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        if let Some(delay) = retry_after {
            return delay.min(Duration::from_millis(self.max_delay_ms));
        }
        let exponent = attempt.saturating_sub(1).min(20);
        let delay = self.base_delay_ms.saturating_mul(1u64 << exponent);
        Duration::from_millis(delay.min(self.max_delay_ms))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SyncOperation {
    Add { remote: RemoteTrack, position: u32 },
    Remove { remote_id: String },
    Reorder { remote_ids: Vec<String> },
    Rename { name: String },
}

impl SyncOperation {
    /// Stable key used by providers to make retries safe after a timeout.
    pub fn idempotency_key(&self, playlist_id: &str) -> String {
        let encoded = serde_json::to_vec(self).expect("sync operation is serializable");
        let digest = blake3::hash(&[playlist_id.as_bytes(), &encoded].concat());
        format!("aurora-sync-{}", digest.to_hex())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncPlan {
    pub operations: Vec<SyncOperation>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    Capability(&'static str),
    Conflict,
    MissingRemoteTrack(String),
}

/// Normalize metadata only for matching. Display values remain untouched.
pub fn track_match_key(title: &str, artist: Option<&str>, album: Option<&str>) -> String {
    fn normalize(value: &str) -> String {
        value
            .trim()
            .chars()
            .flat_map(char::to_lowercase)
            .filter(|c| !c.is_whitespace() && !matches!(c, '(' | ')' | '[' | ']'))
            .collect()
    }
    format!(
        "{}\u{1f}{}\u{1f}{}",
        normalize(title),
        artist.map(normalize).unwrap_or_default(),
        album.map(normalize).unwrap_or_default()
    )
}

/// Create a remote page from a complete provider-neutral snapshot. Real
/// adapters should use the provider's cursor, while this helper makes tests
/// and offline imports deterministic.
pub fn page<T: Clone>(items: &[T], cursor: Option<&str>, limit: usize) -> RemotePage<T> {
    let start = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
    let end = start.saturating_add(limit.max(1)).min(items.len());
    RemotePage {
        items: items[start.min(items.len())..end].to_vec(),
        next_cursor: (end < items.len()).then(|| end.to_string()),
        snapshot: Some(items.len().to_string()),
    }
}

/// Build an idempotent write plan that converges local and remote membership.
pub fn plan_sync(
    playlist_id: &str,
    local: &[LocalPlaylistTrack],
    remote: &RemotePlaylist,
    capabilities: SyncCapabilities,
    conflict: ConflictPolicy,
) -> Result<SyncPlan, SyncError> {
    if !capabilities.can_read {
        return Err(SyncError::Capability("read"));
    }
    let local_by_key: BTreeMap<_, _> = local
        .iter()
        .map(|track| {
            (
                track_match_key(
                    &track.title,
                    track.artist.as_deref(),
                    track.album.as_deref(),
                ),
                track,
            )
        })
        .collect();
    let remote_by_key: BTreeMap<_, _> = remote
        .tracks
        .iter()
        .map(|track| {
            (
                track_match_key(
                    &track.title,
                    track.artist.as_deref(),
                    track.album.as_deref(),
                ),
                track,
            )
        })
        .collect();
    let mut operations = Vec::new();
    let mut skipped = Vec::new();

    if conflict == ConflictPolicy::Fail
        && local_by_key
            .keys()
            .any(|key| remote_by_key.contains_key(key))
    {
        // A shared track is not itself a conflict; conflict is only ambiguous
        // membership. The policy is resolved below when one side differs.
    }
    if conflict == ConflictPolicy::RemoteWins {
        for track in local {
            let key = track_match_key(
                &track.title,
                track.artist.as_deref(),
                track.album.as_deref(),
            );
            if !remote_by_key.contains_key(&key) {
                skipped.push(format!("local track {} not present remotely", track.id.0));
            }
        }
    } else {
        for track in local {
            let key = track_match_key(
                &track.title,
                track.artist.as_deref(),
                track.album.as_deref(),
            );
            if !remote_by_key.contains_key(&key) {
                if !capabilities.can_add_tracks {
                    return Err(SyncError::Capability("add_tracks"));
                }
                operations.push(SyncOperation::Add {
                    remote: RemoteTrack {
                        id: track.id.0.to_string(),
                        title: track.title.clone(),
                        artist: track.artist.clone(),
                        album: track.album.clone(),
                        duration_ms: None,
                    },
                    position: track.position,
                });
            }
        }
    }
    if conflict != ConflictPolicy::LocalWins {
        for track in &remote.tracks {
            let key = track_match_key(
                &track.title,
                track.artist.as_deref(),
                track.album.as_deref(),
            );
            if !local_by_key.contains_key(&key) {
                if conflict == ConflictPolicy::Fail {
                    return Err(SyncError::Conflict);
                }
                skipped.push(format!("remote track {} retained", track.id));
            }
        }
    } else {
        for track in &remote.tracks {
            let key = track_match_key(
                &track.title,
                track.artist.as_deref(),
                track.album.as_deref(),
            );
            if !local_by_key.contains_key(&key) {
                if !capabilities.can_remove_tracks {
                    return Err(SyncError::Capability("remove_tracks"));
                }
                operations.push(SyncOperation::Remove {
                    remote_id: track.id.clone(),
                });
            }
        }
    }
    let desired_ids: Vec<_> = local
        .iter()
        .filter_map(|track| {
            let key = track_match_key(
                &track.title,
                track.artist.as_deref(),
                track.album.as_deref(),
            );
            remote_by_key.get(&key).map(|remote| remote.id.clone())
        })
        .collect();
    let remote_ids: Vec<_> = remote.tracks.iter().map(|track| track.id.clone()).collect();
    if conflict != ConflictPolicy::RemoteWins
        && desired_ids != remote_ids
        && !desired_ids.is_empty()
    {
        if !capabilities.can_reorder_tracks {
            return Err(SyncError::Capability("reorder_tracks"));
        }
        operations.push(SyncOperation::Reorder {
            remote_ids: desired_ids,
        });
    }
    let _ = playlist_id;
    Ok(SyncPlan {
        operations,
        skipped,
    })
}

/// Minimal in-memory executor used by adapters and tests to prove retries do
/// not duplicate writes. It records operation keys, not network state.
#[derive(Debug, Default)]
pub struct AppliedOperations {
    keys: BTreeSet<String>,
}

impl AppliedOperations {
    pub fn apply(&mut self, playlist_id: &str, operation: &SyncOperation) -> bool {
        self.keys.insert(operation.idempotency_key(playlist_id))
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn local(title: &str, position: u32) -> LocalPlaylistTrack {
        LocalPlaylistTrack {
            id: TrackId(Uuid::from_u128(position as u128 + 1)),
            title: title.into(),
            artist: Some("Artist".into()),
            album: None,
            position,
        }
    }
    fn remote(id: &str, title: &str) -> RemoteTrack {
        RemoteTrack {
            id: id.into(),
            title: title.into(),
            artist: Some("artist".into()),
            album: None,
            duration_ms: None,
        }
    }
    fn playlist(tracks: Vec<RemoteTrack>) -> RemotePlaylist {
        RemotePlaylist {
            provider: RemoteProvider::Other,
            id: "p".into(),
            name: "P".into(),
            tracks,
            revision: None,
        }
    }

    #[test]
    fn matching_ignores_case_spaces_and_brackets() {
        assert_eq!(
            track_match_key(" Song (Live) ", Some("ARTIST"), None),
            track_match_key("songlive", Some("artist"), None)
        );
    }

    #[test]
    fn pagination_is_cursor_based_and_bounded() {
        let values = vec![1, 2, 3, 4, 5];
        let first = page(&values, None, 2);
        assert_eq!(first.items, vec![1, 2]);
        let second = page(&values, first.next_cursor.as_deref(), 2);
        assert_eq!(second.items, vec![3, 4]);
        assert_eq!(page(&values, Some("99"), 2).items, Vec::<i32>::new());
    }

    #[test]
    fn local_wins_adds_and_removes_to_converge() {
        let plan = plan_sync(
            "p",
            &[local("New", 0)],
            &playlist(vec![remote("old", "Old")]),
            SyncCapabilities::FULL,
            ConflictPolicy::LocalWins,
        )
        .unwrap();
        assert!(
            plan.operations
                .iter()
                .any(|op| matches!(op, SyncOperation::Add { .. }))
        );
        assert!(
            plan.operations
                .iter()
                .any(|op| matches!(op, SyncOperation::Remove { remote_id } if remote_id == "old"))
        );
    }

    #[test]
    fn read_only_fails_before_writes() {
        let err = plan_sync(
            "p",
            &[local("New", 0)],
            &playlist(vec![]),
            SyncCapabilities::READ_ONLY,
            ConflictPolicy::LocalWins,
        )
        .unwrap_err();
        assert_eq!(err, SyncError::Capability("add_tracks"));
    }

    #[test]
    fn operation_application_is_idempotent() {
        let op = SyncOperation::Remove {
            remote_id: "x".into(),
        };
        let mut applied = AppliedOperations::default();
        assert!(applied.apply("p", &op));
        assert!(!applied.apply("p", &op));
        assert_eq!(applied.len(), 1);
    }

    #[test]
    fn retry_after_wins_and_backoff_is_capped() {
        let policy = RetryPolicy {
            max_attempts: 4,
            base_delay_ms: 100,
            max_delay_ms: 250,
        };
        assert_eq!(policy.delay_for_retry(3, None), Duration::from_millis(250));
        assert_eq!(
            policy.delay_for_retry(1, Some(Duration::from_secs(2))),
            Duration::from_millis(250)
        );
    }
}
