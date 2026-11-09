//! Read-only playlist detail fetching for the browser OAuth adapter.
//!
//! Only allowlisted HTTPS provider endpoints are reachable from here, requests
//! are bounded in page count and response size, and every payload is parsed
//! strictly with `deny_unknown_fields` so that malformed or unexpected shapes
//! are rejected instead of being silently normalized.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use url::Url;

use crate::{
    AppState, PlaylistPath, ReqwestTokenTransport, fail, hash, limits, origin_ok,
    provider_descriptor, provider_error, provider_settings,
};

/// Upper bound on how many provider pages a single detail request may walk.
const MAX_PAGES: usize = 100;
/// Upper bound on how many tracks a detail response may contain.
const MAX_TRACKS: usize = 10_000;
/// Upper bound on the size of a single provider response body.
const MAX_BODY_BYTES: usize = limits::MAX_PROVIDER_BODY_BYTES;
/// Per-request provider timeout; the shared client is not reused for details.
const PROVIDER_TIMEOUT: Duration = Duration::from_secs(15);
const SPOTIFY_PLAYLISTS_ENDPOINT: &str = "https://api.spotify.com/v1/playlists";
const YOUTUBE_ITEMS_ENDPOINT: &str = "https://www.googleapis.com/youtube/v3/playlistItems";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaylistDetail {
    pub name: String,
    pub revision: Option<String>,
    pub tracks: Vec<PlaylistDetailTrack>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaylistDetailTrack {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotifyPage {
    items: Vec<SpotifyItem>,
    next: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotifyItem {
    track: Option<SpotifyTrack>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotifyTrack {
    id: Option<String>,
    name: Option<String>,
    artists: Option<Vec<SpotifyArtist>>,
    album: Option<SpotifyAlbum>,
    duration_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotifyArtist {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SpotifyAlbum {
    name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct YoutubeItemsPage {
    #[serde(default)]
    items: Vec<YoutubeItem>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct YoutubeItem {
    snippet: Option<YoutubeSnippet>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct YoutubeSnippet {
    title: Option<String>,
    #[serde(default)]
    video_owner_channel_title: Option<String>,
    resource_id: Option<YoutubeResourceId>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct YoutubeResourceId {
    video_id: Option<String>,
}

/// Rejects payloads that exceed the bounded response size.
fn bounded_parse<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, ()> {
    if body.len() > MAX_BODY_BYTES {
        return Err(());
    }
    serde_json::from_slice(body).map_err(|_| ())
}

/// A next-page link and page token must stay on the allowlisted provider host.
fn allowlisted_next(candidate: &str, host: &str) -> Option<Url> {
    let url = Url::parse(candidate).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some(host)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    Some(url)
}

fn normalized_id(raw: &str) -> Option<String> {
    // Track and video ids are base64url-ish, never path segments or separators.
    limits::is_base64url_token(raw, 1..=256).then(|| raw.to_owned())
}

fn normalized_text(raw: &str, limit: usize) -> Option<String> {
    let value = raw.trim();
    if value.is_empty() || value.chars().count() > limit {
        return None;
    }
    // Control bytes and bidi overrides would let a title forge terminal output
    // or reorder a filename once a caller writes it out, so they are rejected
    // rather than stripped.
    if value.chars().any(|c| c.is_control() || c == '\u{202e}') {
        return None;
    }
    Some(value.to_owned())
}

fn normalize_spotify(body: &[u8]) -> Result<Vec<PlaylistDetailTrack>, ()> {
    let page: SpotifyPage = bounded_parse(body)?;
    let mut tracks = Vec::new();
    for item in page.items {
        let Some(track) = item.track else {
            // Unplayable entries (local files, removed episodes) are skipped.
            continue;
        };
        let (Some(id), Some(title)) = (track.id.as_deref(), track.name.as_deref()) else {
            continue;
        };
        let (Some(id), Some(title)) = (normalized_id(id), normalized_text(title, 512)) else {
            return Err(());
        };
        let mut artists = Vec::new();
        for artist in track.artists.unwrap_or_default() {
            if let Some(name) = artist.name.as_deref().and_then(|n| normalized_text(n, 512)) {
                artists.push(name);
            }
        }
        let album = track
            .album
            .and_then(|a| a.name)
            .and_then(|name| normalized_text(&name, 512));
        tracks.push(PlaylistDetailTrack {
            id,
            title,
            artists,
            album,
            duration_ms: track.duration_ms,
        });
        if tracks.len() > MAX_TRACKS {
            return Err(());
        }
    }
    Ok(tracks)
}

fn normalize_youtube(body: &[u8]) -> Result<Vec<PlaylistDetailTrack>, ()> {
    let page: YoutubeItemsPage = bounded_parse(body)?;
    let mut tracks = Vec::new();
    for item in page.items {
        let Some(snippet) = item.snippet else {
            continue;
        };
        let (Some(video_id), Some(title)) = (
            snippet
                .resource_id
                .as_ref()
                .and_then(|r| r.video_id.as_deref()),
            snippet.title.as_deref(),
        ) else {
            continue;
        };
        let (Some(id), Some(title)) = (normalized_id(video_id), normalized_text(title, 512)) else {
            return Err(());
        };
        let artists = snippet
            .video_owner_channel_title
            .as_deref()
            .and_then(|channel| normalized_text(channel, 512))
            .into_iter()
            .collect();
        tracks.push(PlaylistDetailTrack {
            id,
            title,
            artists,
            album: None,
            duration_ms: None,
        });
        if tracks.len() > MAX_TRACKS {
            return Err(());
        }
    }
    Ok(tracks)
}

async fn spotify_detail(
    tokens: &mut aurora_providers::token::TokenSet,
    client: &aurora_providers::token::Client,
    http: &mut Box<dyn aurora_providers::playlist::Http + Send>,
    refresh_gate: &aurora_providers::token::RefreshGate,
    refresh_key: &str,
    playlist_id: &str,
) -> Result<PlaylistDetail, aurora_providers::playlist::Error> {
    let mut next: Option<String> = None;
    let mut tracks: Vec<PlaylistDetailTrack> = Vec::new();
    for _ in 0..MAX_PAGES {
        let mut url = Url::parse(&format!(
            "{SPOTIFY_PLAYLISTS_ENDPOINT}/{playlist_id}/tracks"
        ))
        .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
        url.query_pairs_mut()
            .append_pair("limit", "100")
            .append_pair(
                "fields",
                "items(track(id,name,artists(name),album(name),duration_ms)),next",
            );
        if let Some(offset) = next.as_deref().filter(|raw| limits::is_spotify_cursor(raw)) {
            url.query_pairs_mut().append_pair("offset", offset);
        }
        let response = aurora_providers::playlist::authorized_get(
            &mut *http,
            tokens,
            client,
            refresh_gate,
            refresh_key,
            url.to_string(),
            4,
        )
        .await?;
        let page: SpotifyPage = bounded_parse(&response.body)
            .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
        tracks.extend(
            normalize_spotify(&response.body)
                .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?,
        );
        next = page
            .next
            .as_deref()
            .and_then(|candidate| allowlisted_next(candidate, "api.spotify.com"))
            .and_then(|u| {
                u.query_pairs()
                    .find(|(k, _)| k == "offset")
                    .map(|(_, v)| v.into_owned())
            });
        if next.is_none() || tracks.len() >= MAX_TRACKS {
            break;
        }
    }
    if tracks.is_empty() && next.is_some() {
        return Err(aurora_providers::playlist::Error::InvalidResponse);
    }
    Ok(PlaylistDetail {
        name: String::new(),
        revision: None,
        tracks,
    })
}

async fn youtube_detail(
    tokens: &mut aurora_providers::token::TokenSet,
    client: &aurora_providers::token::Client,
    http: &mut Box<dyn aurora_providers::playlist::Http + Send>,
    refresh_gate: &aurora_providers::token::RefreshGate,
    refresh_key: &str,
    playlist_id: &str,
) -> Result<PlaylistDetail, aurora_providers::playlist::Error> {
    let mut page_token: Option<String> = None;
    let mut tracks: Vec<PlaylistDetailTrack> = Vec::new();
    for _ in 0..MAX_PAGES {
        let mut url = Url::parse(YOUTUBE_ITEMS_ENDPOINT)
            .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
        url.query_pairs_mut()
            .append_pair("part", "snippet")
            .append_pair("playlistId", playlist_id)
            .append_pair("maxResults", "50");
        if let Some(token) = page_token
            .as_deref()
            .filter(|raw| limits::is_page_token(raw))
        {
            url.query_pairs_mut().append_pair("pageToken", token);
        }
        let response = aurora_providers::playlist::authorized_get(
            &mut *http,
            tokens,
            client,
            refresh_gate,
            refresh_key,
            url.to_string(),
            4,
        )
        .await?;
        let page: YoutubeItemsPage = bounded_parse(&response.body)
            .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
        tracks.extend(
            normalize_youtube(&response.body)
                .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?,
        );
        page_token = page
            .next_page_token
            .filter(|token| limits::is_page_token(token));
        if page_token.is_none() || tracks.len() >= MAX_TRACKS {
            break;
        }
    }
    Ok(PlaylistDetail {
        name: String::new(),
        revision: None,
        tracks,
    })
}

pub(crate) async fn playlist_detail(
    Path(path): Path<PlaylistPath>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !["spotify", "youtube-music"].contains(&path.provider.as_str())
        || !origin_ok(&headers, &state.config)
    {
        return fail(StatusCode::FORBIDDEN);
    }
    if !limits::is_playlist_id(&path.playlist_id) {
        return fail(StatusCode::BAD_REQUEST);
    }
    // The same origin + CSRF + session gate the list endpoint uses: the request
    // must carry the CSRF token bound to the presented session, so a page on
    // another origin cannot drive this read with the visitor's cookie.
    let sid = match crate::authorize_csrf(&state, &headers).await {
        Ok(sid) => sid,
        Err(rejection) => {
            tracing::warn!(provider = %path.provider, "playlist detail rejected at origin/session/csrf gate");
            return rejection;
        }
    };
    let session_hash = hash(&sid);
    let ciphertext = match aurora_storage::get_oauth_token_ciphertext(
        &state.pool,
        &session_hash,
        &path.provider,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => {
            tracing::warn!(
                provider = %path.provider,
                session = %&session_hash[..8],
                "playlist detail: no stored tokens for this session"
            );
            return fail(StatusCode::NOT_FOUND);
        }
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let plaintext = match aurora_storage::decrypt_oauth_tokens(&state.encryption_key, &ciphertext) {
        Ok(value) => value,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let mut tokens = match aurora_providers::token::TokenSet::decode_native(&plaintext) {
        Ok(value) => value,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let (settings, _, token_endpoint, scopes) =
        match provider_settings(&state.config, &path.provider) {
            Some(value) => value,
            None => return fail(StatusCode::SERVICE_UNAVAILABLE),
        };
    let config = aurora_providers::ProviderConfig {
        id: path.provider.clone(),
        client_id: settings.client_id.clone(),
        authorization_endpoint: provider_descriptor(&path.provider)
            .map(|x| x.1)
            .unwrap_or_default()
            .into(),
        token_endpoint: token_endpoint.into(),
        scopes: scopes.split_whitespace().map(str::to_owned).collect(),
        enabled: true,
    };
    let client = match aurora_providers::token::Client::new(
        config,
        settings.client_secret.clone(),
        std::sync::Arc::new(ReqwestTokenTransport {
            http: crate::secure_http_client(),
        }),
        PROVIDER_TIMEOUT,
    ) {
        Ok(value) => value,
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE),
    };
    let detail_http = crate::ReqwestPlaylistHttp {
        http: detail_http_client(),
    };
    let mut detail_http: Box<dyn aurora_providers::playlist::Http + Send> = Box::new(detail_http);
    // Refresh serialization key: opaque, derived from the same hashes the
    // credential rows are keyed by. No token material crosses here.
    let refresh_key = format!("{session_hash}:{}", path.provider);
    let result = if path.provider == "spotify" {
        spotify_detail(
            &mut tokens,
            &client,
            &mut detail_http,
            &state.refresh_gate,
            &refresh_key,
            &path.playlist_id,
        )
        .await
    } else {
        youtube_detail(
            &mut tokens,
            &client,
            &mut detail_http,
            &state.refresh_gate,
            &refresh_key,
            &path.playlist_id,
        )
        .await
    };
    // Rotated credentials are persisted before the follow-up result is
    // interpreted, so a failed detail request cannot discard them.
    let persisted = crate::persist_after_request(
        &state,
        &session_hash,
        &path.provider,
        &tokens,
        result.as_ref().err(),
    )
    .await;
    let mut detail = match result {
        Ok(value) => value,
        Err(error) => return provider_error(error, &state, &session_hash, &path.provider).await,
    };
    if !persisted {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    detail.name = format!("{} playlist", path.provider);
    detail.revision = Some(hash(&format!("{}:{}", path.provider, path.playlist_id)));
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(detail),
    )
        .into_response()
}

fn detail_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(PROVIDER_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static playlist HTTP client configuration")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn oversized() -> Vec<u8> {
        let mut body = Vec::with_capacity(MAX_BODY_BYTES + 1);
        body.push(b'[');
        while body.len() <= MAX_BODY_BYTES {
            body.push(b' ');
        }
        body.push(b']');
        body
    }

    #[test]
    fn spotify_payload_is_normalized_to_neutral_tracks() {
        let body = br#"{"items":[
            {"track":{"id":"t1","name":"First","artists":[{"name":"A"},{"name":"B"}],"album":{"name":"LP"},"duration_ms":1000}},
            {"track":null},
            {"track":{"id":"t2","name":"Second","artists":[],"album":null,"duration_ms":null}}
        ],"next":null}"#;
        let tracks = normalize_spotify(body).unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].id, "t1");
        assert_eq!(tracks[0].title, "First");
        assert_eq!(tracks[0].artists, vec!["A".to_owned(), "B".to_owned()]);
        assert_eq!(tracks[0].album.as_deref(), Some("LP"));
        assert_eq!(tracks[0].duration_ms, Some(1000));
        assert_eq!(tracks[1].title, "Second");
        assert_eq!(tracks[1].album, None);
        assert_eq!(tracks[1].duration_ms, None);
    }

    #[test]
    fn youtube_payload_is_normalized_to_neutral_tracks() {
        let body = br#"{"items":[
            {"snippet":{"title":"Video","videoOwnerChannelTitle":"Channel","resourceId":{"videoId":"v1"}}},
            {"snippet":{"title":"no resource"}}
        ],"nextPageToken":"CAEQABgE"}"#;
        let tracks = normalize_youtube(body).unwrap();
        assert_eq!(tracks.len(), 1);
        assert_eq!(tracks[0].id, "v1");
        assert_eq!(tracks[0].artists, vec!["Channel".to_owned()]);
        assert_eq!(tracks[0].album, None);
        assert_eq!(tracks[0].duration_ms, None);
        let page: YoutubeItemsPage = bounded_parse(body).unwrap();
        assert_eq!(page.next_page_token.as_deref(), Some("CAEQABgE"));
    }

    #[test]
    fn malformed_payloads_are_rejected() {
        assert!(normalize_spotify(b"{").is_err());
        assert!(normalize_youtube(b"{").is_err());
        // Unknown fields mean the provider contract changed: reject, do not guess.
        assert!(normalize_spotify(br#"{"items":[],"next":null,"total":3}"#).is_err());
        assert!(normalize_youtube(br#"{"items":[],"unexpected":true}"#).is_err());
        // Empty or hostile identifiers inside an otherwise valid envelope.
        assert!(
            normalize_spotify(br#"{"items":[{"track":{"id":"","name":"x"}}],"next":null}"#)
                .is_err()
        );
        assert!(
            normalize_spotify(
                br#"{"items":[{"track":{"id":"../../etc","name":"x"}}],"next":null}"#
            )
            .is_err()
        );
        assert!(
            normalize_youtube(
                br#"{"items":[{"snippet":{"title":"t","resource_id":{"video_id":"<script>"}}}]}"#
            )
            .is_err()
        );
        assert!(
            normalize_youtube(
                br#"{"items":[{"snippet":{"title":"   ","resource_id":{"video_id":"v1"}}}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn oversized_payloads_are_rejected() {
        assert!(bounded_parse::<serde_json::Value>(&oversized()).is_err());
        assert!(normalize_spotify(&oversized()).is_err());
        assert!(normalize_youtube(&oversized()).is_err());
    }

    #[test]
    fn pagination_links_must_stay_on_the_allowlisted_host() {
        assert!(
            allowlisted_next(
                "https://api.spotify.com/v1/me/playlists?offset=25",
                "api.spotify.com"
            )
            .is_some()
        );
        assert!(
            allowlisted_next("http://api.spotify.com/v1/me/playlists", "api.spotify.com").is_none()
        );
        assert!(
            allowlisted_next("https://evil.example/v1/me/playlists", "api.spotify.com").is_none()
        );
        assert!(
            allowlisted_next(
                "https://api.spotify.com.evil.example/next",
                "api.spotify.com"
            )
            .is_none()
        );
        assert!(
            allowlisted_next("https://user:pass@api.spotify.com/next", "api.spotify.com").is_none()
        );
        assert!(allowlisted_next("not a url", "api.spotify.com").is_none());
    }

    #[test]
    fn fixed_endpoints_are_https_and_provider_specific() {
        for endpoint in [SPOTIFY_PLAYLISTS_ENDPOINT, YOUTUBE_ITEMS_ENDPOINT] {
            let url = Url::parse(endpoint).unwrap();
            assert_eq!(url.scheme(), "https");
            assert!(
                matches!(
                    url.host_str(),
                    Some("api.spotify.com") | Some("www.googleapis.com")
                ),
                "{endpoint}"
            );
        }
    }

    #[tokio::test]
    async fn detail_handler_requires_origin_and_csrf() {
        let state = crate::memory_state(crate::tests::config());
        let path = Path(PlaylistPath {
            provider: "spotify".into(),
            playlist_id: "abc123".into(),
        });
        let no_origin = crate::playlists::playlist_detail(
            Path(PlaylistPath {
                provider: "spotify".into(),
                playlist_id: "abc123".into(),
            }),
            State(state.clone()),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);
        // With an allowed origin but no session at all, the gate answers 401
        // before it ever looks for a CSRF token: absence of a session is the
        // narrower failure and is reported as such.
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://app.example".parse().unwrap());
        let no_session = crate::playlists::playlist_detail(path, State(state), headers).await;
        assert_eq!(no_session.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn detail_handler_rejects_a_bad_provider_and_playlist_id() {
        let state = crate::memory_state(crate::tests::config());
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, "https://app.example".parse().unwrap());
        let bad_provider = crate::playlists::playlist_detail(
            Path(PlaylistPath {
                provider: "apple-music".into(),
                playlist_id: "abc123".into(),
            }),
            State(state.clone()),
            headers.clone(),
        )
        .await;
        assert_eq!(bad_provider.status(), StatusCode::FORBIDDEN);
        for hostile in ["", "../../etc/passwd", "abc/def", "abc def", "abc\"def"] {
            let rejected = crate::playlists::playlist_detail(
                Path(PlaylistPath {
                    provider: "spotify".into(),
                    playlist_id: hostile.into(),
                }),
                State(state.clone()),
                headers.clone(),
            )
            .await;
            assert_eq!(
                rejected.status(),
                StatusCode::BAD_REQUEST,
                "accepted {hostile:?}"
            );
        }
    }
}
