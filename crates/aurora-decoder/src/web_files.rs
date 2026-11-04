//! In-memory file registry for the wasm shell: "web://<id>" paths
//! resolve to bytes the JS side registered (the browser has no
//! filesystem).
//!
//! Files are CLONED on take, not consumed: a track must play again
//! without the JS shell re-registering it (the queue replays tracks,
//! decks jump to cues, loops re-seek — all re-open the source).
//!
//! Native builds see the same API: register is a no-op (native paths
//! are real files) and take never succeeds (a "web://" path cannot
//! occur on a machine with a disk). This keeps the module resolvable
//! on BOTH targets so the code compiles identically everywhere.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Shared, immutable bytes: cheap to clone (refcount), the decoder
/// reads through a fresh Cursor per open.
type Entry = Arc<(Vec<u8>, String)>;

/// The registry itself. Wasm: the live store. Native: always empty —
/// registering is pointless without browser File objects, and the
/// compiler folds the whole thing away as dead code.
#[cfg(target_family = "wasm")]
static FILES: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);

#[cfg(target_family = "wasm")]
fn files() -> &'static Mutex<Option<HashMap<String, Entry>>> {
    &FILES
}

#[cfg(not(target_family = "wasm"))]
fn files() -> &'static Mutex<Option<HashMap<String, Entry>>> {
    // Native: a single shared empty map — take can never succeed, and
    // there is nothing to register (paths are real files here).
    static EMPTY: Mutex<Option<HashMap<String, Entry>>> = Mutex::new(None);
    &EMPTY
}

/// Register file bytes under an id, with the file extension (no dot)
/// for the probe hint. Re-registering replaces (last write wins).
/// Native: a no-op (the map stays empty).
pub fn register(id: &str, bytes: Vec<u8>, extension: &str) {
    #[cfg(target_family = "wasm")]
    {
        files()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_or_insert_with(HashMap::new)
            .insert(id.to_string(), Arc::new((bytes, extension.to_string())));
    }
    #[cfg(not(target_family = "wasm"))]
    {
        let _ = (id, bytes, extension);
    }
}

/// Clone the file bytes for playback. Multiple opens of the same id are
/// fine (queue replay, deck reload, A/B loop re-seek).
/// Native: always None (the registry is never populated).
pub fn take(id: &str) -> Option<(Vec<u8>, String)> {
    files()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_ref()
        .and_then(|m| m.get(id))
        .map(|entry| (entry.0.clone(), entry.1.clone()))
}
