//! Playlist import/export: file formats in and out of the library.
//!
//! Export is M3U8 (universal: paths, UTF-8, every player reads it).
//! Import accepts:
//! - **M3U / M3U8** — `#EXTM3U` headers, absolute or playlist-relative
//!   paths. Local files resolve against the playlist's own directory.
//! - **PLS** — INI-style `[playlist]` sections (Winamp-era, still common).
//! - **Spotify JSON** — account export: arrays of `{title, artist, album}`.
//! - **CSV** (Last.fm / Apple Music / generic) — heuristics over common
//!   column names (title/track, artist, album).
//!
//! Imported service playlists (Spotify/CSV) have no paths: entries are
//! matched against the LOCAL library by (title, artist). An honest
//! import report says how many matched, not just "done": `matched` +
//! `unmatched` counts come back to the UI verbatim. Unmatched entries
//! are never fabricated as tracks — they are listed by name so the user
//! can see what is missing.

use std::path::Path;

/// One imported playlist entry, resolved as far as the format allows.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportedEntry {
    /// A concrete audio file on this machine (from M3U/PLS paths).
    Path(String),
    /// A title/artist pair to match against the local library
    /// (Spotify/CSV imports). `artist` may be None (CSV without artist).
    Meta {
        title: String,
        artist: Option<String>,
    },
}

/// A parsed playlist file.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedPlaylist {
    /// Playlist name (from the format where derivable, else file stem).
    pub name: String,
    /// Entries in file order (order is preserved on import).
    pub entries: Vec<ImportedEntry>,
}

/// Parse a playlist file (format detected from extension, then content).
pub fn parse_playlist(path: &Path, raw: &str) -> Result<ImportedPlaylist, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let name = file_stem_string(path);

    // Content sniff: structure first, then extension, then heuristics.
    let trimmed = raw.trim_start();
    if trimmed.starts_with("[playlist") {
        return parse_pls(raw, &name);
    }
    if trimmed.starts_with("#EXTM3U") || ext == "m3u" || ext == "m3u8" {
        return parse_m3u(raw, &name, path);
    }
    if ext == "csv" || ext == "txt" || (ext != "json" && raw.contains(',')) {
        return parse_csv_playlist(raw, &name);
    }
    if trimmed.starts_with('{') || trimmed.starts_with('[') || ext == "json" {
        return parse_json_playlist(raw, &name);
    }
    // Last resort: bare path list (no extension, one path per line).
    parse_m3u(raw, &name, path)
}

// ---------------------------------------------------------------------------
// M3U / M3U8
// ---------------------------------------------------------------------------

fn parse_m3u(raw: &str, name: &str, playlist_path: &Path) -> Result<ImportedPlaylist, String> {
    let base_dir = playlist_path.parent().map(Path::to_path_buf);
    let mut entries = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue; // #EXTM3U / #EXTINF metadata lines
        }
        let path = if Path::new(line).is_absolute() {
            line.to_string()
        } else {
            // Playlist-relative path: resolve against its directory.
            match &base_dir {
                Some(base) => {
                    let joined = base.join(line);
                    let s = joined.to_string_lossy().replace('/', "\\");
                    if s.is_empty() { line.to_string() } else { s }
                }
                None => line.to_string(),
            }
        };
        entries.push(ImportedEntry::Path(path));
    }
    if entries.is_empty() {
        return Err("M3U: no track entries found".into());
    }
    Ok(ImportedPlaylist {
        name: name.into(),
        entries,
    })
}

// ---------------------------------------------------------------------------
// PLS
// ---------------------------------------------------------------------------

fn parse_pls(raw: &str, name: &str) -> Result<ImportedPlaylist, String> {
    // Entries arrive as File1=…, File2=…: parse and sort by their index
    // so playlist order is the file's order, not the line order.
    let mut file_entries: Vec<(u64, String)> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        // "File" (index-less dialect) or "File1", "File2", …
        if key.eq_ignore_ascii_case("file") {
            file_entries.push((0, value.trim().to_string()));
        } else if let Some(num) = key
            .strip_prefix("File")
            .or(key.strip_prefix("file"))
            .filter(|n| n.trim().parse::<u64>().is_ok())
        {
            let idx = num.trim().parse::<u64>().unwrap_or(0);
            file_entries.push((idx, value.trim().to_string()));
        }
        // Title/length lines are ignored: paths carry the content.
    }
    file_entries.sort_by_key(|(idx, _)| *idx);
    let entries = file_entries
        .into_iter()
        .map(|(_, p)| ImportedEntry::Path(p))
        .collect::<Vec<_>>();
    if entries.is_empty() {
        return Err("PLS: no File entries found".into());
    }
    Ok(ImportedPlaylist {
        name: name.into(),
        entries,
    })
}

// ---------------------------------------------------------------------------
// Spotify-style JSON
// ---------------------------------------------------------------------------

/// Spotify account export shapes seen in the wild:
/// - `[{"name": "...", "tracks": [...]}]` (playlists array) — we import
///   the first playlist.
/// - `[{"title": "...", "artist": "..."}]` (a flat track list) — e.g.
///   extended streaming history / liked-songs exports.
/// - `{"tracks": [...]}` — single playlist object.
fn parse_json_playlist(raw: &str, name: &str) -> Result<ImportedPlaylist, String> {
    let value: serde_json::Value = serde_json::from_str(raw).map_err(|e| format!("JSON: {e}"))?;

    let (pl_name, track_values) = match &value {
        serde_json::Value::Array(items) => {
            // Playlists array (Spotify full export) or flat track list.
            if items
                .iter()
                .all(|i| i.get("tracks").is_some() && i.get("name").is_some())
            {
                // Array of playlists: take the first (name carries).
                let first = items.first().ok_or("JSON: empty playlist array")?;
                let n = first
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(name);
                let tracks = first
                    .get("tracks")
                    .and_then(|t| t.as_array())
                    .cloned()
                    .unwrap_or_default();
                (n.to_string(), tracks)
            } else {
                (name.to_string(), items.clone())
            }
        }
        serde_json::Value::Object(map) => {
            let n = map
                .get("name")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(name);
            let tracks = map
                .get("tracks")
                .or_else(|| map.get("items"))
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            (n.to_string(), tracks)
        }
        _ => return Err("JSON: unsupported structure".into()),
    };

    let mut entries = Vec::new();
    for tv in &track_values {
        if let Some(e) = entry_from_json_track(tv) {
            entries.push(e);
        }
    }
    if entries.is_empty() {
        return Err("JSON: no tracks with a title found".into());
    }
    Ok(ImportedPlaylist {
        name: pl_name,
        entries,
    })
}

/// Extract a Meta entry from one JSON track object (several dialects:
/// flat `{title, artist}`, Spotify `{track: {name, artists}}`).
fn entry_from_json_track(tv: &serde_json::Value) -> Option<ImportedEntry> {
    let (title, artist) = match tv {
        serde_json::Value::Object(_) => {
            if let Some(track) = tv.get("track").and_then(|t| t.as_object()) {
                // Wrapped (Spotify playlist item).
                let title = track.get("name").and_then(serde_json::Value::as_str)?;
                let artist = track
                    .get("artists")
                    .and_then(|a| a.as_array())
                    .and_then(|a| a.first())
                    .and_then(|a| a.get("name"))
                    .and_then(serde_json::Value::as_str);
                (title.to_string(), artist.map(str::to_string))
            } else {
                let title = tv
                    .get("title")
                    .or_else(|| tv.get("name"))
                    .and_then(serde_json::Value::as_str)?;
                let artist = tv
                    .get("artist")
                    .or_else(|| tv.get("artists"))
                    .and_then(serde_json::Value::as_str);
                (title.to_string(), artist.map(str::to_string))
            }
        }
        _ => return None,
    };
    Some(ImportedEntry::Meta { title, artist })
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// CSV import (Last.fm, Apple Music, generic exports).
///
/// Column names are matched case-insensitively against a small set of
/// well-known spellings ("Track", "Title", "Name", "Song", "Artist",
/// "Artist Name", "Album", "Album Artist"). Quoted CSV values with
/// embedded commas are handled by a tiny state machine (no csv crate —
/// the format subset music services emit is one header row + quoted
/// strings).
fn parse_csv_playlist(raw: &str, name: &str) -> Result<ImportedPlaylist, String> {
    let lines = split_csv_lines(raw);
    let header = lines
        .first()
        .ok_or("CSV: empty file")?
        .iter()
        .map(|h| h.trim().to_ascii_lowercase())
        .collect::<Vec<_>>();

    let title_col = header
        .iter()
        .position(|h| ["track", "title", "name", "song", "track title"].contains(&h.as_str()));
    let artist_col = header
        .iter()
        .position(|h| ["artist", "artist name", "artist_names"].contains(&h.as_str()));

    let title_col = title_col.ok_or("CSV: no title/track column found")?;

    let mut entries = Vec::new();
    for row in lines.iter().skip(1) {
        let title = row.get(title_col).map(|s| s.trim()).unwrap_or("");
        if title.is_empty() {
            continue;
        }
        let artist = artist_col
            .and_then(|c| row.get(c))
            .map(|s| s.trim().to_string());
        entries.push(ImportedEntry::Meta {
            title: title.to_string(),
            artist: artist.filter(|a| !a.is_empty()),
        });
    }
    if entries.is_empty() {
        return Err("CSV: no track rows found".into());
    }
    Ok(ImportedPlaylist {
        name: name.into(),
        entries,
    })
}

/// Split CSV into rows of cells (handles quoted values with commas and
/// CRLF line ends).
fn split_csv_lines(raw: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut in_quotes = false;
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quotes => {
                if chars.peek() == Some(&'"') {
                    cell.push('"');
                    chars.next();
                } else {
                    in_quotes = false;
                }
            }
            '"' => in_quotes = true,
            ',' if !in_quotes => {
                row.push(std::mem::take(&mut cell));
            }
            '\r' => {}
            '\n' if !in_quotes => {
                if !cell.is_empty() || !row.is_empty() {
                    row.push(std::mem::take(&mut cell));
                    rows.push(std::mem::take(&mut row));
                }
            }
            c => cell.push(c),
        }
    }
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        rows.push(row);
    }
    rows
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// Render a playlist as M3U8 (absolute paths, UTF-8, EXTINF lines with
/// title — artist).
pub fn to_m3u8(name: &str, tracks: &[ExportTrack]) -> String {
    let mut out = String::from("#EXTM3U\n");
    for t in tracks {
        let line = match (&t.artist, &t.title) {
            (Some(a), Some(ti)) => format!("{a} — {ti}"),
            (None, Some(ti)) => ti.clone(),
            (Some(a), None) => a.clone(),
            (None, None) => String::new(),
        };
        let duration = t
            .duration_s
            .map(|d| format!("{}", d as i64))
            .unwrap_or("-1".into());
        out.push_str(&format!("#EXTINF:{duration},{line}\n"));
        out.push_str(&t.path);
        out.push('\n');
    }
    let _ = name; // M3U8 has no playlist-name field; the file name carries it.
    out
}

/// One track to export.
#[derive(Debug, Clone)]
pub struct ExportTrack {
    pub path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub duration_s: Option<f64>,
}

fn file_stem_string(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Imported playlist".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m3u_absolute_paths() {
        let raw = "#EXTM3U\n#EXTINF:213,Artist — Title\nC:\\music\\a.flac\nC:\\music\\b.mp3\n";
        let pl = parse_playlist(Path::new("list.m3u8"), raw).unwrap();
        assert_eq!(pl.name, "list");
        assert_eq!(
            pl.entries,
            vec![
                ImportedEntry::Path("C:\\music\\a.flac".into()),
                ImportedEntry::Path("C:\\music\\b.mp3".into()),
            ]
        );
    }

    #[test]
    fn m3u_relative_paths_resolve_against_playlist_dir() {
        let raw = "#EXTM3U\n01.flac\nsub/02.mp3\n";
        let pl = parse_playlist(Path::new("C:/music/list.m3u8"), raw).unwrap();
        assert_eq!(
            pl.entries,
            vec![
                ImportedEntry::Path("C:\\music\\01.flac".into()),
                ImportedEntry::Path("C:\\music\\sub\\02.mp3".into()),
            ]
        );
    }

    #[test]
    fn pls_files() {
        let raw =
            "[playlist]\nNumberOfEntries=2\nFile1=C:\\x\\a.mp3\nTitle1=A\nFile2=C:\\x\\b.mp3\n";
        let pl = parse_playlist(Path::new("old.pls"), raw).unwrap();
        assert_eq!(
            pl.entries,
            vec![
                ImportedEntry::Path("C:\\x\\a.mp3".into()),
                ImportedEntry::Path("C:\\x\\b.mp3".into()),
            ]
        );
    }

    #[test]
    fn spotify_playlist_array() {
        let raw = r#"[{"name":"Road trip","tracks":[{"track":{"name":"Song A","artists":[{"name":"Artist X"}]}},{"title":"Song B","artist":"Artist Y"}]}]"#;
        let pl = parse_playlist(Path::new("spotify.json"), raw).unwrap();
        assert_eq!(pl.name, "Road trip");
        assert_eq!(
            pl.entries,
            vec![
                ImportedEntry::Meta {
                    title: "Song A".into(),
                    artist: Some("Artist X".into())
                },
                ImportedEntry::Meta {
                    title: "Song B".into(),
                    artist: Some("Artist Y".into())
                },
            ]
        );
    }

    #[test]
    fn flat_json_track_list() {
        let raw = r#"[{"title":"Alpha","artist":"Beta"},{"title":"Gamma"}]"#;
        let pl = parse_playlist(Path::new("liked.json"), raw).unwrap();
        assert_eq!(
            pl.entries,
            vec![
                ImportedEntry::Meta {
                    title: "Alpha".into(),
                    artist: Some("Beta".into())
                },
                ImportedEntry::Meta {
                    title: "Gamma".into(),
                    artist: None
                },
            ]
        );
    }

    #[test]
    fn lastfm_csv() {
        let raw = "Artist,Track,Album\n\"DJ, Two\",Song One,Album\nBeta,Song Two,Album\n";
        let pl = parse_playlist(Path::new("history.csv"), raw).unwrap();
        assert_eq!(
            pl.entries,
            vec![
                ImportedEntry::Meta {
                    title: "Song One".into(),
                    artist: Some("DJ, Two".into())
                },
                ImportedEntry::Meta {
                    title: "Song Two".into(),
                    artist: Some("Beta".into())
                },
            ]
        );
    }

    #[test]
    fn apple_music_csv_dialect() {
        let raw = "Title,Artist,Album\nSong A,Artist B,Album C\n";
        let pl = parse_playlist(Path::new("apple.csv"), raw).unwrap();
        assert_eq!(
            pl.entries,
            vec![ImportedEntry::Meta {
                title: "Song A".into(),
                artist: Some("Artist B".into())
            }]
        );
    }

    #[test]
    fn export_roundtrip_through_m3u() {
        let tracks = vec![ExportTrack {
            path: "C:\\music\\a.flac".into(),
            title: Some("Title".into()),
            artist: Some("Artist".into()),
            duration_s: Some(213.4),
        }];
        let m3u = to_m3u8("list", &tracks);
        let pl = parse_playlist(Path::new("C:/music/list.m3u8"), &m3u).unwrap();
        assert_eq!(
            pl.entries,
            vec![ImportedEntry::Path("C:\\music\\a.flac".into())]
        );
    }

    #[test]
    fn garbage_fails_cleanly() {
        let err = parse_playlist(Path::new("x.json"), "not json at all").is_err();
        assert!(err);
    }
}
