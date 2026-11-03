//! In-memory file registry for wasm builds (wasm shell registers File
//! bytes; "web://<id>" paths resolve here). Native builds never see
//! these paths and this module does not exist for them.

use std::collections::HashMap;
use std::sync::Mutex;

static FILES: Mutex<Option<HashMap<String, (Vec<u8>, String)>>> = Mutex::new(None);

/// Register file bytes under an id, with the file extension (no dot)
/// for the probe hint.
pub fn register(id: &str, bytes: Vec<u8>, extension: &str) {
    FILES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get_or_insert_with(HashMap::new)
        .insert(id.to_string(), (bytes, extension.to_string()));
}

/// Take the file out of the registry for playback (the source consumes
/// it; a second play re-registers from JS — the shell owns persistence).
pub fn take(id: &str) -> Option<(Vec<u8>, String)> {
    FILES
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .as_mut()
        .and_then(|m| m.remove(id))
}
