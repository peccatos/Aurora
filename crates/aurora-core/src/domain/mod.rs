//! Domain types and contracts.
//!
//! Split by bounded area: audio representation, library entities and
//! playback state. Everything here is backend-agnostic by construction
//! (see ARCHITECTURE.md, dependency direction).

pub mod audio;
pub mod library;
pub mod playback;
pub mod sync;

pub use audio::{
    AudioFormat, AudioTransformation, ChannelLayout, OutputDiagnostics, OutputIntegrity, PcmFormat,
    SampleFormat,
};
pub use library::{Album, Artist, Playlist, Track, TrackId};
pub use playback::{PlaybackMode, PlaybackState};
pub use sync::{
    AppliedOperations, ConflictPolicy, LocalPlaylistTrack, RemotePage, RemotePlaylist,
    RemoteProvider, RemoteTrack, RetryPolicy, SyncCapabilities, SyncError, SyncOperation, SyncPlan,
    page, plan_sync, track_match_key,
};
