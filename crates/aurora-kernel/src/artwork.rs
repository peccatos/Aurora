//! Track artwork: embedded cover art from tags, as data URLs, cached.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use crate::error::KernelError;

/// In-memory artwork cache: track path -> data URL. Bounded; one entry
/// per track actually displayed.
fn artwork_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get the embedded cover art of a track as a data URL (for <img src>).
/// `Ok(None)` = no artwork in tags (the UI shows its placeholder).
pub fn track_artwork(path: &str) -> Result<Option<String>, KernelError> {
    // Cache hit (the library re-asks for the now-playing art on every
    // status poll's title update).
    if let Some(hit) = artwork_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(path).cloned())
    {
        return Ok(Some(hit));
    }
    let bytes = aurora_metadata::read_cover_art(std::path::Path::new(path))?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let mime = sniff_image_mime(&bytes);
    let data = base64_encode(&bytes);
    let url = format!("data:{mime};base64,{data}");
    if let Ok(mut cache) = artwork_cache().lock() {
        // Bound the cache: drop the oldest entries past 64.
        if cache.len() >= 64 {
            cache.clear();
        }
        cache.insert(path.to_string(), url.clone());
    }
    Ok(Some(url))
}

fn sniff_image_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.starts_with(b"BM") {
        "image/bmp"
    } else if bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "application/octet-stream"
    }
}

/// Minimal base64 (std lib has none; keep it local and simple).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}
