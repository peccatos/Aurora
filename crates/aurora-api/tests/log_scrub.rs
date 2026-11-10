//! Log-scrub regression tests: the OAuth happy path must never write the
//! state, the authorization code, the CSRF token, the PKCE verifier, or any
//! provider token into tracing output.
//!
//! The allowed surface is deliberately narrow: provider name, booleans, the
//! 8-character session-hash prefix and the account subject. Anything else a
//! future change wants to log must be added here consciously, never by
//! weakening these assertions.

use aurora_api::{Config, ProviderSettings, router};
use aurora_providers::token::{TokenRequest, TokenResponse, TokenSet, Transport, TransportError};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

const ORIGIN: &str = "https://app.example";
const FRONTEND: &str = "https://app.example/settings/accounts";
const SPOTIFY_REDIRECT: &str = "https://api.example/api/oauth/spotify/callback";
const TOKEN_ENDPOINT: &str = "https://accounts.spotify.com/api/token";
const PROFILE_ENDPOINT: &str = "https://api.spotify.com/v1/me";
const ENCRYPTION_KEY: [u8; 32] = [7u8; 32];
const ACCESS_TOKEN: &str = "secret-access-token";
const AUTH_CODE: &str = "auth-code-12345";

// ---------------------------------------------------------------------------
// Log capture: a MakeWriter that collects every formatted line in memory.
// ---------------------------------------------------------------------------

type Sink = Arc<Mutex<Vec<u8>>>;

#[derive(Clone)]
struct CapturingWriter {
    sink: Sink,
}

impl std::io::Write for CapturingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.sink
            .lock()
            .expect("sink not poisoned")
            .extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturingWriter {
    type Writer = CapturingWriter;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// Fake provider transport (same shape as the oauth_playlist_flow harness).
// ---------------------------------------------------------------------------

type SendFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TokenResponse, TransportError>> + Send + 'a>>;

struct FakeTransport;

impl Transport for FakeTransport {
    fn send<'a>(&'a self, request: TokenRequest) -> SendFuture<'a> {
        let body = if request.endpoint == TOKEN_ENDPOINT {
            Ok(serde_json::to_vec(&json!({
                "access_token": ACCESS_TOKEN,
                "token_type": "Bearer",
                "refresh_token": "refresh-token-6789",
                "expires_in": 3600,
                "scope": "user-read-email playlist-read-private",
            }))
            .unwrap())
        } else if request.endpoint == PROFILE_ENDPOINT {
            Ok(serde_json::to_vec(&json!({"id": "spotify-subject"})).unwrap())
        } else {
            Err(TransportError::Failed)
        };
        Box::pin(async move { body.map(|body| TokenResponse { status: 200, body }) })
    }
}

struct ScriptedOAuthService;

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
            let client = aurora_providers::token::Client::new(
                aurora_providers::ProviderConfig {
                    id: "spotify".into(),
                    client_id: "spotify-client-id".into(),
                    authorization_endpoint: "https://accounts.spotify.com/authorize".into(),
                    token_endpoint: TOKEN_ENDPOINT.into(),
                    scopes: vec!["user-read-email".into()],
                    enabled: true,
                },
                "spotify-client-secret".into(),
                Arc::new(FakeTransport),
                std::time::Duration::from_secs(5),
            )
            .expect("scripted token client");
            let tokens = client
                .exchange_code(code, redirect_uri, verifier)
                .await
                .map_err(|_| ())?;
            let profile = client.profile(&tokens).await.map_err(|_| ())?;
            Ok((
                aurora_api::Account {
                    display_name: profile.account_id.clone(),
                    subject: profile.account_id,
                    avatar_url: None,
                    capabilities: vec!["identity".into()],
                    connected_at: chrono::Utc::now(),
                },
                tokens,
            ))
        })
    }
}

#[derive(Default)]
struct RecordingPlaylistHttp;

impl aurora_providers::playlist::Http for RecordingPlaylistHttp {
    fn send(
        &mut self,
        _request: aurora_providers::playlist::Request,
    ) -> Result<aurora_providers::playlist::Response, aurora_providers::playlist::Error> {
        Err(aurora_providers::playlist::Error::Http {
            status: 599,
            retry_after_ms: None,
        })
    }
}

fn config() -> Config {
    Config {
        allowed_origins: vec![ORIGIN.into()],
        registration_open: true,
        frontend_url: FRONTEND.into(),
        spotify: Some(ProviderSettings {
            client_id: "spotify-client-id".into(),
            client_secret: "spotify-client-secret".into(),
            redirect_uri: SPOTIFY_REDIRECT.into(),
        }),
        youtube: Some(ProviderSettings {
            client_id: "youtube-client-id".into(),
            client_secret: "youtube-client-secret".into(),
            redirect_uri: "https://api.example/api/oauth/youtube-music/callback".into(),
        }),
    }
}

fn raw_hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(value.as_bytes()))
}

// ---------------------------------------------------------------------------
// The happy path, with every secret captured.
// ---------------------------------------------------------------------------

struct Flow {
    state: String,
    csrf: String,
    verifier: String,
    sid: String,
    uid: String,
}

async fn drive(app: &axum::Router, pool: &aurora_storage::SqlitePool) -> Flow {
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        Request::builder()
            .method(Method::POST)
            .uri("/api/oauth/spotify/begin")
            .header(header::ORIGIN, ORIGIN)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .expect("router responds");
    assert_eq!(response.status(), StatusCode::OK, "begin must succeed");
    let cookies: Vec<String> = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_owned())
        .collect();
    let cookie = |name: &str| {
        cookies
            .iter()
            .find_map(|c| c.strip_prefix(&format!("{name}=")))
            .map(|rest| rest.split(';').next().unwrap().to_owned())
            .unwrap_or_else(|| panic!("{name} cookie is set"))
    };
    let sid = cookie("__Host-aurora_session");
    let uid = cookie("__Host-aurora_user");
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body readable");
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    let csrf = body["csrf_token"].as_str().expect("csrf token").to_owned();
    let auth_url = body["authorization_url"].as_str().expect("auth url");
    let mut params: std::collections::HashMap<String, String> = url::Url::parse(auth_url)
        .expect("valid url")
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let state = params.remove("state").expect("state param");

    // The verifier as it sits at rest before the callback consumes it.
    let (verifier,): (String,) =
        sqlx::query_as("SELECT verifier FROM oauth_transactions WHERE state_hash=?")
            .bind(raw_hash(&state))
            .fetch_one(pool)
            .await
            .expect("pending transaction row");
    assert!(!verifier.is_empty(), "the verifier was stored");

    let cookie_hdr = format!("__Host-aurora_session={sid}; __Host-aurora_user={uid}");
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        Request::builder()
            .method(Method::GET)
            .uri(format!(
                "/api/oauth/spotify/callback?state={state}&code={AUTH_CODE}"
            ))
            .header(header::COOKIE, cookie_hdr)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .expect("router responds");
    assert_eq!(
        response.status(),
        StatusCode::SEE_OTHER,
        "callback succeeds"
    );

    Flow {
        state,
        csrf,
        verifier,
        sid,
        uid,
    }
}

#[tokio::test]
async fn oauth_happy_path_logs_no_credentials() {
    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let writer = CapturingWriter { sink: sink.clone() };
    let subscriber = tracing_subscriber::fmt()
        .with_writer(writer)
        .with_max_level(tracing::Level::DEBUG)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("no global subscriber is installed before this test");

    let pool = aurora_storage::open_in_memory()
        .await
        .expect("in-memory database");
    let state = aurora_api::test_state(
        config(),
        pool.clone(),
        ENCRYPTION_KEY,
        Arc::new(ScriptedOAuthService),
        Box::new(RecordingPlaylistHttp),
    );
    let app = router(state);
    let flow = drive(&app, &pool).await;

    let captured = String::from_utf8(sink.lock().expect("sink not poisoned").clone())
        .expect("log output is UTF-8");
    assert!(!captured.is_empty(), "the flow must emit some log lines");

    for (what, secret) in [
        ("state", flow.state.as_str()),
        ("authorization code", AUTH_CODE),
        ("CSRF token", flow.csrf.as_str()),
        ("PKCE verifier", flow.verifier.as_str()),
        ("access token", ACCESS_TOKEN),
        ("refresh token", "refresh-token-6789"),
        ("session cookie value", flow.sid.as_str()),
        ("user cookie value", flow.uid.as_str()),
        ("client secret", "spotify-client-secret"),
    ] {
        assert!(
            !captured.contains(secret),
            "the captured logs leaked the {what}: {captured}"
        );
    }

    // What IS allowed stays visible, so the assertions above can never pass
    // merely because logging was silently disabled.
    assert!(
        captured.contains("spotify-subject"),
        "the subject is allowed in the logs: {captured}"
    );
    assert!(
        captured.contains(&raw_hash(&flow.sid)[..8]),
        "the 8-char session hash prefix is allowed: {captured}"
    );

    // The verifier is also scrubbed at rest once the transaction is consumed.
    let (verifier,): (String,) =
        sqlx::query_as("SELECT verifier FROM oauth_transactions WHERE state_hash=?")
            .bind(raw_hash(&flow.state))
            .fetch_one(&pool)
            .await
            .expect("consumed transaction row is retained");
    assert_eq!(verifier, "", "verifier must be erased after the claim");
}
