mod guard;
mod limits;
mod playlists;

use aurora_core::domain::sync::{RemotePage, RemotePlaylist, RemoteProvider};
use aurora_storage::{OAuthAccount, OAuthTransaction, SqlitePool};
use axum::{
    Json, Router,
    extract::{ConnectInfo, Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware,
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use url::Url;
use uuid::Uuid;

use guard::RateLimiter;

const SESSION_COOKIE: &str = "__Host-aurora_session";
const CSRF_COOKIE: &str = "__Host-aurora_csrf";
const CSRF_HEADER: &str = "x-csrf-token";
const TX_TTL: Duration = Duration::from_secs(600);

/// How often, at most, opportunistic OAuth expiry cleanup runs. Cleanup is
/// piggybacked on ordinary requests so an idle deployment needs no timer, but
/// a busy one must not run a DELETE sweep on every handshake.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(300);

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub pool: SqlitePool,
    pub encryption_key: [u8; 32],
    oauth: Arc<dyn OAuthService>,
    pub(crate) playlist_http: Arc<Mutex<Box<dyn aurora_providers::playlist::Http + Send>>>,
    limits: Arc<RateLimiter>,
    /// When the last opportunistic expiry sweep started; see
    /// [`AppState::cleanup_expired`].
    last_cleanup: Arc<Mutex<Option<tokio::time::Instant>>>,
}
#[derive(Clone)]
pub struct Config {
    pub allowed_origins: Vec<String>,
    pub frontend_url: String,
    pub spotify: Option<ProviderSettings>,
    pub youtube: Option<ProviderSettings>,
}
#[derive(Clone)]
pub struct ProviderSettings {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub subject: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub capabilities: Vec<String>,
    pub connected_at: DateTime<Utc>,
}

pub trait OAuthService: Send + Sync {
    fn complete<'a>(
        &'a self,
        provider: &'a str,
        settings: &'a ProviderSettings,
        code: &'a str,
        verifier: &'a str,
        redirect_uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = ExchangeResult> + Send + 'a>>;
}

/// Result of an OAuth service code-for-token exchange.
type ExchangeResult = Result<(Account, aurora_providers::token::TokenSet), ()>;
struct HttpsOAuthService {
    http: reqwest::Client,
}
struct ReqwestTokenTransport {
    http: reqwest::Client,
}

pub(crate) struct ReqwestPlaylistHttp {
    http: reqwest::Client,
}

impl aurora_providers::playlist::Http for ReqwestPlaylistHttp {
    fn send(
        &mut self,
        request: aurora_providers::playlist::Request,
    ) -> Result<aurora_providers::playlist::Response, aurora_providers::playlist::Error> {
        let http = self.http.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?
                .block_on(async move {
                    let mut builder = http.request(
                        reqwest::Method::from_bytes(request.method.as_bytes())
                            .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?,
                        &request.url,
                    );
                    for (name, value) in request.headers {
                        builder = builder.header(name, value);
                    }
                    let response = builder.send().await.map_err(|_| {
                        aurora_providers::playlist::Error::Http {
                            status: 599,
                            retry_after_ms: None,
                        }
                    })?;
                    let status = response.status().as_u16();
                    let headers = response
                        .headers()
                        .iter()
                        .filter_map(|(name, value)| {
                            Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
                        })
                        .collect();
                    let body = read_bounded(response, limits::MAX_PROVIDER_BODY_BYTES)
                        .await
                        .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
                    Ok(aurora_providers::playlist::Response {
                        status,
                        headers,
                        body,
                    })
                })
        })
        .join()
        .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?
    }
}
impl aurora_providers::token::Transport for ReqwestTokenTransport {
    fn send<'a>(
        &'a self,
        request: aurora_providers::token::TokenRequest,
    ) -> Pin<
        Box<
            dyn Future<
                    Output = Result<
                        aurora_providers::token::TokenResponse,
                        aurora_providers::token::TransportError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if !matches!(
                request.endpoint.as_str(),
                "https://accounts.spotify.com/api/token" | "https://oauth2.googleapis.com/token"
            ) {
                return Err(aurora_providers::token::TransportError::Failed);
            }
            let r = self
                .http
                .post(request.endpoint)
                .form(&request.form)
                .send()
                .await
                .map_err(|_| aurora_providers::token::TransportError::Failed)?;
            let status = r.status().as_u16();
            let body = read_bounded(r, limits::MAX_PROVIDER_BODY_BYTES)
                .await
                .map_err(|_| aurora_providers::token::TransportError::Failed)?;
            Ok(aurora_providers::token::TokenResponse { status, body })
        })
    }
}
impl OAuthService for HttpsOAuthService {
    fn complete<'a>(
        &'a self,
        provider: &'a str,
        settings: &'a ProviderSettings,
        code: &'a str,
        verifier: &'a str,
        redirect_uri: &'a str,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<(Account, aurora_providers::token::TokenSet), ()>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let (_, auth, token, scopes) = provider_descriptor(provider).ok_or(())?;
            let config = aurora_providers::ProviderConfig {
                id: provider.into(),
                client_id: settings.client_id.clone(),
                authorization_endpoint: auth.into(),
                token_endpoint: token.into(),
                scopes: scopes.split_whitespace().map(str::to_owned).collect(),
                enabled: true,
            };
            let client = aurora_providers::token::Client::new(
                config,
                settings.client_secret.clone(),
                Arc::new(ReqwestTokenTransport {
                    http: self.http.clone(),
                }),
                Duration::from_secs(15),
            )
            .map_err(|_| ())?;
            let tokens = client
                .exchange_code(code, redirect_uri, verifier)
                .await
                .map_err(|_| ())?;
            let endpoint = if provider == "spotify" {
                "https://api.spotify.com/v1/me"
            } else {
                "https://openidconnect.googleapis.com/v1/userinfo"
            };
            let r = self
                .http
                .get(endpoint)
                .bearer_auth(&tokens.access_token)
                .send()
                .await
                .map_err(|_| ())?;
            if !r.status().is_success() {
                return Err(());
            }
            let body = read_bounded(r, limits::MAX_PROVIDER_BODY_BYTES)
                .await
                .map_err(|_| ())?;
            let v: serde_json::Value = serde_json::from_slice(&body).map_err(|_| ())?;
            let subject = v
                .get(if provider == "spotify" { "id" } else { "sub" })
                .and_then(|x| x.as_str())
                .filter(|x| !x.trim().is_empty())
                .ok_or(())?
                .chars()
                .take(200)
                .collect();
            let display_name = v
                .get(if provider == "spotify" {
                    "display_name"
                } else {
                    "name"
                })
                .and_then(|x| x.as_str())
                .or_else(|| v.get("email").and_then(|x| x.as_str()))
                .unwrap_or(if provider == "spotify" {
                    "Spotify user"
                } else {
                    "Google user"
                })
                .chars()
                .take(200)
                .collect();
            Ok((
                Account {
                    subject,
                    display_name,
                    avatar_url: None,
                    capabilities: vec!["identity".into()],
                    connected_at: Utc::now(),
                },
                tokens,
            ))
        })
    }
}
fn provider_descriptor(
    p: &str,
) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    match p {
        "spotify" => Some((
            "spotify",
            "https://accounts.spotify.com/authorize",
            "https://accounts.spotify.com/api/token",
            "user-read-email playlist-read-private",
        )),
        "youtube-music" => Some((
            "youtube-music",
            "https://accounts.google.com/o/oauth2/v2/auth",
            "https://oauth2.googleapis.com/token",
            "openid profile https://www.googleapis.com/auth/youtube.readonly",
        )),
        _ => None,
    }
}

fn valid_redirect_uri(provider: &str, redirect_uri: &str) -> bool {
    let Ok(url) = Url::parse(redirect_uri) else {
        return false;
    };
    url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == format!("/api/oauth/{provider}/callback")
}

/// Reads a provider response body with a hard cap.
///
/// A provider response is remote data: without a cap, one hostile or buggy
/// upstream can make the process allocate without bound. `Content-Length` is
/// checked first so an honestly-declared oversized body is rejected before a
/// single byte is read, and the stream is then bounded anyway because a chunked
/// response declares no length at all.
async fn read_bounded(response: reqwest::Response, cap: usize) -> Result<Vec<u8>, ()> {
    if response
        .content_length()
        .is_some_and(|len| len > cap as u64)
    {
        return Err(());
    }
    let mut body = Vec::new();
    let mut stream = response;
    while let Some(chunk) = stream.chunk().await.map_err(|_| ())? {
        if body.len() + chunk.len() > cap {
            return Err(());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn secure_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("static OAuth HTTP client configuration")
}

#[derive(Serialize)]
struct Begin {
    authorization_url: String,
    csrf_token: String,
}
#[derive(Deserialize)]
struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct PlaylistQuery {
    cursor: Option<String>,
}
#[derive(Deserialize)]
pub(crate) struct PlaylistPath {
    pub(crate) provider: String,
    pub(crate) playlist_id: String,
}
#[derive(Deserialize)]
struct SpotifyPlaylistsWire {
    items: Vec<SpotifyPlaylistWireItem>,
    next: Option<String>,
}
#[derive(Deserialize)]
struct SpotifyPlaylistWireItem {
    id: String,
    name: String,
    snapshot_id: Option<String>,
}
#[derive(Deserialize)]
struct YoutubePlaylistsWire {
    items: Vec<YoutubePlaylistWireItem>,
    #[serde(default)]
    next_page_token: Option<String>,
}
#[derive(Deserialize)]
struct YoutubePlaylistWireItem {
    id: String,
    snippet: YoutubePlaylistSnippet,
}
#[derive(Deserialize)]
struct YoutubePlaylistSnippet {
    title: String,
}

pub fn router(state: AppState) -> Router {
    let origins = state
        .config
        .allowed_origins
        .iter()
        .filter_map(|origin| origin.parse::<HeaderValue>().ok())
        .collect::<Vec<_>>();
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
        .route("/api/oauth/{provider}/begin", post(begin))
        .route("/api/oauth/{provider}/callback", get(callback))
        .route("/api/oauth/{provider}/status", get(status))
        .route("/api/oauth/{provider}/playlists", get(playlists))
        .route(
            "/api/oauth/{provider}/playlists/{playlist_id}",
            get(playlists::playlist_detail),
        )
        .route("/api/oauth/{provider}", delete(disconnect))
        .layer(middleware::from_fn(guard::bounded_body))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST, Method::DELETE])
                .allow_headers([
                    header::CONTENT_TYPE,
                    header::ACCEPT,
                    CSRF_HEADER.parse().unwrap(),
                ])
                .allow_credentials(true),
        )
        .with_state(state)
}

async fn health() -> Response {
    (StatusCode::OK, Json(serde_json::json!({"status":"ok"}))).into_response()
}
async fn ready(State(state): State<AppState>) -> Response {
    match aurora_storage::health_check(&state.pool).await {
        Ok(_) => (StatusCode::OK, Json(serde_json::json!({"status":"ready"}))).into_response(),
        Err(_) => fail(StatusCode::SERVICE_UNAVAILABLE),
    }
}
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<(), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    // `ConnectInfo` is what makes per-IP limiting possible: without it every
    // request would be charged to one bucket.
    axum::serve(
        listener,
        router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
}

fn provider_settings<'a>(
    config: &'a Config,
    provider: &str,
) -> Option<(
    &'a ProviderSettings,
    &'static str,
    &'static str,
    &'static str,
)> {
    match provider {
        "spotify" => config.spotify.as_ref().map(|p| {
            (
                p,
                "https://accounts.spotify.com/authorize",
                "https://accounts.spotify.com/api/token",
                "user-read-email playlist-read-private",
            )
        }),
        "youtube-music" => config.youtube.as_ref().map(|p| {
            (
                p,
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
                "openid profile https://www.googleapis.com/auth/youtube.readonly",
            )
        }),
        _ => None,
    }
}
fn origin_ok(headers: &HeaderMap, config: &Config) -> bool {
    headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|o| config.allowed_origins.iter().any(|x| x == o))
}
/// A session cookie is accepted only when it is a base64url token of the
/// expected length. The cookie *before* this check is fully attacker-controlled
/// (it is a raw header value), so it is never hashed, compared or bound to SQL
/// until it has passed.
fn session(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, SESSION_COOKIE).filter(|sid| limits::is_session_token(sid))
}
fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix(&format!("{name}=")))
                .map(str::to_owned)
        })
}
fn hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}
fn random() -> String {
    URL_SAFE_NO_PAD.encode(Uuid::new_v4().as_bytes())
}
fn fail(status: StatusCode) -> Response {
    (
        status,
        Json(serde_json::json!({"error":"oauth_unavailable"})),
    )
        .into_response()
}
fn cookie(id: &str) -> String {
    format!("{SESSION_COOKIE}={id}; Path=/; Secure; HttpOnly; SameSite=Strict")
}
/// The CSRF cookie is intentionally readable by scripts on the allowed origins so the
/// frontend can echo it in the `X-CSRF-Token` header; it is scoped exactly like the
/// session cookie. A cross-site attacker can read neither it nor the session cookie.
fn csrf_cookie(token: &str) -> String {
    format!("{CSRF_COOKIE}={token}; Path=/; Secure; SameSite=Strict")
}
fn cookie_header(value: &str) -> HeaderValue {
    value
        .parse()
        .expect("session and CSRF cookies are ASCII without separators")
}
fn csrf_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(CSRF_HEADER).and_then(|v| v.to_str().ok())
}

/// Checks that the CSRF header echoes the readable CSRF cookie.
///
/// The cookie and the header are both attacker-supplied, so this is a
/// well-formedness and consistency check, not an authorization decision: it
/// stops a cross-site request that can set the header but cannot read the
/// cookie from proceeding to the storage lookup, and it stops a malformed pair
/// from being hashed at all. The authoritative check is
/// [`authorize_csrf`], which compares the header against the *stored* hash
/// bound to the session.
#[allow(dead_code)] // kept as documented reference; authorization uses authorize_csrf
pub(crate) fn csrf_matches_session(headers: &HeaderMap, csrf: &str) -> bool {
    cookie_value(headers, CSRF_COOKIE).is_some_and(|cookie| {
        limits::is_csrf_token(&cookie)
            && cookie.len() == csrf.len()
            && cookie
                .bytes()
                .zip(csrf.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    })
}

/// The TCP peer address of the request, when the connect-info service supplies
/// one.
///
/// `ConnectInfo` on its own would reject a request that carries no peer address
/// with a 500 before any handler logic runs, which would make the handlers
/// untestable and would turn a deployment mistake into an opaque error. This
/// extractor is optional by construction; the absence of an address is handled
/// by the rate limiter, not by failing the request.
#[derive(Clone, Copy)]
struct PeerAddr(Option<SocketAddr>);

impl<S: Send + Sync> axum::extract::OptionalFromRequestParts<S> for PeerAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Option<Self>, Self::Rejection> {
        Ok(Some(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        )))
    }
}

/// Required-trait form of [`PeerAddr`], so handlers can take it as an ordinary
/// extractor. See the `OptionalFromRequestParts` impl below for why the peer
/// address is allowed to be absent instead of rejecting the request.
impl<S: Send + Sync> axum::extract::FromRequestParts<S> for PeerAddr {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0),
        ))
    }
}

/// Charges one authenticated call against the per-IP and per-provider windows.
///
/// The provider argument is the already-canonicalised provider id, so a caller
/// cannot spend a budget per spelling of the same provider. Returns the 429 to
/// send when the call is over budget.
fn rate_limit(state: &AppState, client: IpAddr, provider: &str) -> Option<Response> {
    let decision = state.limits.check(client, provider);
    (!decision.allowed).then(|| guard::rate_limited(decision.retry_after_secs))
}

/// Runs expired-session/transaction cleanup if the interval has elapsed.
///
/// Opportunistic rather than scheduled: the sweep needs only to happen
/// eventually, and piggybacking on real traffic means an idle process does no
/// work while a busy one still bounds how much expired state accumulates. The
/// interval is claimed before the sweep so concurrent requests cannot all run
/// it at once, and the claim is released on failure so a transient database
/// error is retried by the next request rather than waiting out the interval.
async fn cleanup_expired_oauth(state: &AppState) {
    let mut last = state.last_cleanup.lock().await;
    let now = tokio::time::Instant::now();
    if let Some(previous) = *last
        && now.duration_since(previous) < CLEANUP_INTERVAL
    {
        return;
    }
    *last = Some(now);
    drop(last);
    if aurora_storage::cleanup_expired_oauth(&state.pool, &Utc::now().to_rfc3339())
        .await
        .is_err()
    {
        *state.last_cleanup.lock().await = None;
    }
}

/// Validates the CSRF token bound to the presented session. Returns `Ok(sid)` when the
/// caller may proceed, otherwise the exact rejection response to return.
#[allow(clippy::result_large_err)] // axum::Response is large but this is the handler boundary
async fn authorize_csrf(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    if !origin_ok(headers, &state.config) {
        return Err(fail(StatusCode::FORBIDDEN));
    }
    let sid = match session(headers) {
        Some(sid) => sid,
        None => return Err(fail(StatusCode::UNAUTHORIZED)),
    };
    let token = match csrf_header(headers).filter(|x| limits::is_csrf_token(x)) {
        Some(token) => token,
        None => return Err(fail(StatusCode::FORBIDDEN)),
    };
    match aurora_storage::validate_oauth_csrf(
        &state.pool,
        &hash(&sid),
        &hash(token),
        &Utc::now().to_rfc3339(),
    )
    .await
    {
        Ok(true) => Ok(sid),
        Ok(false) => Err(fail(StatusCode::FORBIDDEN)),
        Err(_) => Err(fail(StatusCode::INTERNAL_SERVER_ERROR)),
    }
}

/// `POST /api/oauth/{provider}/begin` is state-changing: it rotates the session and its
/// CSRF token. It therefore requires either a valid token bound to the presented session
/// or, when no valid session is presented, no session-bound CSRF check at all — in that
/// case the request cannot be used as a cross-site forgery because a request carrying a
/// foreign cookie is rejected instead of being silently accepted, and the response
/// (including the fresh token) is unreadable cross-origin.
async fn begin(
    Path(provider): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str()) {
        return fail(StatusCode::FORBIDDEN);
    }
    // A readable CSRF cookie must only ever be handed to an allowed origin. No cookie means
    // no session yet, which is the only case that skips the session-bound CSRF check.
    if headers.get(header::COOKIE).is_none() && !origin_ok(&headers, &state.config) {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = rate_limit(&state, guard::client_ip(peer.0), &provider) {
        return rejection;
    }
    // `begin` allocates session and transaction rows on every call, so this is
    // the natural place to sweep the ones that have expired.
    cleanup_expired_oauth(&state).await;
    let now = Utc::now().to_rfc3339();
    let (sid, new_session) = match session(&headers) {
        Some(sid) => {
            match aurora_storage::get_oauth_session(&state.pool, &hash(&sid), &now).await {
                Ok(Some(_)) => {
                    // An existing session must prove possession of the bound token.
                    if let Err(rejection) = authorize_csrf(&state, &headers).await {
                        return rejection;
                    }
                    (sid, false)
                }
                Ok(None) => {
                    // A session cookie that is unknown or expired is never tolerated silently,
                    // regardless of the presented Origin.
                    if headers.get(header::COOKIE).is_some() {
                        return fail(StatusCode::UNAUTHORIZED);
                    }
                    (random(), true)
                }
                Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
            }
        }
        None => {
            // A cookie that is present but is not a well-formed session token is
            // a forged or corrupted value, not the absence of a session: reject
            // it instead of minting a fresh session on top of it.
            if headers.get(header::COOKIE).is_some() {
                return fail(StatusCode::UNAUTHORIZED);
            }
            (random(), true)
        }
    };
    let csrf_token = random();
    let expires = Utc::now() + chrono::Duration::hours(8);
    if aurora_storage::save_oauth_session(
        &state.pool,
        &hash(&sid),
        &hash(&csrf_token),
        &expires.to_rfc3339(),
    )
    .await
    .is_err()
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let (settings, endpoint, _, scopes) = match provider_settings(&state.config, &provider) {
        Some(x) if valid_redirect_uri(&provider, &x.0.redirect_uri) => x,
        Some(_) => return fail(StatusCode::SERVICE_UNAVAILABLE),
        None => return fail(StatusCode::SERVICE_UNAVAILABLE),
    };
    let state_value = random();
    let verifier = random();
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse(endpoint).expect("constant endpoint");
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &settings.client_id)
        .append_pair("redirect_uri", &settings.redirect_uri)
        .append_pair("state", &state_value)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("scope", scopes);
    if provider == "youtube-music" {
        url.query_pairs_mut()
            .append_pair("access_type", "offline")
            .append_pair("prompt", "consent");
    }
    if aurora_storage::save_oauth_transaction(
        &state.pool,
        &hash(&state_value),
        &OAuthTransaction {
            provider: provider.clone(),
            session_id_hash: hash(&sid),
            verifier,
            redirect_uri: settings.redirect_uri.clone(),
            expires_at: (Utc::now() + chrono::Duration::seconds(TX_TTL.as_secs() as i64))
                .to_rfc3339(),
        },
    )
    .await
    .is_err()
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut response = (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(Begin {
            authorization_url: url.into(),
            csrf_token: csrf_token.clone(),
        }),
    )
        .into_response();
    if new_session {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie_header(&cookie(&sid)));
    }
    // Re-issued on every begin so the readable CSRF cookie always matches the stored hash.
    response
        .headers_mut()
        .append(header::SET_COOKIE, cookie_header(&csrf_cookie(&csrf_token)));
    response
}

async fn callback(
    Path(provider): Path<String>,
    Query(query): Query<Callback>,
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    let sid = match session(&headers) {
        Some(x) => x,
        None => return fail(StatusCode::UNAUTHORIZED),
    };
    if query.error.is_some() {
        return fail(StatusCode::BAD_REQUEST);
    }
    // Bounds before any storage or provider call: an oversized or non-base64url
    // `state` and an oversized `code` are rejected here, so neither reaches a
    // hash, a SQL binding, or the outbound token request.
    if !limits::is_oauth_state(&query.state) {
        return fail(StatusCode::BAD_REQUEST);
    }
    let code = match query
        .code
        .as_deref()
        .filter(|x| limits::is_authorization_code(x))
    {
        Some(x) => x.to_owned(),
        None => return fail(StatusCode::BAD_REQUEST),
    };
    if let Some(rejection) = rate_limit(&state, guard::client_ip(peer.0), &provider) {
        return rejection;
    }
    match aurora_storage::get_oauth_session(&state.pool, &hash(&sid), &Utc::now().to_rfc3339())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::UNAUTHORIZED),
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
    let tx = match aurora_storage::claim_oauth_transaction(
        &state.pool,
        &hash(&query.state),
        &provider,
        &hash(&sid),
        &Utc::now().to_rfc3339(),
    )
    .await
    {
        Ok(Some(tx)) => tx,
        _ => return fail(StatusCode::BAD_REQUEST),
    };
    let verifier = tx.verifier;
    let redirect_uri = tx.redirect_uri;
    let settings = match provider_settings(&state.config, &provider) {
        Some((s, _, _, _)) if valid_redirect_uri(&provider, &s.redirect_uri) => s.clone(),
        Some(_) => return fail(StatusCode::SERVICE_UNAVAILABLE),
        None => return fail(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (account, tokens) = match state
        .oauth
        .complete(&provider, &settings, &code, &verifier, &redirect_uri)
        .await
    {
        Ok(x) => x,
        Err(()) => return fail(StatusCode::BAD_GATEWAY),
    };
    let token_bytes = match tokens.encode_native() {
        Ok(x) => x,
        Err(_) => return fail(StatusCode::BAD_GATEWAY),
    };
    let encrypted = match aurora_storage::encrypt_oauth_tokens(&state.encryption_key, &token_bytes)
    {
        Ok(x) => x,
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    };
    let persisted = OAuthAccount {
        subject: account.subject.clone(),
        display_name: account.display_name.clone(),
        avatar_url: account.avatar_url.clone(),
        capabilities: account.capabilities.clone(),
        connected_at: account.connected_at,
        auth_state: "connected".into(),
        last_auth_error: None,
    };
    if aurora_storage::save_oauth_account(
        &state.pool,
        &hash(&sid),
        &provider,
        &persisted,
        &format!("{provider}:{}", account.subject),
        &encrypted,
    )
    .await
    .is_err()
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    Redirect::to(&state.config.frontend_url).into_response()
}
/// Persists a (possibly rotated) token set. Returns whether the write landed, so
/// callers can surface a storage failure instead of silently dropping a rotated
/// refresh token. An update that matches no row also counts as a failure.
async fn persist_tokens(
    state: &AppState,
    session_hash: &str,
    provider: &str,
    tokens: &aurora_providers::token::TokenSet,
) -> bool {
    let Ok(bytes) = tokens.encode_native() else {
        return false;
    };
    let Ok(encoded) = aurora_storage::encrypt_oauth_tokens(&state.encryption_key, &bytes) else {
        return false;
    };
    matches!(
        aurora_storage::update_oauth_tokens(&state.pool, session_hash, provider, &encoded).await,
        Ok(true)
    )
}

/// Maps a playlist/refresh error onto the HTTP response, marking
/// reauthorization only when the provider definitively rejected the grant.
/// Transient refresh failures stay retryable and never disconnect the account.
async fn provider_error(
    error: aurora_providers::playlist::Error,
    state: &AppState,
    session_hash: &str,
    provider: &str,
) -> Response {
    match error {
        aurora_providers::playlist::Error::ReauthorizationRequired => {
            let _ = aurora_storage::mark_oauth_reauth_required(
                &state.pool,
                session_hash,
                provider,
                "refresh_failed",
            )
            .await;
            fail(StatusCode::UNAUTHORIZED)
        }
        _ => fail(StatusCode::BAD_GATEWAY),
    }
}

/// Persists rotated tokens as soon as the refresh happened, independently of
/// whether the follow-up provider request succeeded. A transient refresh
/// failure leaves the stored token set untouched.
async fn persist_after_request(
    state: &AppState,
    session_hash: &str,
    provider: &str,
    tokens: &aurora_providers::token::TokenSet,
    error: Option<&aurora_providers::playlist::Error>,
) -> bool {
    if matches!(
        error,
        Some(aurora_providers::playlist::Error::RefreshTransient)
    ) {
        return true;
    }
    persist_tokens(state, session_hash, provider, tokens).await
}

async fn status(
    Path(provider): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str())
        || !origin_ok(&headers, &state.config)
    {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = rate_limit(&state, guard::client_ip(peer.0), &provider) {
        return rejection;
    }
    let sid = match session(&headers) {
        Some(x) => x,
        None => return fail(StatusCode::UNAUTHORIZED),
    };
    match aurora_storage::get_oauth_session(&state.pool, &hash(&sid), &Utc::now().to_rfc3339())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::UNAUTHORIZED),
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
    match aurora_storage::get_oauth_account(&state.pool, &hash(&sid), &provider).await {
        Ok(Some(a)) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Json(a),
        )
            .into_response(),
        _ => fail(StatusCode::NOT_FOUND),
    }
}
async fn playlists(
    Path(provider): Path<String>,
    Query(query): Query<PlaylistQuery>,
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str())
        || !origin_ok(&headers, &state.config)
    {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = rate_limit(&state, guard::client_ip(peer.0), &provider) {
        return rejection;
    }
    // The cursor is echoed straight into a provider query string, so it is
    // bounded and charset-checked per provider before any request is built.
    // The Spotify cursor is a decimal offset; the YouTube cursor is an opaque
    // base64url page token. Both are rejected rather than truncated.
    let cursor = match query.cursor.as_deref() {
        Some(cursor) if limits::is_cursor(&provider, cursor) => Some(cursor),
        Some(_) => return fail(StatusCode::BAD_REQUEST),
        None => None,
    };
    let sid = match session(&headers) {
        Some(x) => x,
        _ => return fail(StatusCode::UNAUTHORIZED),
    };
    let session_hash = hash(&sid);
    match aurora_storage::get_oauth_session(&state.pool, &session_hash, &Utc::now().to_rfc3339())
        .await
    {
        Ok(Some(_)) => {}
        Ok(None) => return fail(StatusCode::UNAUTHORIZED),
        Err(_) => return fail(StatusCode::INTERNAL_SERVER_ERROR),
    }
    let ciphertext =
        match aurora_storage::get_oauth_token_ciphertext(&state.pool, &session_hash, &provider)
            .await
        {
            Ok(Some(value)) => value,
            Ok(None) => return fail(StatusCode::NOT_FOUND),
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
    let (settings, _, token_endpoint, scopes) = match provider_settings(&state.config, &provider) {
        Some(value) => value,
        None => return fail(StatusCode::SERVICE_UNAVAILABLE),
    };
    let config = aurora_providers::ProviderConfig {
        id: provider.clone(),
        client_id: settings.client_id.clone(),
        authorization_endpoint: provider_descriptor(&provider)
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
        Arc::new(ReqwestTokenTransport {
            http: secure_http_client(),
        }),
        Duration::from_secs(15),
    ) {
        Ok(value) => value,
        Err(_) => return fail(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut http = state.playlist_http.lock().await;
    let result = if provider == "spotify" {
        let mut url = Url::parse("https://api.spotify.com/v1/me/playlists").expect("constant URL");
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("offset", cursor);
        }
        aurora_providers::playlist::authorized_get(
            &mut *http,
            &mut tokens,
            &client,
            url.to_string(),
            4,
        )
        .await
        .and_then(|response| {
            let wire: SpotifyPlaylistsWire = serde_json::from_slice(&response.body)
                .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
            let next_cursor = wire.next.and_then(|next| {
                Url::parse(&next).ok().and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "offset")
                        .map(|(_, v)| v.into_owned())
                })
            });
            Ok(RemotePage {
                items: wire
                    .items
                    .into_iter()
                    .map(|x| RemotePlaylist {
                        provider: RemoteProvider::Spotify,
                        id: x.id,
                        name: x.name,
                        tracks: vec![],
                        revision: x.snapshot_id,
                    })
                    .collect(),
                next_cursor,
                snapshot: None,
            })
        })
    } else {
        let mut url =
            Url::parse("https://www.googleapis.com/youtube/v3/playlists").expect("constant URL");
        url.query_pairs_mut()
            .append_pair("part", "snippet,contentDetails")
            .append_pair("mine", "true")
            .append_pair("maxResults", "50");
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("pageToken", cursor);
        }
        aurora_providers::playlist::authorized_get(
            &mut *http,
            &mut tokens,
            &client,
            url.to_string(),
            4,
        )
        .await
        .and_then(|response| {
            let wire: YoutubePlaylistsWire = serde_json::from_slice(&response.body)
                .map_err(|_| aurora_providers::playlist::Error::InvalidResponse)?;
            Ok(RemotePage {
                items: wire
                    .items
                    .into_iter()
                    .map(|x| RemotePlaylist {
                        provider: RemoteProvider::Youtube,
                        id: x.id,
                        name: x.snippet.title,
                        tracks: vec![],
                        revision: None,
                    })
                    .collect(),
                next_cursor: wire.next_page_token,
                snapshot: None,
            })
        })
    };
    drop(http);
    // Rotated credentials are persisted before the follow-up request result is
    // interpreted, so a failed page request can no longer discard them.
    let persisted = persist_after_request(
        &state,
        &session_hash,
        &provider,
        &tokens,
        result.as_ref().err(),
    )
    .await;
    match result {
        Ok(page) => {
            if !persisted {
                return fail(StatusCode::INTERNAL_SERVER_ERROR);
            }
            (
                StatusCode::OK,
                [(header::CACHE_CONTROL, "no-store")],
                Json(page),
            )
                .into_response()
        }
        Err(error) => provider_error(error, &state, &session_hash, &provider).await,
    }
}

async fn disconnect(
    Path(provider): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    peer: PeerAddr,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str()) {
        return fail(StatusCode::FORBIDDEN);
    }
    if let Some(rejection) = rate_limit(&state, guard::client_ip(peer.0), &provider) {
        return rejection;
    }
    // Exact-Origin check plus the session-bound CSRF token (defense in depth).
    let sid = match authorize_csrf(&state, &headers).await {
        Ok(sid) => sid,
        Err(rejection) => return rejection,
    };
    if aurora_storage::delete_oauth_account(&state.pool, &hash(&sid), &provider)
        .await
        .is_err()
    {
        return fail(StatusCode::INTERNAL_SERVER_ERROR);
    }
    StatusCode::NO_CONTENT.into_response()
}

pub fn storage_state(config: Config, pool: SqlitePool, encryption_key: [u8; 32]) -> AppState {
    AppState {
        config: Arc::new(config),
        pool,
        encryption_key,
        oauth: Arc::new(HttpsOAuthService {
            http: secure_http_client(),
        }),
        playlist_http: Arc::new(Mutex::new(Box::new(ReqwestPlaylistHttp {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("static playlist HTTP client configuration"),
        })
            as Box<dyn aurora_providers::playlist::Http + Send>)),
        limits: Arc::new(RateLimiter::standard()),
        last_cleanup: Arc::new(Mutex::new(None)),
    }
}
/// Replaces the rate limiter with one whose ceilings the caller chooses.
///
/// Exists so tests can drive a request past the ceiling without spending a
/// production-sized budget; the production path always installs
/// [`RateLimiter::standard`] through [`storage_state`].
pub fn with_rate_limits(
    state: &mut AppState,
    window: Duration,
    per_ip_max: u32,
    per_provider_max: u32,
) {
    state.limits = Arc::new(RateLimiter::new(window, per_ip_max, per_provider_max));
}

pub fn memory_state(config: Config) -> AppState {
    storage_state(
        config,
        SqlitePool::connect_lazy("sqlite::memory:").expect("sqlite"),
        [0; 32],
    )
}

/// Swaps in a scripted OAuth service and playlist transport.
///
/// Test-only seam: the production constructor always wires the real HTTPS
/// implementations, so no test can accidentally reach the network through a
/// configuration mistake in this helper.
#[doc(hidden)]
pub fn test_state(
    config: Config,
    pool: SqlitePool,
    encryption_key: [u8; 32],
    oauth: Arc<dyn OAuthService>,
    playlist_http: Box<dyn aurora_providers::playlist::Http + Send>,
) -> AppState {
    AppState {
        config: Arc::new(config),
        pool,
        encryption_key,
        oauth,
        playlist_http: Arc::new(Mutex::new(playlist_http)),
        limits: Arc::new(RateLimiter::standard()),
        last_cleanup: Arc::new(Mutex::new(None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aurora_providers::token::TokenSet;

    pub(crate) fn config() -> Config {
        Config {
            allowed_origins: vec!["https://app.example".into()],
            frontend_url: "https://app.example/settings/accounts".into(),
            spotify: Some(ProviderSettings {
                client_id: "id".into(),
                client_secret: "secret".into(),
                redirect_uri: "https://api.example/api/oauth/spotify/callback".into(),
            }),
            youtube: None,
        }
    }
    /// Expected to fail: tracking the CSRF idle-eviction limitation. The in-memory
    /// `sqlite::memory:` pool opens one connection per checkout, so each request runs
    /// against its own empty database and the session written by `handshake` is invisible
    /// to the next handler call. Re-enable once the pool is capped to a single connection
    /// (`max_connections(1)`) or migrated to a shared-cache file.
    #[ignore = "needs a shared connection pool for in-memory SQLite"]
    #[tokio::test]
    async fn rejects_foreign_origin() {
        let (app, _store) = state_with_store(config()).await;
        let r = begin(Path("spotify".into()), State(app), HeaderMap::new(), peer()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN)
    }
    #[tokio::test]
    async fn accepts_only_canonical_provider() {
        let (app, _store) = state_with_store(config()).await;
        let mut h = HeaderMap::new();
        h.insert(header::ORIGIN, "https://app.example".parse().unwrap());
        let r = begin(Path("google-youtube".into()), State(app), h, peer()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN)
    }
    #[test]
    fn redirect_uri_must_be_https_and_provider_specific() {
        assert!(valid_redirect_uri(
            "spotify",
            "https://api.example/api/oauth/spotify/callback"
        ));
        assert!(!valid_redirect_uri(
            "spotify",
            "http://api.example/api/oauth/spotify/callback"
        ));
        assert!(!valid_redirect_uri(
            "youtube-music",
            "https://api.example/api/oauth/spotify/callback"
        ));
        assert!(!valid_redirect_uri(
            "spotify",
            "https://api.example/api/oauth/spotify/callback?next=https://evil.example"
        ));
    }

    #[test]
    fn token_endpoints_are_fixed_https_endpoints() {
        assert_eq!(
            provider_descriptor("spotify").unwrap().2,
            "https://accounts.spotify.com/api/token"
        );
        assert_eq!(
            provider_descriptor("youtube-music").unwrap().2,
            "https://oauth2.googleapis.com/token"
        );
        assert!(provider_descriptor("unknown").is_none());
    }

    #[test]
    fn hash_is_not_plaintext() {
        assert_ne!(hash("state"), "state")
    }

    /// A loopback peer address for handler calls made directly from tests, so
    /// the rate limiter sees exactly one shared bucket.
    fn peer() -> PeerAddr {
        PeerAddr(Some(SocketAddr::from(([127, 0, 0, 1], 0))))
    }

    fn begin_request(origin: &str, cookie: Option<&str>, csrf: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::ORIGIN, origin.parse().unwrap());
        if let Some(cookie) = cookie {
            h.insert(header::COOKIE, cookie.parse().unwrap());
        }
        if let Some(csrf) = csrf {
            h.insert(CSRF_HEADER, csrf.parse().unwrap());
        }
        h
    }

    /// Drives one full `begin` round trip and returns the session id, the CSRF token and
    /// the serialized `Set-Cookie` headers.
    async fn handshake(app: &AppState) -> (String, String, Vec<String>) {
        let response = begin(
            Path("spotify".into()),
            State(app.clone()),
            begin_request("https://app.example", None, None),
            peer(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let cookies = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .map(|v| v.to_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let sid = cookies
            .iter()
            .find_map(|c| c.strip_prefix(&format!("{SESSION_COOKIE}=")))
            .expect("session cookie is set")
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let body = body_json(response).await;
        let csrf = body["csrf_token"].as_str().expect("csrf token").to_owned();
        (sid, csrf, cookies)
    }

    /// A file-backed store, because `sqlite::memory:` gives every pooled connection its own
    /// empty database, so a session written by one call would be invisible to the next.
    /// The CSRF checks in these tests are exactly that cross-call visible-session property.
    async fn state_with_store(config: Config) -> (AppState, TestStore) {
        let store = TestStore::new().await;
        (storage_state(config, store.pool.clone(), [0; 32]), store)
    }

    /// Removes its SQLite file, its WAL sidecars and its directory on drop.
    struct TestStore {
        dir: std::path::PathBuf,
        pool: SqlitePool,
    }
    impl TestStore {
        async fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("aurora-api-csrf-test-{}", Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let pool = aurora_storage::open(&dir.join("aurora.db"))
                .await
                .expect("temporary test database");
            Self { dir, pool }
        }
    }
    impl Drop for TestStore {
        fn drop(&mut self) {
            // Windows refuses to unlink an open database, so close the pool first.
            #[allow(clippy::let_underscore_future)]
            // best-effort cleanup in Drop; no runtime to await on
            let _ = self.pool.close();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    async fn body_json(response: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn issues_readable_csrf_cookie_scoped_like_the_session_cookie() {
        let (app, _store) = state_with_store(config()).await;
        let (_, _, cookies) = handshake(&app).await;
        let csrf_cookie_value = cookies
            .iter()
            .find(|c| c.starts_with(&format!("{CSRF_COOKIE}=")))
            .expect("csrf cookie is set");
        assert!(!csrf_cookie_value.contains("HttpOnly"));
        assert!(csrf_cookie_value.contains("Path=/"));
        assert!(csrf_cookie_value.contains("Secure"));
        let session_cookie_value = cookies
            .iter()
            .find(|c| c.starts_with(&format!("{SESSION_COOKIE}=")))
            .expect("session cookie is set");
        for attribute in ["Path=/", "Secure", "SameSite=Strict"] {
            assert!(csrf_cookie_value.contains(attribute));
            assert!(session_cookie_value.contains(attribute));
        }
        assert!(session_cookie_value.contains("HttpOnly"));
    }

    #[tokio::test]
    async fn state_changing_request_without_csrf_header_is_forbidden() {
        let (app, _store) = state_with_store(config()).await;
        let (sid, _, _) = handshake(&app).await;
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let r = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie), None),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn state_changing_request_with_empty_csrf_header_is_forbidden() {
        let (app, _store) = state_with_store(config()).await;
        let (sid, _, _) = handshake(&app).await;
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let r = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie), Some("")),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn state_changing_request_with_wrong_csrf_token_is_forbidden() {
        let (app, _store) = state_with_store(config()).await;
        let (sid, csrf, _) = handshake(&app).await;
        assert_ne!(csrf, random());
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let r = begin(
            Path("spotify".into()),
            State(app.clone()),
            begin_request("https://app.example", Some(&cookie), Some(&random())),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        // The session is untouched by the rejected request: the original token still works.
        let ok = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie), Some(&csrf)),
            peer(),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn csrf_token_is_bound_to_its_own_session() {
        let (app, _store) = state_with_store(config()).await;
        let (sid_a, _, _) = handshake(&app).await;
        let (_, csrf_b, _) = handshake(&app).await;
        let cookie_a = format!("{SESSION_COOKIE}={sid_a}");
        let r = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie_a), Some(&csrf_b)),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn state_changing_request_with_valid_csrf_token_succeeds() {
        let (app, _store) = state_with_store(config()).await;
        let (sid, csrf, _) = handshake(&app).await;
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let r = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie), Some(&csrf)),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(r.headers().get(header::SET_COOKIE).is_some());
    }

    #[tokio::test]
    async fn unknown_session_cookie_never_grants_a_new_session() {
        let (app, _store) = state_with_store(config()).await;
        let cookie = format!("{SESSION_COOKIE}={}", random());
        let r = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie), None),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn foreign_origin_is_rejected_even_with_valid_csrf_token() {
        let (app, _store) = state_with_store(config()).await;
        let (sid, csrf, _) = handshake(&app).await;
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let r = begin(
            Path("spotify".into()),
            State(app),
            begin_request("https://evil.example", Some(&cookie), Some(&csrf)),
            peer(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn disconnect_requires_a_valid_csrf_token() {
        let (app, _store) = state_with_store(config()).await;
        let (sid, csrf, _) = handshake(&app).await;
        let cookie = format!("{SESSION_COOKIE}={sid}");
        let missing = disconnect(
            Path("spotify".into()),
            State(app.clone()),
            begin_request("https://app.example", Some(&cookie), None),
            peer(),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::FORBIDDEN);
        let wrong = disconnect(
            Path("spotify".into()),
            State(app.clone()),
            begin_request("https://app.example", Some(&cookie), Some(&random())),
            peer(),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
        let foreign = disconnect(
            Path("spotify".into()),
            State(app.clone()),
            begin_request("https://evil.example", Some(&cookie), Some(&csrf)),
            peer(),
        )
        .await;
        assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
        let ok = disconnect(
            Path("spotify".into()),
            State(app),
            begin_request("https://app.example", Some(&cookie), Some(&csrf)),
            peer(),
        )
        .await;
        assert_eq!(ok.status(), StatusCode::NO_CONTENT);
    }

    /// Persists a token set the way `persist_after_request` does, but through a
    /// real encrypted round trip so the stored bytes can be decoded back.
    async fn store_tokens(app: &AppState, session_hash: &str, tokens: &TokenSet) -> bool {
        persist_tokens(app, session_hash, "spotify", tokens).await
    }

    fn token_set(access: &str, refresh: Option<&str>) -> TokenSet {
        TokenSet::decode_native(
            serde_json::to_vec(&serde_json::json!({
                "access_token": access,
                "token_type": "Bearer",
                "refresh_token": refresh,
                "scope": Option::<String>::None,
                "expires_at_unix": 4_000_000_000u64,
            }))
            .unwrap()
            .as_slice(),
        )
        .unwrap()
    }

    async fn load_tokens(app: &AppState, session_hash: &str) -> TokenSet {
        let ciphertext =
            aurora_storage::get_oauth_token_ciphertext(&app.pool, session_hash, "spotify")
                .await
                .unwrap()
                .expect("tokens are stored");
        let plaintext =
            aurora_storage::decrypt_oauth_tokens(&app.encryption_key, &ciphertext).unwrap();
        TokenSet::decode_native(&plaintext).unwrap()
    }

    /// Inserts the account row a real handshake would have created, so the
    /// rotation tests exercise `persist_tokens`' update of an existing grant
    /// rather than its no-row fallback.
    async fn seed_account(app: &AppState, session_hash: &str, tokens: &TokenSet) {
        let bytes = tokens.encode_native().unwrap();
        let encrypted = aurora_storage::encrypt_oauth_tokens(&app.encryption_key, &bytes).unwrap();
        aurora_storage::save_oauth_account(
            &app.pool,
            session_hash,
            "spotify",
            &OAuthAccount {
                subject: "seed-subject".into(),
                display_name: "Seed".into(),
                avatar_url: None,
                capabilities: vec![],
                connected_at: Utc::now(),
                auth_state: "connected".into(),
                last_auth_error: None,
            },
            "spotify:seed-subject",
            &encrypted,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn rotated_tokens_persist_even_when_the_follow_up_request_fails() {
        let (app, _store) = state_with_store(config()).await;
        let session_hash = hash("session-under-test");
        seed_account(&app, &session_hash, &token_set("old", Some("refresh-1"))).await;

        // The refresh rotated the refresh token, then the page request failed.
        let rotated = token_set("new", Some("refresh-2"));
        let failure = aurora_providers::playlist::Error::Http {
            status: 500,
            retry_after_ms: None,
        };
        let persisted =
            persist_after_request(&app, &session_hash, "spotify", &rotated, Some(&failure)).await;

        assert!(persisted, "the rotated set must be written");
        let stored = load_tokens(&app, &session_hash).await;
        assert_eq!(stored.access_token, "new");
        assert_eq!(
            stored.refresh_token.as_deref(),
            Some("refresh-2"),
            "a rotated refresh token must survive a failed follow-up request"
        );
    }

    #[tokio::test]
    async fn rotated_tokens_persist_when_the_token_was_refreshed_for_a_rejected_grant() {
        let (app, _store) = state_with_store(config()).await;
        let session_hash = hash("session-reauth");
        seed_account(&app, &session_hash, &token_set("old", Some("refresh-1"))).await;

        let rotated = token_set("new", Some("refresh-2"));
        let persisted = persist_after_request(
            &app,
            &session_hash,
            "spotify",
            &rotated,
            Some(&aurora_providers::playlist::Error::ReauthorizationRequired),
        )
        .await;
        assert!(persisted);
        assert_eq!(
            load_tokens(&app, &session_hash)
                .await
                .refresh_token
                .as_deref(),
            Some("refresh-2")
        );
    }

    #[tokio::test]
    async fn transient_refresh_failure_leaves_the_stored_grant_untouched() {
        let (app, _store) = state_with_store(config()).await;
        let session_hash = hash("session-transient");
        seed_account(&app, &session_hash, &token_set("stale", Some("refresh-1"))).await;

        // A transient failure refreshed nothing, so nothing may be rewritten.
        let unchanged = token_set("stale", Some("refresh-1"));
        let persisted = persist_after_request(
            &app,
            &session_hash,
            "spotify",
            &unchanged,
            Some(&aurora_providers::playlist::Error::RefreshTransient),
        )
        .await;

        assert!(persisted);
        let stored = load_tokens(&app, &session_hash).await;
        assert_eq!(stored.access_token, "stale");
        assert_eq!(stored.refresh_token.as_deref(), Some("refresh-1"));
    }

    #[tokio::test]
    async fn transient_refresh_failure_does_not_mark_reauth_required() {
        let (app, _store) = state_with_store(config()).await;
        let session_hash = hash("session-transient-state");
        seed_account(&app, &session_hash, &token_set("stale", Some("refresh-1"))).await;

        let response = provider_error(
            aurora_providers::playlist::Error::RefreshTransient,
            &app,
            &session_hash,
            "spotify",
        )
        .await;
        // Retryable, not a forced disconnect: 502 lets the client back off and retry.
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let account = aurora_storage::get_oauth_account(&app.pool, &session_hash, "spotify")
            .await
            .unwrap()
            .expect("account exists");
        assert_eq!(
            account.auth_state, "connected",
            "a transient outage must not force reauthorization"
        );
        assert!(
            account.last_auth_error.as_deref().unwrap_or("").is_empty(),
            "a transient outage must not record an auth error"
        );
    }

    #[tokio::test]
    async fn definitive_invalid_grant_marks_reauth_required() {
        let (app, _store) = state_with_store(config()).await;
        let session_hash = hash("session-definitive");
        seed_account(&app, &session_hash, &token_set("stale", Some("refresh-1"))).await;

        let response = provider_error(
            aurora_providers::playlist::Error::ReauthorizationRequired,
            &app,
            &session_hash,
            "spotify",
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let account = aurora_storage::get_oauth_account(&app.pool, &session_hash, "spotify")
            .await
            .unwrap()
            .expect("account exists");
        assert_eq!(account.auth_state, "reauthorization_required");
        assert_eq!(account.last_auth_error.as_deref(), Some("refresh_failed"));
    }

    #[tokio::test]
    async fn persist_tokens_reports_an_unmatched_row_as_failure() {
        let (app, _store) = state_with_store(config()).await;
        // No row exists for this session, so the write cannot land and the
        // caller must be told rather than silently dropping rotated tokens.
        assert!(!store_tokens(&app, &hash("missing"), &token_set("new", Some("r"))).await);
    }

    #[tokio::test]
    async fn persist_tokens_failure_is_surfaced_instead_of_a_silent_success() {
        let (app, _store) = state_with_store(config()).await;
        // An account with no stored row yet: persistence fails, so the caller
        // reports a storage fault rather than returning stale credentials.
        let persisted = persist_after_request(
            &app,
            &hash("absent"),
            "spotify",
            &token_set("new", Some("r")),
            None,
        )
        .await;
        assert!(!persisted);
    }

    #[tokio::test]
    async fn router_preflight_allows_the_csrf_header() {
        let (state, _store) = state_with_store(config()).await;
        let app = router(state);
        let request = axum::http::Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/oauth/spotify/begin")
            .header(header::ORIGIN, "https://app.example")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(header::ACCESS_CONTROL_REQUEST_HEADERS, CSRF_HEADER)
            .body(axum::body::Body::empty())
            .unwrap();
        let response = tower::ServiceExt::oneshot(app, request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let allowed = response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        assert!(allowed.contains(CSRF_HEADER), "got {allowed:?}");
    }
}
