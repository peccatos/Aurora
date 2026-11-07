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
use std::{
    collections::HashMap, future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration,
};
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
    oauth: Arc<dyn OAuthService>,
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
    credentials: HashMap<(String, String), aurora_providers::token::TokenSet>,
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

pub trait OAuthService: Send + Sync {
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
    >;
}
struct HttpsOAuthService {
    http: reqwest::Client,
}
struct ReqwestTokenTransport {
    http: reqwest::Client,
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
            let body = r
                .bytes()
                .await
                .map_err(|_| aurora_providers::token::TransportError::Failed)?;
            Ok(aurora_providers::token::TokenResponse {
                status,
                body: body.to_vec(),
            })
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
            let body = r.bytes().await.map_err(|_| ())?;
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
    let verifier = tx.verifier.clone();
    let redirect_uri = tx.redirect_uri.clone();
    let settings = match provider_settings(&state.config, &provider) {
        Some((s, _, _, _)) if valid_redirect_uri(&provider, &s.redirect_uri) => s.clone(),
        Some(_) => return fail(StatusCode::SERVICE_UNAVAILABLE),
        None => return fail(StatusCode::SERVICE_UNAVAILABLE),
    };
    drop(store);
    if query.error.is_some() {
        return Redirect::to(&state.config.frontend_url).into_response();
    }
    let code = match query.code.filter(|x| !x.is_empty()) {
        Some(x) => x,
        None => return fail(StatusCode::BAD_REQUEST),
    };
    let (account, tokens) = match state
        .oauth
        .complete(&provider, &settings, &code, &verifier, &redirect_uri)
        .await
    {
        Ok(x) => x,
        Err(()) => return fail(StatusCode::BAD_GATEWAY),
    };
    let mut store = state.inner.lock().await;
    store
        .accounts
        .insert((sid.clone(), provider.clone()), account);
    store.credentials.insert((sid, provider), tokens);
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
    store.credentials.remove(&(sid.clone(), provider.clone()));
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
            credentials: HashMap::new(),
        })),
        oauth: Arc::new(HttpsOAuthService {
            http: secure_http_client(),
        }),
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
}
