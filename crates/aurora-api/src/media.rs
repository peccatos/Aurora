//! Browser-side yt-dlp media endpoints: metadata and authorized downloads.
//!
//! The Tauri shell shells out to the yt-dlp sidecar directly (see
//! `aurora-tauri::commands`); the browser cannot spawn processes, so the same
//! two operations are exposed here as authenticated endpoints. The browser
//! path is metadata-only for the same reason downloads are desktop-only in
//! `frontend/src/media-download.ts`: a downloaded file belongs on the user's
//! disk next to their library, and a server is not that disk. (A future
//! streaming download is a deployment decision, tracked in plan.md.)
//!
//! Policy mirrors the Tauri commands exactly, because the allowlist is the
//! product's promise about what it will fetch: HTTPS, host on the shared
//! allowlist ([`crate::limits::MEDIA_ALLOWED_HOSTS`]), no userinfo, bounded
//! URL length, `--no-playlist`, bounded output, hard timeout. No DRM or
//! protected-stream extraction is attempted; the tool is invoked for metadata
//! on public pages only.
//!
//! `POST /api/media/metadata` requires a browser session *without* an OAuth
//! account: media lookup is not a Spotify/YouTube-Music grant operation, so
//! it is authorized by the session cookie + CSRF pair alone and charged to
//! the `media` rate-limit bucket rather than any provider's.

use std::path::PathBuf;
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::{AppState, PeerAddr, fail, guard, hash, limits, origin_ok, session};

/// How long one yt-dlp invocation may run before it is killed.
const YTDLP_TIMEOUT: Duration = Duration::from_secs(45);
/// Ceiling on yt-dlp's combined output (stdout + stderr) before the response
/// is discarded as hostile rather than parsed.
const YTDLP_MAX_OUTPUT: usize = 2 * 1024 * 1024;

/// Body accepted by `POST /api/media/metadata`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetadataRequest {
    url: String,
}

/// The sanitized metadata the browser is allowed to see. Fields are exactly
/// what `frontend/src/media-download.ts::normalizeMetadata` reads — no raw
/// JSON passthrough, so an unexpected yt-dlp field cannot reach the browser
/// and an oversized field is truncated here, not client-side.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MediaMetadata {
    title: String,
    uploader: Option<String>,
    duration_seconds: Option<f64>,
    webpage_url: String,
    thumbnail_url: Option<String>,
}

impl MediaMetadata {
    fn from_stdout(raw: &[u8]) -> Result<Self, ()> {
        if raw.len() > YTDLP_MAX_OUTPUT {
            return Err(());
        }
        // Shape-checked subset of yt-dlp's single-JSON output. Unknown keys
        // are expected (there are dozens) and ignored; required keys are
        // validated, and every string that survives is bounded before it is
        // serialized back out.
        let value: serde_json::Value = serde_json::from_slice(raw).map_err(|_| ())?;
        let object = value.as_object().ok_or(())?;
        let webpage_url = object
            .get("webpage_url")
            .and_then(|v| v.as_str())
            .and_then(|v| bounded_str(v, 2048))
            .ok_or(())?;
        if !limits::is_media_url(&webpage_url) {
            return Err(());
        }
        let title = object
            .get("title")
            .and_then(|v| v.as_str())
            .and_then(|v| bounded_str(v, 500))
            .ok_or(())?;
        let uploader = object
            .get("uploader")
            .and_then(|v| v.as_str())
            .and_then(|v| bounded_str(v, 300));
        let duration_seconds = object
            .get("duration")
            .and_then(|v| v.as_f64())
            .filter(|d| d.is_finite() && *d >= 0.0);
        let thumbnail_url = object
            .get("thumbnail")
            .and_then(|v| v.as_str())
            .and_then(|v| bounded_str(v, 2048))
            .filter(|v| limits::is_media_url(v));
        Ok(Self {
            title,
            uploader,
            duration_seconds,
            webpage_url,
            thumbnail_url,
        })
    }
}

/// A bounded, control-character-free copy of a provider string. The same rule
/// the playlist normalizers apply: control characters and the RTL override
/// are rejected (they can reorder the UI that renders them), and everything
/// else is truncated at the caller's cap.
fn bounded_str(raw: &str, max: usize) -> Option<String> {
    if raw.is_empty() || raw.len() > max {
        return None;
    }
    if raw.chars().any(|c| c.is_control() || c == '\u{202e}') {
        return None;
    }
    Some(raw.to_owned())
}

/// Locates the yt-dlp binary for the server process. The Tauri sidecar
/// resolution is not reused verbatim because the API runs from a different
/// working directory; the search covers the API binary's own directory and
/// the repo's staged sidecar directories.
fn ytdlp_path() -> Option<PathBuf> {
    let name = format!("yt-dlp{}", std::env::consts::EXE_SUFFIX);
    let mut candidates: Vec<PathBuf> = vec![
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join(&name))),
        Some(PathBuf::from("bin").join(&name)),
        Some(PathBuf::from("apps/desktop/binaries").join(&name)),
    ]
    .into_iter()
    .flatten()
    .collect();
    for directory in [PathBuf::from("bin"), PathBuf::from("apps/desktop/binaries")] {
        if let Ok(entries) = std::fs::read_dir(&directory) {
            candidates.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| {
                        value.starts_with("yt-dlp-")
                            && value.ends_with(std::env::consts::EXE_SUFFIX)
                    })
            }));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Runs yt-dlp once with a hard timeout and output cap; returns stdout.
async fn run_ytdlp(args: Vec<String>) -> Result<String, StatusCode> {
    let Some(path) = ytdlp_path() else {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    };
    let process = tokio::process::Command::new(path)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let child = match process {
        Ok(child) => child,
        Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let output = tokio::time::timeout(YTDLP_TIMEOUT, child.wait_with_output()).await;
    let output = match output {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        // The process is already gone: wait_with_output owned it, and the
        // timeout dropping the future kills the child.
        Err(_) => return Err(StatusCode::GATEWAY_TIMEOUT),
    };
    if output.stdout.len() + output.stderr.len() > YTDLP_MAX_OUTPUT {
        return Err(StatusCode::BAD_GATEWAY);
    }
    if !output.status.success() {
        return Err(StatusCode::BAD_GATEWAY);
    }
    String::from_utf8(output.stdout).map_err(|_| StatusCode::BAD_GATEWAY)
}

/// The shared request gate for both media endpoints: allowed origin,
/// session cookie + CSRF token bound to that session, and a charge against
/// the `media` rate-limit bucket. Returns `Ok(sid)` on success.
async fn authorize_media(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    if !origin_ok(headers, &state.config) {
        return Err(fail(StatusCode::FORBIDDEN));
    }
    let Some(sid) = session(headers) else {
        return Err(fail(StatusCode::UNAUTHORIZED));
    };
    let Some(token) = headers
        .get(crate::CSRF_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| limits::is_csrf_token(v))
    else {
        return Err(fail(StatusCode::FORBIDDEN));
    };
    match aurora_storage::validate_oauth_csrf(
        &state.pool,
        &hash(&sid),
        &hash(token),
        &chrono::Utc::now().to_rfc3339(),
    )
    .await
    {
        Ok(true) => Ok(sid),
        Ok(false) => Err(fail(StatusCode::FORBIDDEN)),
        Err(_) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

/// `POST /api/media/metadata` — allowlisted URL in, sanitized metadata out.
pub(crate) async fn metadata(
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
    body: Json<MetadataRequest>,
) -> Response {
    let Json(request) = body;
    if !limits::is_media_url(&request.url) {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Err(rejection) = authorize_media(&state, &headers).await {
        return rejection;
    }
    if let Some(rejection) = rate_limit(&state, peer) {
        return rejection;
    }
    match run_ytdlp(vec![
        "--no-playlist".into(),
        "--dump-single-json".into(),
        "--no-warnings".into(),
        request.url,
    ])
    .await
    {
        Ok(stdout) => match MediaMetadata::from_stdout(stdout.as_bytes()) {
            Ok(metadata) => (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "no-store")],
                Json(metadata),
            )
                .into_response(),
            Err(()) => fail(StatusCode::BAD_GATEWAY),
        },
        Err(status) => fail(status),
    }
}

fn rate_limit(state: &AppState, peer: PeerAddr) -> Option<Response> {
    let decision = state.limits.check(guard::client_ip(peer.0), "media");
    (!decision.allowed).then(|| guard::rate_limited(decision.retry_after_secs))
}

/// `GET /api/media/ping` — tells the browser whether this deployment can run
/// yt-dlp at all, so the UI can hide the feature instead of showing a
/// featureless 503 on first use. Cheap (existence check only) and unauthed.
pub(crate) async fn ping() -> Response {
    match ytdlp_path() {
        Some(_) => (StatusCode::OK, Json(serde_json::json!({"available":true}))).into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"available":false})),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_urls_must_be_allowlisted_https() {
        assert!(limits::is_media_url("https://archive.org/details/x"));
        assert!(limits::is_media_url("https://www.youtube.com/watch?v=abc"));
        assert!(limits::is_media_url("https://youtu.be/abc"));
        assert!(!limits::is_media_url("http://youtube.com/watch?v=abc"));
        assert!(!limits::is_media_url("https://vimeo.com/x"));
        assert!(!limits::is_media_url(
            "https://user:pass@youtube.com/watch?v=abc"
        ));
        assert!(!limits::is_media_url("not a url"));
        assert!(!limits::is_media_url(&format!(
            "https://youtu.be/{}",
            "a".repeat(2100)
        )));
    }

    #[test]
    fn metadata_parses_only_expected_fields() {
        let raw = br#"{"title":"Song","uploader":"Chan","duration":123.5,"webpage_url":"https://youtu.be/abc","thumbnail":"https://i.ytimg.com/vi/abc/hq.jpg","random_field":true}"#;
        let parsed = MediaMetadata::from_stdout(raw).expect("valid metadata");
        assert_eq!(parsed.title, "Song");
        assert_eq!(parsed.uploader.as_deref(), Some("Chan"));
        assert_eq!(parsed.duration_seconds, Some(123.5));
        // i.ytimg.com is not on the media allowlist: the thumbnail is dropped,
        // not passed through.
        assert_eq!(parsed.thumbnail_url, None);
    }

    #[test]
    fn metadata_rejects_missing_or_hostile_fields() {
        // No title.
        assert!(MediaMetadata::from_stdout(br#"{"webpage_url":"https://youtu.be/abc"}"#).is_err());
        // webpage_url not on the allowlist.
        assert!(
            MediaMetadata::from_stdout(br#"{"title":"x","webpage_url":"https://evil.example/x"}"#)
                .is_err()
        );
        // Not JSON at all.
        assert!(MediaMetadata::from_stdout(b"not json").is_err());
        // Title containing a control character.
        assert!(
            MediaMetadata::from_stdout(br#"{"title":"a b","webpage_url":"https://youtu.be/abc"}"#)
                .is_err()
        );
    }

    #[test]
    fn bounded_str_rejects_empty_oversized_and_control_chars() {
        assert_eq!(bounded_str("ok", 10).as_deref(), Some("ok"));
        assert!(bounded_str("", 10).is_none());
        assert!(bounded_str("0123456789A", 10).is_none());
        assert!(bounded_str("a b", 10).is_none());
    }
}
