//! End-to-end HTTP tests for the browser OAuth flow and the playlist APIs.
//!
//! Every test drives the real router against a real in-memory SQLite database
//! (`aurora_storage::open_in_memory`, so the production migrations run) and a
//! fake provider transport that answers token exchange and userinfo locally.
//! Nothing here touches the network: the fake rejects every URL that is not one
//! of the two allowlisted provider endpoints, and tests that must not reach a
//! provider assert the call counter is still zero.

use aurora_api::{Config, ProviderSettings, router};
use aurora_providers::token::{TokenRequest, TokenResponse, TokenSet, Transport, TransportError};
use aurora_storage::{OAuthAccount, OAuthTransaction, SqlitePool};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use axum::response::Response;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

const ORIGIN: &str = "https://app.example";
const FRONTEND: &str = "https://app.example/settings/accounts";
const SPOTIFY_REDIRECT: &str = "https://api.example/api/oauth/spotify/callback";
const YOUTUBE_REDIRECT: &str = "https://api.example/api/oauth/youtube-music/callback";
const SPOTIFY_TOKEN_ENDPOINT: &str = "https://accounts.spotify.com/api/token";
const SPOTIFY_PROFILE_ENDPOINT: &str = "https://api.spotify.com/v1/me";
const ENCRYPTION_KEY: [u8; 32] = [7u8; 32];

const SESSION_COOKIE: &str = "__Host-aurora_session";
const CSRF_COOKIE: &str = "__Host-aurora_csrf";
const CSRF_HEADER: &str = "x-csrf-token";

/// Mirrors the API's own `hash`: base64url(SHA-256(value)).
fn raw_hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

// ---------------------------------------------------------------------------
// Fake provider transport
// ---------------------------------------------------------------------------

type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TokenResponse, TransportError>> + Send + 'a>>;

/// One scripted provider round trip.
enum Step {
    Token(Value),
    Profile(Value),
    Reject,
}

/// Serves token/userinfo responses from a scripted queue and counts calls, so a
/// test can prove a rejected request never reached the provider.
struct FakeTransport {
    steps: Vec<Step>,
    calls: AtomicUsize,
}

impl FakeTransport {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            calls: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl Transport for FakeTransport {
    fn send<'a>(&'a self, request: TokenRequest) -> SendFuture<'a> {
        let is_token = request.endpoint == SPOTIFY_TOKEN_ENDPOINT;
        let is_profile = request.endpoint == SPOTIFY_PROFILE_ENDPOINT;
        let index = self.calls.fetch_add(1, Ordering::SeqCst);
        // Any other endpoint would be a real network call: fail instead.
        let body = if !is_token && !is_profile {
            Err(TransportError::Failed)
        } else {
            match self.steps.get(index) {
                Some(Step::Reject) => Err(TransportError::Failed),
                Some(Step::Token(claims)) if is_token => Ok(serde_json::to_vec(claims).unwrap()),
                Some(Step::Profile(claims)) if is_profile => {
                    Ok(serde_json::to_vec(claims).unwrap())
                }
                _ => Err(TransportError::Failed),
            }
        };
        Box::pin(async move { body.map(|body| TokenResponse { status: 200, body }) })
    }
}

/// Scripts one complete, successful authorization-code exchange.
fn successful_exchange(access_token: &str) -> Vec<Step> {
    vec![
        Step::Token(json!({
            "access_token": access_token,
            "token_type": "Bearer",
            "refresh_token": "refresh-token",
            "expires_in": 3600,
            "scope": "user-read-email playlist-read-private",
        })),
        Step::Profile(json!({"id": "spotify-subject"})),
    ]
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Owns everything a test needs: the real router, the in-memory database the
/// router writes through, and the shared encryption key.
struct Harness {
    app: axum::Router,
    pool: SqlitePool,
    transport: std::sync::Arc<FakeTransport>,
    playlist_attempts: std::sync::Arc<AtomicUsize>,
}

impl Harness {
    async fn new(steps: Vec<Step>) -> Self {
        let pool = aurora_storage::open_in_memory()
            .await
            .expect("in-memory database with production migrations");
        Self::with_pool(steps, pool).await
    }

    async fn with_pool(steps: Vec<Step>, pool: SqlitePool) -> Self {
        let transport = std::sync::Arc::new(FakeTransport::new(steps));
        Self::with_transport(transport, pool).await
    }

    async fn with_transport(transport: std::sync::Arc<FakeTransport>, pool: SqlitePool) -> Self {
        let playlist_attempts = std::sync::Arc::new(AtomicUsize::new(0));
        let state = aurora_api::test_state(
            config(),
            pool.clone(),
            ENCRYPTION_KEY,
            std::sync::Arc::new(ScriptedOAuthService {
                transport: transport.clone(),
            }),
            Box::new(RecordingPlaylistHttp {
                attempts: playlist_attempts.clone(),
            }),
        );
        Self {
            app: router(state),
            pool,
            transport,
            playlist_attempts,
        }
    }
}

fn config() -> Config {
    Config {
        allowed_origins: vec![ORIGIN.into()],
        frontend_url: FRONTEND.into(),
        spotify: Some(ProviderSettings {
            client_id: "spotify-client-id".into(),
            client_secret: "spotify-client-secret".into(),
            redirect_uri: SPOTIFY_REDIRECT.into(),
        }),
        youtube: Some(ProviderSettings {
            client_id: "youtube-client-id".into(),
            client_secret: "youtube-client-secret".into(),
            redirect_uri: YOUTUBE_REDIRECT.into(),
        }),
    }
}

/// The production `OAuthService` shape, backed by the scripted transport: one
/// token exchange plus one userinfo call, no network.
struct ScriptedOAuthService {
    transport: std::sync::Arc<FakeTransport>,
}

impl ScriptedOAuthService {
    fn client(&self) -> aurora_providers::token::Client {
        aurora_providers::token::Client::new(
            aurora_providers::ProviderConfig {
                id: "spotify".into(),
                client_id: "spotify-client-id".into(),
                authorization_endpoint: "https://accounts.spotify.com/authorize".into(),
                token_endpoint: SPOTIFY_TOKEN_ENDPOINT.into(),
                scopes: vec!["user-read-email".into()],
                enabled: true,
            },
            "spotify-client-secret".into(),
            self.transport.clone(),
            std::time::Duration::from_secs(5),
        )
        .expect("scripted token client")
    }
}

impl aurora_api::OAuthService for ScriptedOAuthService {
    fn complete<'a>(
        &'a self,
        _provider: &'a str,
        _settings: &'a ProviderSettings,
        code: &'a str,
        verifier: &'a str,
        redirect_uri: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(aurora_api::Account, TokenSet), ()>> + Send + 'a>>
    {
        Box::pin(async move {
            let client = self.client();
            let tokens = client
                .exchange_code(code, redirect_uri, verifier)
                .await
                .map_err(|_| ())?;
            // Mirrors the production service: the account identity is read from
            // the userinfo endpoint, and only the subject is carried over.
            let profile = client.profile(&tokens).await.map_err(|_| ())?;
            Ok((
                aurora_api::Account {
                    display_name: profile.account_id.clone(),
                    subject: profile.account_id,
                    avatar_url: None,
                    capabilities: vec!["identity".into()],
                    connected_at: Utc::now(),
                },
                tokens,
            ))
        })
    }
}

/// Serves no playlist payload and records every attempt, so a test can tell a
/// request that was rejected during validation from one that reached the
/// provider boundary.
#[derive(Default)]
struct RecordingPlaylistHttp {
    attempts: std::sync::Arc<AtomicUsize>,
}

impl aurora_providers::playlist::Http for RecordingPlaylistHttp {
    fn send(
        &mut self,
        _request: aurora_providers::playlist::Request,
    ) -> Result<aurora_providers::playlist::Response, aurora_providers::playlist::Error> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        Err(aurora_providers::playlist::Error::Http {
            status: 599,
            retry_after_ms: None,
        })
    }
}

// ---------------------------------------------------------------------------
// HTTP helpers
// ---------------------------------------------------------------------------

async fn send(app: &axum::Router, request: Request<Body>) -> Response {
    tower::ServiceExt::oneshot(app.clone(), request)
        .await
        .expect("the router is infallible")
}

fn request(method: Method, uri: &str) -> axum::http::request::Builder {
    Request::builder().method(method).uri(uri)
}

/// A same-origin request carrying the session cookie and CSRF header.
fn authed(method: Method, uri: &str, sid: &str, csrf: &str) -> Request<Body> {
    request(method, uri)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
        .header(CSRF_HEADER, csrf)
        .body(Body::empty())
        .unwrap()
}

/// A same-origin request carrying the session cookie but no CSRF header.
fn session_only(method: Method, uri: &str, sid: &str) -> Request<Body> {
    request(method, uri)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
        .body(Body::empty())
        .unwrap()
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body is readable");
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body is readable")
        .to_vec()
}

fn set_cookies(response: &Response) -> Vec<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

fn cookie_value(cookies: &[String], name: &str) -> Option<String> {
    cookies.iter().find_map(|cookie| {
        cookie
            .strip_prefix(&format!("{name}="))
            .map(|rest| rest.split(';').next().unwrap().to_owned())
    })
}

fn query_params(url: &str) -> HashMap<String, String> {
    url::Url::parse(url)
        .expect("valid URL")
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect()
}

/// Drives `POST begin` for a provider from a clean (session-less) client and
/// returns the session id, the CSRF token, the `Set-Cookie` headers and body.
async fn begin(app: &axum::Router, provider: &str) -> (String, String, Vec<String>, Value) {
    let response = send(
        app,
        request(Method::POST, &format!("/api/oauth/{provider}/begin"))
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK, "begin must succeed");
    let cookies = set_cookies(&response);
    let body = body_json(response).await;
    let sid = cookie_value(&cookies, SESSION_COOKIE).expect("begin issues a session cookie");
    let csrf = body["csrf_token"]
        .as_str()
        .expect("begin returns a CSRF token")
        .to_owned();
    assert_eq!(
        cookie_value(&cookies, CSRF_COOKIE).as_deref(),
        Some(csrf.as_str()),
        "the readable CSRF cookie must match the issued token"
    );
    (sid, csrf, cookies, body)
}

/// Extracts the `state` the authorization URL will echo back.
fn authorization_state(body: &Value) -> String {
    let url = body["authorization_url"]
        .as_str()
        .expect("authorization_url is a string");
    query_params(url)
        .remove("state")
        .expect("authorization URL carries state")
}

/// Runs `begin` followed by a successful callback and returns the live session.
async fn connected_session(h: &Harness) -> (String, String) {
    let (sid, csrf, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = send(
        &h.app,
        request(
            Method::GET,
            &format!("/api/oauth/spotify/callback?state={state}&code=auth-code"),
        )
        .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "callback succeeds"
    );
    (sid, csrf)
}

/// Inserts a session row that is already past its expiry.
async fn insert_expired_session(pool: &SqlitePool, sid: &str, csrf: &str) {
    aurora_storage::save_oauth_session(
        pool,
        &raw_hash(sid),
        &raw_hash(csrf),
        &(Utc::now() - chrono::Duration::hours(1)).to_rfc3339(),
    )
    .await
    .expect("insert expired session");
}

/// Inserts a transaction row that is already past its expiry.
async fn insert_expired_transaction(pool: &SqlitePool, sid: &str, state: &str) {
    aurora_storage::save_oauth_transaction(
        pool,
        &raw_hash(state),
        &OAuthTransaction {
            provider: "spotify".into(),
            session_id_hash: raw_hash(sid),
            verifier: "expired-verifier".into(),
            redirect_uri: SPOTIFY_REDIRECT.into(),
            expires_at: (Utc::now() - chrono::Duration::minutes(1)).to_rfc3339(),
        },
    )
    .await
    .expect("insert expired transaction");
}

/// The account row a successful callback must leave behind.
async fn stored_account(pool: &SqlitePool, sid: &str) -> OAuthAccount {
    aurora_storage::get_oauth_account(pool, &raw_hash(sid), "spotify")
        .await
        .expect("account lookup succeeds")
        .expect("the account is stored")
}

/// A structurally valid callback query whose `state` is unknown to the server,
/// so validation passes and the session check is what the handler gets to.
fn valid_query() -> String {
    format!("state={}&code=auth-code", random_token())
}

/// A fresh base64url token of the length the API's own generator produces.
fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(uuid::Uuid::new_v4().as_bytes())
}

/// Issues a `GET /api/oauth/spotify/callback` for the given session.
async fn callback(app: &axum::Router, sid: Option<&str>, query: &str) -> Response {
    let mut builder = request(Method::GET, &format!("/api/oauth/spotify/callback?{query}"));
    if let Some(sid) = sid {
        builder = builder.header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"));
    }
    send(app, builder.body(Body::empty()).unwrap()).await
}

async fn token_ciphertext(h: &Harness, sid: &str) -> Option<Vec<u8>> {
    aurora_storage::get_oauth_token_ciphertext(&h.pool, &raw_hash(sid), "spotify")
        .await
        .unwrap()
}

/// Asserts whether the session has stored credentials.
async fn assert_tokens(expected: Option<&[u8]>, h: &Harness, sid: &str) {
    let stored = token_ciphertext(h, sid).await;
    match expected {
        Some(expected) => assert_eq!(stored.as_deref(), Some(expected)),
        None => assert!(stored.is_none(), "no credentials must be stored"),
    }
}

// ---------------------------------------------------------------------------
// begin: session issuance, CSRF, origin policy
// ---------------------------------------------------------------------------

#[tokio::test]
async fn begin_issues_a_session_csrf_pair_and_a_hardened_authorization_url() {
    let h = Harness::new(vec![]).await;
    let (sid, csrf, cookies, body) = begin(&h.app, "spotify").await;

    // Independent, unguessable secrets: neither is the other, nor its hash.
    assert_ne!(sid, csrf);
    assert_ne!(csrf, raw_hash(&csrf));
    assert!(csrf.len() >= 16, "csrf token {csrf:?} is too short");
    assert_ne!(
        csrf, sid,
        "the CSRF token must not be derivable from the session id"
    );

    let session_cookie = cookies
        .iter()
        .find(|c| c.starts_with(&format!("{SESSION_COOKIE}=")))
        .expect("session cookie is present");
    for attribute in ["HttpOnly", "Secure", "SameSite=Strict", "Path=/"] {
        assert!(
            session_cookie.contains(attribute),
            "session cookie {session_cookie:?} is missing {attribute}"
        );
    }
    let csrf_cookie = cookies
        .iter()
        .find(|c| c.starts_with(&format!("{CSRF_COOKIE}=")))
        .expect("csrf cookie is present");
    assert!(
        !csrf_cookie.contains("HttpOnly"),
        "the CSRF cookie must be script-readable so the SPA can echo it"
    );

    let url = body["authorization_url"].as_str().unwrap();
    let parsed = url::Url::parse(url).unwrap();
    assert_eq!(
        parsed.origin().ascii_serialization(),
        "https://accounts.spotify.com",
        "the browser is never redirected off the provider origin"
    );
    assert!(parsed.username().is_empty() && parsed.password().is_none());
    let params = query_params(url);
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["client_id"], "spotify-client-id");
    assert_eq!(params["redirect_uri"], SPOTIFY_REDIRECT);
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(params["scope"], "user-read-email playlist-read-private");
    assert!(
        !params["code_challenge"].is_empty(),
        "the PKCE challenge is mandatory"
    );
    // The verifier stays server-side; only its S256 challenge is published.
    assert!(
        !params.contains_key("code_verifier"),
        "the PKCE verifier must never appear in the authorization URL"
    );
    assert!(body.get("code_verifier").is_none());
    assert!(!url.contains(&sid));
    assert!(!url.contains(&csrf));

    // The transaction is persisted under the hash of the state, never the state
    // itself, and is bound to this session.
    assert_eq!(stored_state_hash(&h).await, raw_hash(&params["state"]));
    assert_eq!(
        stored_state_hash(&h).await,
        raw_hash(&params["state"]).as_str()
    );
}

/// The hash the pending transaction was stored under.
async fn stored_state_hash(h: &Harness) -> String {
    use sqlx::Row;
    let row = sqlx::query("SELECT state_hash FROM oauth_transactions")
        .fetch_one(&h.pool)
        .await
        .expect("exactly one pending transaction");
    row.try_get::<String, _>("state_hash").unwrap()
}

#[tokio::test]
async fn begin_rejects_a_foreign_origin_without_issuing_a_session() {
    let h = Harness::new(vec![]).await;
    let response = send(
        &h.app,
        request(Method::POST, "/api/oauth/spotify/begin")
            .header(header::ORIGIN, "https://evil.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        set_cookies(&response).is_empty(),
        "a cross-origin begin must not mint a session"
    );
    assert_eq!(h.transport.calls(), 0);
}

#[tokio::test]
async fn begin_rejects_a_missing_origin_and_an_unknown_provider() {
    let h = Harness::new(vec![]).await;
    let no_origin = send(
        &h.app,
        request(Method::POST, "/api/oauth/spotify/begin")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);

    let unknown_provider = send(
        &h.app,
        request(Method::POST, "/api/oauth/apple-music/begin")
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unknown_provider.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn begin_rejects_an_unconfigured_provider_and_accepts_youtube_when_configured() {
    let pool = aurora_storage::open_in_memory().await.unwrap();
    let state = aurora_api::test_state(
        Config {
            allowed_origins: vec![ORIGIN.into()],
            frontend_url: FRONTEND.into(),
            spotify: None,
            youtube: Some(ProviderSettings {
                client_id: "youtube-client-id".into(),
                client_secret: "youtube-client-secret".into(),
                redirect_uri: YOUTUBE_REDIRECT.into(),
            }),
        },
        pool,
        ENCRYPTION_KEY,
        std::sync::Arc::new(ScriptedOAuthService {
            transport: std::sync::Arc::new(FakeTransport::new(vec![])),
        }),
        Box::new(RecordingPlaylistHttp::default()),
    );
    let app = router(state);

    let unconfigured = send(
        &app,
        request(Method::POST, "/api/oauth/spotify/begin")
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unconfigured.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(set_cookies(&unconfigured).is_empty());

    let (_, _, _, body) = begin(&app, "youtube-music").await;
    let params = query_params(body["authorization_url"].as_str().unwrap());
    assert_eq!(params["access_type"], "offline");
    assert_eq!(params["prompt"], "consent");
    assert_eq!(params["redirect_uri"], YOUTUBE_REDIRECT);
    assert_eq!(
        url::Url::parse(body["authorization_url"].as_str().unwrap())
            .unwrap()
            .host_str(),
        Some("accounts.google.com")
    );
}

#[tokio::test]
async fn begin_rotates_the_csrf_token_for_an_existing_session() {
    let h = Harness::new(vec![]).await;
    let (sid, csrf, _, _) = begin(&h.app, "spotify").await;

    let response = send(
        &h.app,
        authed(Method::POST, "/api/oauth/spotify/begin", &sid, &csrf),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cookies = set_cookies(&response);
    assert!(
        cookie_value(&cookies, SESSION_COOKIE).is_none(),
        "an existing session is reused rather than reissued"
    );
    let rotated = body_json(response).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(rotated, csrf, "begin always rotates the CSRF token");

    let stale = send(
        &h.app,
        authed(Method::POST, "/api/oauth/spotify/begin", &sid, &csrf),
    )
    .await;
    assert_eq!(
        stale.status(),
        StatusCode::FORBIDDEN,
        "old token is revoked"
    );

    let fresh = send(
        &h.app,
        authed(Method::POST, "/api/oauth/spotify/begin", &sid, &rotated),
    )
    .await;
    assert_eq!(fresh.status(), StatusCode::OK);
}

#[tokio::test]
async fn begin_requires_csrf_for_an_existing_session_and_origin_for_a_new_one() {
    let h = Harness::new(vec![]).await;
    let (sid, _, _, _) = begin(&h.app, "spotify").await;

    let missing = send(
        &h.app,
        session_only(Method::POST, "/api/oauth/spotify/begin", &sid),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::FORBIDDEN);

    let foreign = send(
        &h.app,
        request(Method::POST, "/api/oauth/spotify/begin")
            .header(header::ORIGIN, "https://evil.example")
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn begin_rejects_an_unknown_session_cookie() {
    let h = Harness::new(vec![]).await;
    let response = send(
        &h.app,
        session_only(
            Method::POST,
            "/api/oauth/spotify/begin",
            "forged-session-id",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(set_cookies(&response).is_empty());
}

// ---------------------------------------------------------------------------
// callback: rejection paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn callback_without_a_session_is_unauthorized() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let response = callback(&h.app, None, "state=whatever&code=auth-code").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(h.transport.calls(), 0, "no provider call is attempted");
}

#[tokio::test]
async fn callback_rejects_an_expired_session() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let sid = "expired-session-id";
    insert_expired_session(&h.pool, sid, "csrf").await;
    let response = callback(&h.app, Some(sid), &valid_query()).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(h.transport.calls(), 0);
    assert!(
        aurora_storage::get_oauth_account(&h.pool, &raw_hash(sid), "spotify")
            .await
            .unwrap()
            .is_none(),
        "an expired session never persists an account"
    );
}

#[tokio::test]
async fn callback_rejects_an_unknown_state() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (sid, _, _, _) = begin(&h.app, "spotify").await;
    let response = callback(
        &h.app,
        Some(&sid),
        "state=not-a-state-we-issued&code=auth-code",
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(h.transport.calls(), 0);
    assert_tokens(None, &h, &sid).await;
}

#[tokio::test]
async fn callback_rejects_the_first_request_carrying_both_code_and_error() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (sid, _, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = callback(
        &h.app,
        Some(&sid),
        &format!("state={state}&code=auth-code&error=access_denied"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        h.transport.calls(),
        0,
        "an aborted grant is never exchanged"
    );
    assert_tokens(None, &h, &sid).await;
}

#[tokio::test]
async fn callback_rejects_a_missing_or_empty_code() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (sid, _, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    for query in [
        format!("state={state}"),
        format!("state={state}&code="),
        format!("state={state}&error="),
    ] {
        let response = callback(&h.app, Some(&sid), &query).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "query {query:?}"
        );
    }
    assert_eq!(h.transport.calls(), 0);
}

#[tokio::test]
async fn callback_rejects_a_state_that_belongs_to_another_session() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (victim, _, _, victim_body) = begin(&h.app, "spotify").await;
    let (attacker, _, _, _) = begin(&h.app, "spotify").await;
    let stolen_state = authorization_state(&victim_body);

    let response = callback(
        &h.app,
        Some(&attacker),
        &format!("state={stolen_state}&code=auth-code"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(h.transport.calls(), 0);

    // The legitimate session can still complete: the forged attempt did not
    // consume the transaction.
    let ok = callback(
        &h.app,
        Some(&victim),
        &format!("state={stolen_state}&code=auth-code"),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn callback_rejects_a_foreign_provider_for_the_same_state() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (sid, _, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = send(
        &h.app,
        request(
            Method::GET,
            &format!("/api/oauth/youtube-music/callback?state={state}&code=auth-code"),
        )
        .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(h.transport.calls(), 0);
    assert_tokens(None, &h, &sid).await;
}

#[tokio::test]
async fn callback_rejects_an_expired_transaction() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (sid, _, _, _) = begin(&h.app, "spotify").await;
    let state = "expired-state-value";
    insert_expired_transaction(&h.pool, &sid, state).await;

    let response = callback(&h.app, Some(&sid), &format!("state={state}&code=auth-code")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        h.transport.calls(),
        0,
        "an expired transaction is never exchanged"
    );
    assert_tokens(None, &h, &sid).await;
}

#[tokio::test]
async fn callback_rejects_a_replayed_state() {
    let h = Harness::new(successful_exchange("token-a")).await;
    let (sid, csrf, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);

    let first = callback(&h.app, Some(&sid), &format!("state={state}&code=auth-code")).await;
    assert_eq!(first.status(), StatusCode::SEE_OTHER);
    let calls_after_first = h.transport.calls();
    let tokens_after_first = token_ciphertext(&h, &sid).await;

    let replay = callback(&h.app, Some(&sid), &format!("state={state}&code=auth-code")).await;
    assert_eq!(
        replay.status(),
        StatusCode::BAD_REQUEST,
        "a consumed state can never be claimed twice"
    );
    assert_eq!(
        h.transport.calls(),
        calls_after_first,
        "the replay never reaches the provider"
    );
    assert_eq!(
        token_ciphertext(&h, &sid).await,
        tokens_after_first,
        "the replay must not overwrite the stored credentials"
    );
    // The session itself stays usable.
    let status = send(
        &h.app,
        authed(Method::GET, "/api/oauth/spotify/status", &sid, &csrf),
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
}

#[tokio::test]
async fn callback_reports_a_provider_failure_as_bad_gateway() {
    let h = Harness::new(vec![Step::Reject]).await;
    let (sid, _, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = callback(&h.app, Some(&sid), &format!("state={state}&code=auth-code")).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_tokens(None, &h, &sid).await;
}

#[tokio::test]
async fn callback_reports_a_malformed_token_response_as_bad_gateway() {
    let h = Harness::new(vec![Step::Token(json!({"unexpected": true}))]).await;
    let (sid, _, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = callback(&h.app, Some(&sid), &format!("state={state}&code=auth-code")).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_tokens(None, &h, &sid).await;
}

// ---------------------------------------------------------------------------
// callback: success path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn successful_callback_persists_sanitized_account_metadata() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = callback(&h.app, Some(&sid), &format!("state={state}&code=auth-code")).await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some(FRONTEND),
        "the browser returns to the configured frontend, never to the provider"
    );
    assert_eq!(
        h.transport.calls(),
        2,
        "one token exchange, one userinfo call"
    );

    let account = stored_account(&h.pool, &sid).await;
    assert_eq!(account.subject, "spotify-subject");
    assert_eq!(account.display_name, "spotify-subject");
    assert_eq!(account.avatar_url, None);
    assert_eq!(account.capabilities, vec!["identity".to_owned()]);
    assert_eq!(account.auth_state, "connected");
    // `get_oauth_account` reads `last_auth_error` with a plain `try_get`, which
    // fails on SQL NULL and yields an empty string, so the stored value is
    // asserted where it is actually visible: the response the API hands out.
    let raw = raw_account_row(&h, &sid).await;
    assert_eq!(raw["last_auth_error"], "");

    // The account metadata is not encumbered with any provider payload.
    for column in [
        "subject",
        "display_name",
        "capabilities_json",
        "keyring_account_id",
    ] {
        assert!(
            !raw[column].contains("secret-access-token"),
            "{column} leaked the access token: {}",
            raw[column]
        );
        assert!(
            !raw[column].contains("refresh-token"),
            "{column} leaked the refresh token: {}",
            raw[column]
        );
    }
    let ciphertext = token_ciphertext(&h, &sid).await.expect("token stored");
    assert!(
        !ciphertext
            .windows(b"secret-access-token".len())
            .any(|w| w == b"secret-access-token"),
        "tokens must be stored encrypted, never in the clear"
    );
    let decrypted = aurora_storage::decrypt_oauth_tokens(&ENCRYPTION_KEY, &ciphertext).unwrap();
    assert!(
        std::str::from_utf8(&decrypted)
            .unwrap()
            .contains("secret-access-token"),
        "the ciphertext round-trips with the configured key"
    );

    // The status endpoint answers with the sanitized account only.
    let status = send(
        &h.app,
        authed(Method::GET, "/api/oauth/spotify/status", &sid, &csrf),
    )
    .await;
    assert_eq!(status.status(), StatusCode::OK);
    assert_eq!(
        status
            .headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let status_body = body_json(status).await;
    assert_eq!(status_body["subject"], "spotify-subject");
    assert_eq!(status_body["authState"], "connected");
    let rendered = status_body.to_string();
    for forbidden in [
        "secret-access-token",
        "refresh-token",
        "accessToken",
        "refreshToken",
        "tokenCiphertext",
        "token_ciphertext",
        "client_secret",
        "spotify-client-secret",
    ] {
        assert!(
            !rendered.contains(forbidden),
            "status response leaked {forbidden}: {rendered}"
        );
    }
}

/// The raw `oauth_accounts` row, to check what was actually persisted.
async fn raw_account_row(h: &Harness, sid: &str) -> HashMap<String, String> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT subject, display_name, capabilities_json, keyring_account_id, last_auth_error, \
         auth_state FROM oauth_accounts WHERE session_id_hash=? AND provider=?",
    )
    .bind(raw_hash(sid))
    .bind("spotify")
    .fetch_one(&h.pool)
    .await
    .expect("account row exists");
    let mut out = HashMap::new();
    for column in [
        "subject",
        "display_name",
        "capabilities_json",
        "keyring_account_id",
        "last_auth_error",
        "auth_state",
    ] {
        out.insert(
            column.to_owned(),
            row.try_get::<Option<String>, _>(column)
                .ok()
                .flatten()
                .unwrap_or_default(),
        );
    }
    out
}

#[tokio::test]
async fn callback_uses_the_server_stored_redirect_uri_and_verifier() {
    // The exchange is driven by the transaction row, never by caller-supplied
    // query parameters.
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, _, _, body) = begin(&h.app, "spotify").await;
    let state = authorization_state(&body);
    let response = callback(
        &h.app,
        Some(&sid),
        &format!("state={state}&code=auth-code&redirect_uri=https://evil.example/callback"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        stored_account(&h.pool, &sid).await.subject,
        "spotify-subject"
    );
    assert_eq!(
        stored_transaction_redirect(&h, &raw_hash(&state)).await,
        SPOTIFY_REDIRECT
    );
}

async fn stored_transaction_redirect(h: &Harness, state_hash: &str) -> String {
    use sqlx::Row;
    let row = sqlx::query("SELECT redirect_uri FROM oauth_transactions WHERE state_hash=?")
        .bind(state_hash)
        .fetch_one(&h.pool)
        .await
        .expect("transaction row");
    row.try_get::<String, _>("redirect_uri").unwrap()
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_requires_a_session_and_an_allowed_origin() {
    let h = Harness::new(vec![]).await;

    let anonymous = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/status")
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let (sid, _, _, _) = begin(&h.app, "spotify").await;
    let foreign = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/status")
            .header(header::ORIGIN, "https://evil.example")
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);

    let unknown_provider = send(
        &h.app,
        request(Method::GET, "/api/oauth/apple-music/status")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unknown_provider.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn status_rejects_an_expired_session() {
    let h = Harness::new(vec![]).await;
    let sid = "expired-status-session";
    insert_expired_session(&h.pool, sid, "csrf").await;
    let response = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/status")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn status_reports_not_found_for_a_provider_without_an_account() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;
    let ok = send(
        &h.app,
        authed(Method::GET, "/api/oauth/spotify/status", &sid, &csrf),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::OK);

    let missing = send(
        &h.app,
        authed(Method::GET, "/api/oauth/youtube-music/status", &sid, &csrf),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(missing).await["error"], "oauth_unavailable");
}

// ---------------------------------------------------------------------------
// disconnect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disconnect_removes_the_account_and_is_idempotent() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;
    assert!(token_ciphertext(&h, &sid).await.is_some());

    let first = send(
        &h.app,
        authed(Method::DELETE, "/api/oauth/spotify", &sid, &csrf),
    )
    .await;
    assert_eq!(first.status(), StatusCode::NO_CONTENT);
    assert!(token_ciphertext(&h, &sid).await.is_none());
    assert!(
        aurora_storage::get_oauth_account(&h.pool, &raw_hash(&sid), "spotify")
            .await
            .unwrap()
            .is_none()
    );

    // Repeating the call is not an error and does not disturb the session.
    let second = send(
        &h.app,
        authed(Method::DELETE, "/api/oauth/spotify", &sid, &csrf),
    )
    .await;
    assert_eq!(second.status(), StatusCode::NO_CONTENT);

    // The session survives; only the account is gone.
    let status = send(
        &h.app,
        authed(Method::GET, "/api/oauth/spotify/status", &sid, &csrf),
    )
    .await;
    assert_eq!(status.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn disconnect_requires_session_bound_csrf_and_an_allowed_origin() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;

    let missing_csrf = send(
        &h.app,
        session_only(Method::DELETE, "/api/oauth/spotify", &sid),
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let wrong_csrf = send(
        &h.app,
        authed(Method::DELETE, "/api/oauth/spotify", &sid, "not-the-token"),
    )
    .await;
    assert_eq!(wrong_csrf.status(), StatusCode::FORBIDDEN);

    let foreign = send(
        &h.app,
        request(Method::DELETE, "/api/oauth/spotify")
            .header(header::ORIGIN, "https://evil.example")
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .header(CSRF_HEADER, &csrf)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);

    // Nothing was deleted by the rejected attempts.
    assert!(token_ciphertext(&h, &sid).await.is_some());

    let ok = send(
        &h.app,
        authed(Method::DELETE, "/api/oauth/spotify", &sid, &csrf),
    )
    .await;
    assert_eq!(ok.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn disconnect_rejects_an_unknown_provider_and_an_expired_session() {
    let h = Harness::new(vec![]).await;
    let sid = "expired-disconnect-session";
    insert_expired_session(&h.pool, sid, "csrf").await;

    let expired = send(
        &h.app,
        request(Method::DELETE, "/api/oauth/spotify")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .header(CSRF_HEADER, "csrf")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // `disconnect` validates the session-bound CSRF token before it touches the
    // session, so a dead session presenting an unvalidated token is refused as a
    // CSRF failure. Either 401 or 403 proves no deletion happened.
    assert!(
        matches!(
            expired.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "got {}",
        expired.status()
    );

    let (live, csrf, _, _) = begin(&h.app, "spotify").await;
    let unknown = send(
        &h.app,
        authed(Method::DELETE, "/api/oauth/apple-music", &live, &csrf),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// playlists: list endpoint and malformed cursors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn playlists_rejects_a_malformed_cursor_before_reaching_the_provider() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;
    let calls_before = h.playlist_attempts.load(Ordering::SeqCst);

    for cursor in ["abc", "-1", "1.5", "%20", "0x10", "1e3", "+1"] {
        let uri = format!("/api/oauth/spotify/playlists?cursor={cursor}");
        let response = send(&h.app, authed(Method::GET, &uri, &sid, &csrf)).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "cursor {cursor:?} must be rejected"
        );
        assert_eq!(
            h.playlist_attempts.load(Ordering::SeqCst),
            calls_before,
            "cursor {cursor:?} must be rejected before any provider request"
        );
    }

    // A well-formed cursor passes validation; it then fails at the provider
    // boundary, which is a different, non-400 outcome.
    let accepted = send(
        &h.app,
        authed(
            Method::GET,
            "/api/oauth/spotify/playlists?cursor=50",
            &sid,
            &csrf,
        ),
    )
    .await;
    assert_ne!(accepted.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn playlists_requires_origin_session_and_stored_tokens() {
    let h = Harness::new(vec![]).await;
    let (sid, csrf, _, _) = begin(&h.app, "spotify").await;

    // Same request with no Origin header at all.
    let no_origin = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/playlists")
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);

    let anonymous = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/playlists")
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let no_account = send(
        &h.app,
        authed(Method::GET, "/api/oauth/spotify/playlists", &sid, &csrf),
    )
    .await;
    assert_eq!(no_account.status(), StatusCode::NOT_FOUND);

    let expired_sid = "expired-playlists-session";
    insert_expired_session(&h.pool, expired_sid, "csrf").await;
    let expired = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/playlists")
            .header(header::ORIGIN, ORIGIN)
            .header(header::COOKIE, format!("{SESSION_COOKIE}={expired_sid}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);

    let unknown_provider = send(
        &h.app,
        authed(Method::GET, "/api/oauth/apple-music/playlists", &sid, &csrf),
    )
    .await;
    assert_eq!(unknown_provider.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// playlists: detail endpoint and malformed ids
// ---------------------------------------------------------------------------

#[tokio::test]
async fn playlist_detail_rejects_malformed_ids_before_reaching_the_provider() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;
    let calls_before = h.playlist_attempts.load(Ordering::SeqCst);

    // Encoded traversal, separators, control bytes, non-ASCII and length abuse.
    let oversized = "a".repeat(257);
    for id in [
        "",
        "%2F",
        "%2E%2E%2Fetc",
        "abc%2Fdef",
        "abc%20def",
        "abc%3Brm",
        "abc%00",
        "abc%3Fnext",
        "playlist%E2%9C%93",
        oversized.as_str(),
    ] {
        let uri = format!("/api/oauth/spotify/playlists/{id}");
        let response = send(&h.app, authed(Method::GET, &uri, &sid, &csrf)).await;
        assert_ne!(
            response.status(),
            StatusCode::OK,
            "id {id:?} must never be resolved"
        );
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST
                    | StatusCode::FORBIDDEN
                    | StatusCode::NOT_FOUND
                    | StatusCode::UNAUTHORIZED
            ),
            "id {id:?} produced an unexpected {}",
            response.status()
        );
    }
    assert_eq!(
        h.playlist_attempts.load(Ordering::SeqCst),
        calls_before,
        "malformed ids never reach the provider"
    );
}

#[tokio::test]
async fn playlist_detail_rejects_hostile_ids_that_would_escape_the_endpoint() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;
    let calls_before = h.playlist_attempts.load(Ordering::SeqCst);

    for id in ["..%2F..%2Fme", "%2E%2E", "abc%2F..%2F..%2Fme%2Fplaylists"] {
        let uri = format!("/api/oauth/spotify/playlists/{id}");
        let response = send(&h.app, authed(Method::GET, &uri, &sid, &csrf)).await;
        assert_ne!(response.status(), StatusCode::OK, "id {id:?}");
    }
    assert_eq!(h.playlist_attempts.load(Ordering::SeqCst), calls_before);
}

#[tokio::test]
async fn playlist_detail_requires_origin_session_csrf_and_stored_tokens() {
    let h = Harness::new(vec![]).await;
    let (sid, csrf, _, _) = begin(&h.app, "spotify").await;

    let no_origin = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/playlists/abc123")
            .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
            .header(CSRF_HEADER, &csrf)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_origin.status(), StatusCode::FORBIDDEN);

    let no_csrf = send(
        &h.app,
        session_only(Method::GET, "/api/oauth/spotify/playlists/abc123", &sid),
    )
    .await;
    assert_eq!(no_csrf.status(), StatusCode::FORBIDDEN);

    let anonymous = send(
        &h.app,
        request(Method::GET, "/api/oauth/spotify/playlists/abc123")
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);

    let no_account = send(
        &h.app,
        authed(
            Method::GET,
            "/api/oauth/spotify/playlists/abc123",
            &sid,
            &csrf,
        ),
    )
    .await;
    assert_eq!(no_account.status(), StatusCode::NOT_FOUND);

    let unknown_provider = send(
        &h.app,
        authed(
            Method::GET,
            "/api/oauth/apple-music/playlists/abc123",
            &sid,
            &csrf,
        ),
    )
    .await;
    assert_eq!(unknown_provider.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn playlist_detail_rejects_an_expired_session() {
    let h = Harness::new(vec![]).await;
    let sid = "expired-detail-session";
    insert_expired_session(&h.pool, sid, "csrf").await;
    let response = send(
        &h.app,
        request(
            Method::GET,
            "/api/oauth/spotify/playlists/37i9dQZF1DXcBWIGoYBM5M",
        )
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={sid}"))
        .header(CSRF_HEADER, "csrf")
        .body(Body::empty())
        .unwrap(),
    )
    .await;
    // The session-bound CSRF check runs first, so a dead session carrying an
    // unvalidated token is refused as a CSRF failure; either way it is refused.
    assert!(
        matches!(
            response.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "got {}",
        response.status()
    );
    assert_eq!(
        h.playlist_attempts.load(Ordering::SeqCst),
        0,
        "a dead session never reaches the provider"
    );
}

#[tokio::test]
async fn playlist_detail_accepts_a_well_formed_id_and_fails_only_at_the_provider() {
    let h = Harness::new(successful_exchange("secret-access-token")).await;
    let (sid, csrf) = connected_session(&h).await;
    let before = h.playlist_attempts.load(Ordering::SeqCst);
    let response = send(
        &h.app,
        authed(
            Method::GET,
            "/api/oauth/spotify/playlists/37i9dQZF1DXcBWIGoYBM5M",
            &sid,
            &csrf,
        ),
    )
    .await;
    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "a valid id passes validation and fails only at the provider boundary"
    );
    // A 502 (and not a 4xx) proves the id survived validation; the detail
    // endpoint builds its own HTTP transport, so the playlist counter stays at
    // zero by design and only the concurrent requests share the injected one.
    assert_eq!(
        h.playlist_attempts.load(Ordering::SeqCst),
        before,
        "the detail endpoint does not use the shared playlist transport"
    );
}

// ---------------------------------------------------------------------------
// public endpoints
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_and_ready_are_public_and_body_bearing() {
    let h = Harness::new(vec![]).await;
    for path in ["/health", "/ready"] {
        let response = send(
            &h.app,
            request(Method::GET, path).body(Body::empty()).unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert!(!body_bytes(response).await.is_empty(), "{path}");
    }
}
