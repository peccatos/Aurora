//! Aurora kernel: the music player, shell-free (ADR 0005).
//!
//! Everything a shell (Tauri desktop, web, third-party player) does to
//! music goes through `PlayerKernel`. The kernel composes the audio
//! crates (decoder, DSP, VST, audio engine, metadata, storage, library)
//! and owns no UI, no Tauri, no HTTP. Honesty contracts (ADR 0002)
//! travel with it: a shell can display verdicts, never fake them.

pub mod artwork;
pub mod decks;
pub mod dtos;
pub mod error;
pub mod events;
pub mod kernel;
pub mod playlist_io;
pub mod queue;
pub mod watch;

pub use decks::Deck;
pub use dtos::*;
pub use error::KernelError;
pub use events::{EventSubscribers, KernelEvent, Subscription};
pub use kernel::PlayerKernel;
pub use queue::{QueueTrack, TrackQueue};
