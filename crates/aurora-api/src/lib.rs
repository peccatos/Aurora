use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{delete, get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;
use url::Url;
use uuid::Uuid;

const SESSION_COOKIE: &str = "__Host-aurora_session";
const TX_TTL: Duration = Duration::from_secs(600);

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    inner: Arc<Mutex<MemoryStore>>,
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
struct MemoryStore {
    sessions: HashMap<String, Session>,
    transactions: HashMap<String, Transaction>,
    accounts: HashMap<(String, String), Account>,
}
struct Session {
    _csrf: String,
    expires: DateTime<Utc>,
}
struct Transaction {
    provider: String,
    session: String,
    verifier: String,
    redirect_uri: String,
    expires: DateTime<Utc>,
    consumed: bool,
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
#[derive(Serialize)]
struct Begin {
    authorization_url: String,
}
#[derive(Deserialize)]
struct Callback {
    state: String,
    code: Option<String>,
    error: Option<String>,
}

pub fn router(state: AppState) -> Router {
    let origins = state
        .config
        .allowed_origins
        .iter()
        .filter_map(|origin| origin.parse::<HeaderValue>().ok())
        .collect::<Vec<_>>();
    Router::new()
        .route("/api/oauth/{provider}/begin", post(begin))
        .route("/api/oauth/{provider}/callback", get(callback))
        .route("/api/oauth/{provider}/status", get(status))
        .route("/api/oauth/{provider}", delete(disconnect))
        .layer(
            CorsLayer::new()
                .allow_origin(origins)
                .allow_methods([Method::GET, Method::POST, Method::DELETE])
                .allow_credentials(true),
        )
        .with_state(state)
}
pub async fn serve(state: AppState, addr: SocketAddr) -> Result<(), std::io::Error> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(state)).await
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
fn session(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix(&format!("{SESSION_COOKIE}=")))
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
    format!("{SESSION_COOKIE}={id}; Path=/; Secure; HttpOnly; SameSite=Lax")
}

async fn begin(
    Path(provider): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str())
        || !origin_ok(&headers, &state.config)
    {
        return fail(StatusCode::FORBIDDEN);
    }
    let supplied_sid = session(&headers);
    let mut store = state.inner.lock().await;
    let (sid, new_session) = match supplied_sid {
        Some(sid) if store.sessions.contains_key(&sid) => (sid, false),
        _ => (random(), true),
    };
    let csrf = random();
    let sess = store.sessions.entry(sid.clone()).or_insert(Session {
        _csrf: csrf,
        expires: Utc::now() + chrono::Duration::hours(8),
    });
    if sess.expires < Utc::now() {
        return fail(StatusCode::UNAUTHORIZED);
    }
    let (settings, endpoint, _, scopes) = match provider_settings(&state.config, &provider) {
        Some(x) => x,
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
    store.transactions.insert(
        hash(&state_value),
        Transaction {
            provider,
            session: sid.clone(),
            verifier,
            redirect_uri: settings.redirect_uri.clone(),
            expires: Utc::now() + chrono::Duration::seconds(TX_TTL.as_secs() as i64),
            consumed: false,
        },
    );
    let mut response = (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        Json(Begin {
            authorization_url: url.into(),
        }),
    )
        .into_response();
    if new_session {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie(&sid).parse().unwrap());
    }
    response
}

async fn callback(
    Path(provider): Path<String>,
    Query(query): Query<Callback>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let sid = match session(&headers) {
        Some(x) => x,
        None => return fail(StatusCode::UNAUTHORIZED),
    };
    let mut store = state.inner.lock().await;
    let tx = match store.transactions.get_mut(&hash(&query.state)) {
        Some(x) => x,
        None => return fail(StatusCode::BAD_REQUEST),
    };
    if tx.consumed || tx.expires < Utc::now() || tx.provider != provider || tx.session != sid {
        return fail(StatusCode::BAD_REQUEST);
    }
    tx.consumed = true;
    if query.error.is_some() {
        return Redirect::to(&state.config.frontend_url).into_response();
    }
    let code = match query.code.filter(|x| !x.is_empty()) {
        Some(x) => x,
        None => return fail(StatusCode::BAD_REQUEST),
    };
    // Token exchange is deliberately behind the native provider boundary. No token material is returned here.
    let _ = (code, tx.verifier.as_str(), tx.redirect_uri.as_str());
    Redirect::to(&state.config.frontend_url).into_response()
}
async fn status(
    Path(provider): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str())
        || !origin_ok(&headers, &state.config)
    {
        return fail(StatusCode::FORBIDDEN);
    }
    let sid = match session(&headers) {
        Some(x) => x,
        None => return fail(StatusCode::UNAUTHORIZED),
    };
    let store = state.inner.lock().await;
    match store.accounts.get(&(sid, provider)) {
        Some(a) => (
            StatusCode::OK,
            [(header::CACHE_CONTROL, "no-store")],
            Json(a),
        )
            .into_response(),
        None => fail(StatusCode::NOT_FOUND),
    }
}
async fn disconnect(
    Path(provider): Path<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !["spotify", "youtube-music"].contains(&provider.as_str())
        || !origin_ok(&headers, &state.config)
    {
        return fail(StatusCode::FORBIDDEN);
    }
    let sid = match session(&headers) {
        Some(x) => x,
        None => return fail(StatusCode::UNAUTHORIZED),
    };
    let mut store = state.inner.lock().await;
    store.accounts.remove(&(sid.clone(), provider.clone()));
    store
        .transactions
        .retain(|_, t| t.session != sid || t.provider != provider || t.consumed);
    StatusCode::NO_CONTENT.into_response()
}

pub fn memory_state(config: Config) -> AppState {
    AppState {
        config: Arc::new(config),
        inner: Arc::new(Mutex::new(MemoryStore {
            sessions: HashMap::new(),
            transactions: HashMap::new(),
            accounts: HashMap::new(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
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
    #[tokio::test]
    async fn rejects_foreign_origin() {
        let app = memory_state(config());
        let r = begin(Path("spotify".into()), State(app), HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN)
    }
    #[tokio::test]
    async fn accepts_only_canonical_provider() {
        let app = memory_state(config());
        let mut h = HeaderMap::new();
        h.insert(header::ORIGIN, "https://app.example".parse().unwrap());
        let r = begin(Path("google-youtube".into()), State(app), h).await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN)
    }
    #[test]
    fn hash_is_not_plaintext() {
        assert_ne!(hash("state"), "state")
    }
}
