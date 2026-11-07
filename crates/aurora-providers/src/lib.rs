//! Desktop-only provider OAuth boundary. Secrets remain native and sessions ephemeral.
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Mutex,
    time,
};
use url::Url;
use uuid::Uuid;

fn allowed_scope(provider: &str, scope: &str) -> bool {
    match provider {
        "spotify" => matches!(
            scope,
            "user-read-email"
                | "playlist-read-private"
                | "playlist-modify-public"
                | "playlist-modify-private"
        ),
        "youtube-music" => matches!(
            scope,
            "openid"
                | "email"
                | "profile"
                | "https://www.googleapis.com/auth/userinfo.profile"
                | "https://www.googleapis.com/auth/youtube"
                | "https://www.googleapis.com/auth/youtube.readonly"
        ),
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    pub id: String,
    pub client_id: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub scopes: Vec<String>,
    pub enabled: bool,
}
impl ProviderConfig {
    pub fn validate(&self) -> Result<(), ProviderError> {
        if !self.enabled || self.client_id.trim().is_empty() {
            return Err(ProviderError::NotConfigured);
        }
        if !matches!(self.id.as_str(), "spotify" | "youtube-music") {
            return Err(ProviderError::InvalidConfiguration);
        }
        for endpoint in [&self.authorization_endpoint, &self.token_endpoint] {
            let u = Url::parse(endpoint).map_err(|_| ProviderError::InvalidConfiguration)?;
            if u.scheme() != "https" || u.host_str().is_none() {
                return Err(ProviderError::InvalidConfiguration);
            }
        }
        if self.scopes.is_empty()
            || self
                .scopes
                .iter()
                .any(|s| s.is_empty() || s.chars().any(char::is_whitespace))
            || self
                .scopes
                .iter()
                .any(|scope| !allowed_scope(&self.id, scope))
        {
            return Err(ProviderError::InvalidConfiguration);
        }
        Ok(())
    }
}
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("provider is not configured")]
    NotConfigured,
    #[error("invalid provider configuration")]
    InvalidConfiguration,
    #[error("callback expired")]
    CallbackExpired,
    #[error("invalid callback")]
    InvalidCallback,
    #[error("invalid OAuth state")]
    InvalidState,
    #[error("callback listener failed")]
    Listener,
    #[error("OAuth token exchange failed")]
    Exchange,
    #[error("OAuth profile lookup failed")]
    Profile,
    #[error("credential storage failed")]
    Storage,
    #[error("authorization denied: {0}")]
    Denied(String),
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthorizationStart {
    pub authorization_url: String,
    pub redirect_uri: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallbackResult {
    Code { code: String, verifier: String },
    Denied(String),
}

#[derive(Debug, Clone)]
struct Pending {
    provider: String,
    state: String,
    redirect_uri: String,
    verifier: String,
    expires_at: std::time::Instant,
}
#[derive(Clone)]
pub struct Coordinator {
    sessions: Arc<Mutex<HashMap<String, Pending>>>,
    completed: Arc<Mutex<HashMap<String, (String, CallbackResult)>>>,
}
impl Default for Coordinator {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            completed: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}
impl Coordinator {
    pub async fn begin_authorization(
        &self,
        config: ProviderConfig,
        timeout: Duration,
    ) -> Result<AuthorizationStart, ProviderError> {
        config.validate()?;
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|_| ProviderError::Listener)?;
        let port = listener
            .local_addr()
            .map_err(|_| ProviderError::Listener)?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}/oauth/callback/{}", config.id);
        let state = Uuid::new_v4().to_string();
        let verifier = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = Url::parse(&config.authorization_endpoint)
            .map_err(|_| ProviderError::InvalidConfiguration)?;
        url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_type", "code")
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256")
            .append_pair("scope", &config.scopes.join(" "));
        self.sessions.lock().await.insert(
            state.clone(),
            Pending {
                provider: config.id,
                state: state.clone(),
                redirect_uri: redirect_uri.clone(),
                verifier,
                expires_at: std::time::Instant::now() + timeout,
            },
        );
        let coordinator = self.clone();
        tokio::spawn(async move {
            let _ = receive_callback(listener, coordinator, timeout).await;
        });
        Ok(AuthorizationStart {
            authorization_url: url.to_string(),
            redirect_uri,
        })
    }
    pub async fn complete_callback(
        &self,
        provider: &str,
        state: &str,
        code: Option<&str>,
        error: Option<&str>,
        redirect_uri: &str,
    ) -> Result<CallbackResult, ProviderError> {
        let p = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(state)
                .cloned()
                .ok_or(ProviderError::InvalidState)?
        };
        if p.provider != provider || p.state != state || p.redirect_uri != redirect_uri {
            return Err(ProviderError::InvalidState);
        }
        if p.expires_at <= std::time::Instant::now() {
            self.sessions.lock().await.remove(state);
            return Err(ProviderError::CallbackExpired);
        }
        if let Some(error) = error.filter(|value| !value.is_empty()) {
            self.sessions.lock().await.remove(state);
            return Ok(CallbackResult::Denied(error.to_owned()));
        }
        let code = code
            .filter(|value| !value.is_empty())
            .ok_or(ProviderError::InvalidCallback)?;
        self.sessions.lock().await.remove(state);
        Ok(CallbackResult::Code {
            code: code.to_owned(),
            verifier: p.verifier,
        })
    }

    /// Completes OAuth without exposing credentials. The pending session is consumed only
    /// after token exchange, profile lookup, and storage all succeed. A failed exchange or
    /// storage operation leaves it available for a corrected retry.
    #[allow(clippy::too_many_arguments)]
    pub async fn complete_with_exchange(
        &self,
        provider: &str,
        state: &str,
        code: Option<&str>,
        error: Option<&str>,
        redirect_uri: &str,
        client: &token::Client,
        store: &mut dyn credentials::Store,
    ) -> Result<credentials::CredentialMetadata, ProviderError> {
        if client.provider_id() != provider {
            return Err(ProviderError::InvalidState);
        }
        let pending = {
            let sessions = self.sessions.lock().await;
            sessions
                .get(state)
                .cloned()
                .ok_or(ProviderError::InvalidState)?
        };
        if pending.provider != provider
            || pending.state != state
            || pending.redirect_uri != redirect_uri
        {
            return Err(ProviderError::InvalidState);
        }
        if pending.expires_at <= std::time::Instant::now() {
            self.sessions.lock().await.remove(state);
            return Err(ProviderError::CallbackExpired);
        }
        if let Some(reason) = error.filter(|value| !value.is_empty()) {
            self.sessions.lock().await.remove(state);
            return Err(ProviderError::Denied(reason.to_owned()));
        }
        let code = code
            .filter(|value| !value.is_empty())
            .ok_or(ProviderError::InvalidCallback)?;
        let tokens = client
            .exchange_code(code, redirect_uri, &pending.verifier)
            .await
            .map_err(|_| ProviderError::Exchange)?;
        let profile = client
            .profile(&tokens)
            .await
            .map_err(|_| ProviderError::Profile)?;
        let metadata = store
            .put(provider, &profile.account_id, &tokens.access_token)
            .map_err(|_| ProviderError::Storage)?;
        self.sessions.lock().await.remove(state);
        Ok(metadata)
    }

    pub async fn peek_completed(&self, provider: &str) -> Option<(String, CallbackResult)> {
        self.completed.lock().await.get(provider).cloned()
    }

    pub async fn clear_completed(&self, provider: &str) {
        self.completed.lock().await.remove(provider);
    }

    pub async fn disconnect(&self, provider: &str) {
        self.sessions
            .lock()
            .await
            .retain(|_, pending| pending.provider != provider);
    }
}
async fn receive_callback(
    listener: TcpListener,
    coordinator: Coordinator,
    timeout: Duration,
) -> Result<(), ProviderError> {
    let (mut stream, _) = time::timeout(timeout, listener.accept())
        .await
        .map_err(|_| ProviderError::CallbackExpired)?
        .map_err(|_| ProviderError::Listener)?;
    let mut buf = [0u8; 8192];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|_| ProviderError::Listener)?;
    let request = std::str::from_utf8(&buf[..n]).map_err(|_| ProviderError::InvalidCallback)?;
    let target = request
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("GET "))
        .and_then(|line| line.split_once(" HTTP/").map(|parts| parts.0))
        .ok_or(ProviderError::InvalidCallback)?;
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| ProviderError::InvalidCallback)?;
    let parts: Vec<_> = url.path().split('/').collect();
    if parts.len() != 4 || parts[1] != "oauth" || parts[2] != "callback" {
        return Err(ProviderError::InvalidCallback);
    }
    let query = |key: &str| {
        url.query_pairs()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.into_owned())
    };
    let state = query("state").ok_or(ProviderError::InvalidCallback)?;
    let provider = parts[3].to_owned();
    let redirect_uri = format!("http://127.0.0.1{}", url.path());
    let result = coordinator
        .complete_callback(
            &provider,
            &state,
            query("code").as_deref(),
            query("error").as_deref(),
            &redirect_uri,
        )
        .await;
    if let Ok(ref completed) = result {
        coordinator
            .completed
            .lock()
            .await
            .insert(provider, (redirect_uri, completed.clone()));
    }
    let ok = result.is_ok();
    let body = if ok {
        "Authorization received. You may close this window."
    } else {
        "Authorization failed. You may close this window."
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|_| ProviderError::Listener)?;
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> ProviderConfig {
        ProviderConfig {
            id: "spotify".into(),
            client_id: "x".into(),
            authorization_endpoint: "https://accounts.spotify.com/authorize".into(),
            token_endpoint: "https://accounts.spotify.com/api/token".into(),
            scopes: vec!["user-read-email".into()],
            enabled: true,
        }
    }
    #[test]
    fn rejects_http() {
        let mut c = config();
        c.authorization_endpoint = "http://x.test".into();
        assert!(matches!(
            c.validate(),
            Err(ProviderError::InvalidConfiguration)
        ))
    }
    #[test]
    fn rejects_unknown_scope_and_provider() {
        let mut c = config();
        c.scopes = vec!["not-allowlisted".into()];
        assert_eq!(c.validate(), Err(ProviderError::InvalidConfiguration));
        c.scopes = vec!["user-read-email".into()];
        c.id = "unknown".into();
        assert_eq!(c.validate(), Err(ProviderError::InvalidConfiguration));
    }

    #[tokio::test]
    async fn callback_error_is_single_use_and_disconnect_revokes_pending_state() {
        let c = Coordinator::default();
        let first = c
            .begin_authorization(config(), Duration::from_secs(2))
            .await
            .unwrap();
        let state = Url::parse(&first.authorization_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            c.complete_callback(
                "spotify",
                &state,
                None,
                Some("access_denied"),
                &first.redirect_uri
            )
            .await,
            Ok(CallbackResult::Denied("access_denied".into()))
        );

        let second = c
            .begin_authorization(config(), Duration::from_secs(2))
            .await
            .unwrap();
        let state = Url::parse(&second.authorization_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        c.disconnect("spotify").await;
        assert_eq!(
            c.complete_callback("spotify", &state, Some("code"), None, &second.redirect_uri)
                .await,
            Err(ProviderError::InvalidState)
        );
    }

    #[tokio::test]
    async fn expired_callback_is_rejected_and_removed() {
        let c = Coordinator::default();
        let start = c
            .begin_authorization(config(), Duration::ZERO)
            .await
            .unwrap();
        let state = Url::parse(&start.authorization_url)
            .unwrap()
            .query_pairs()
            .find(|(key, _)| key == "state")
            .unwrap()
            .1
            .into_owned();
        assert_eq!(
            c.complete_callback("spotify", &state, Some("code"), None, &start.redirect_uri)
                .await,
            Err(ProviderError::CallbackExpired)
        );
        assert_eq!(
            c.complete_callback("spotify", &state, Some("code"), None, &start.redirect_uri)
                .await,
            Err(ProviderError::InvalidState)
        );
    }

    #[tokio::test]
    async fn state_single_use() {
        let c = Coordinator::default();
        let s = c
            .begin_authorization(config(), Duration::from_secs(2))
            .await
            .unwrap();
        let u = Url::parse(&s.authorization_url).unwrap();
        let state = u
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        assert!(
            c.complete_callback("spotify", &state, Some("code"), None, &s.redirect_uri)
                .await
                .map(|result| matches!(result, CallbackResult::Code { code, .. } if code == "code"))
                .unwrap_or(false)
        );
        assert!(matches!(
            c.complete_callback("spotify", &state, Some("code"), None, &s.redirect_uri)
                .await,
            Err(ProviderError::InvalidState)
        ))
    }
}

/// Explicit composition registry for configured native adapters.
#[derive(Clone)]
pub struct ProviderRegistry {
    configs: Arc<HashMap<String, ProviderConfig>>,
}
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProviderStatus {
    pub provider: String,
    pub enabled: bool,
    pub configured: bool,
    pub capabilities: Vec<String>,
}
impl ProviderRegistry {
    pub fn from_environment() -> Self {
        let configs = [
            (
                "spotify",
                "AURORA_SPOTIFY_CLIENT_ID",
                "AURORA_SPOTIFY_ENABLED",
                "AURORA_SPOTIFY_SCOPES",
                "user-read-email playlist-read-private playlist-modify-public playlist-modify-private",
                "https://accounts.spotify.com/authorize",
                "https://accounts.spotify.com/api/token",
            ),
            (
                "youtube-music",
                "AURORA_YOUTUBE_MUSIC_CLIENT_ID",
                "AURORA_YOUTUBE_MUSIC_ENABLED",
                "AURORA_YOUTUBE_MUSIC_SCOPES",
                "openid email profile https://www.googleapis.com/auth/youtube",
                "https://accounts.google.com/o/oauth2/v2/auth",
                "https://oauth2.googleapis.com/token",
            ),
        ]
        .into_iter()
        .map(|(id, client_env, enabled_env, scopes_env, default_scopes, authorization_endpoint, token_endpoint)| {
            let client_id = std::env::var(client_env).unwrap_or_default();
            let enabled = std::env::var(enabled_env)
                .map(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "no" | "off"))
                .unwrap_or(!client_id.trim().is_empty());
            let scopes = std::env::var(scopes_env)
                .unwrap_or_else(|_| default_scopes.to_owned())
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            (
                id.to_owned(),
                ProviderConfig {
                    id: id.to_owned(),
                    client_id: client_id.clone(),
                    authorization_endpoint: authorization_endpoint.to_owned(),
                    token_endpoint: token_endpoint.to_owned(),
                    scopes,
                    enabled: enabled && !client_id.trim().is_empty(),
                },
            )
        })
        .collect();
        Self {
            configs: Arc::new(configs),
        }
    }
    pub fn config(&self, provider: &str) -> Option<ProviderConfig> {
        self.configs.get(provider).cloned()
    }

    pub fn statuses(&self) -> Vec<ProviderStatus> {
        self.configs
            .values()
            .map(|c| ProviderStatus {
                provider: c.id.clone(),
                enabled: c.enabled,
                configured: c.validate().is_ok(),
                capabilities: vec!["identity".into()],
            })
            .collect()
    }
    pub fn status(&self, provider: &str) -> Result<ProviderStatus, ProviderError> {
        let c = self
            .configs
            .get(provider)
            .ok_or(ProviderError::NotConfigured)?;
        Ok(ProviderStatus {
            provider: c.id.clone(),
            enabled: c.enabled,
            configured: c.validate().is_ok(),
            capabilities: vec!["identity".into()],
        })
    }
    pub async fn begin(
        &self,
        provider: &str,
        coordinator: &Coordinator,
    ) -> Result<AuthorizationStart, ProviderError> {
        let config = self
            .configs
            .get(provider)
            .cloned()
            .ok_or(ProviderError::NotConfigured)?;
        coordinator
            .begin_authorization(config, Duration::from_secs(600))
            .await
    }
}

/// Mockable, provider-neutral OAuth token transport. The concrete HTTPS implementation
/// belongs to the native process; this boundary deliberately accepts no arbitrary endpoint.
pub mod token {
    use super::ProviderConfig;
    use serde::Deserialize;
    use std::{
        future::Future,
        pin::Pin,
        sync::Arc,
        time::{Duration, Instant},
    };
    use thiserror::Error;
    use tokio::time;
    use url::Url;

    pub struct TokenRequest {
        pub endpoint: String,
        pub form: Vec<(String, String)>,
    }
    pub struct TokenResponse {
        pub status: u16,
        pub body: Vec<u8>,
    }
    pub trait Transport: Send + Sync {
        fn send<'a>(
            &'a self,
            request: TokenRequest,
        ) -> Pin<Box<dyn Future<Output = Result<TokenResponse, TransportError>> + Send + 'a>>;
    }
    #[derive(Debug, Error, Clone, PartialEq, Eq)]
    pub enum TransportError {
        #[error("token transport timed out")]
        Timeout,
        #[error("token transport failed")]
        Failed,
    }
    #[derive(Debug, Error, Clone, PartialEq, Eq)]
    pub enum Error {
        #[error("provider is not configured")]
        NotConfigured,
        #[error("token endpoint is not allowlisted")]
        EndpointNotAllowed,
        #[error("token transport timed out")]
        Timeout,
        #[error("token transport failed")]
        Transport,
        #[error("token endpoint returned HTTP {0}")]
        Http(u16),
        #[error("invalid token response")]
        InvalidResponse,
        #[error("missing refresh token")]
        MissingRefreshToken,
    }
    #[derive(Debug, Deserialize)]
    struct WireProfile {
        id: Option<String>,
        sub: Option<String>,
    }
    pub struct Profile {
        pub account_id: String,
    }
    #[derive(Debug, Deserialize)]
    struct WireTokenSet {
        access_token: String,
        token_type: Option<String>,
        expires_in: Option<u64>,
        refresh_token: Option<String>,
        scope: Option<String>,
    }
    /// Credentials are intentionally not Debug/Serialize: callers must keep them native-only.
    pub struct TokenSet {
        pub access_token: String,
        pub token_type: String,
        pub refresh_token: Option<String>,
        pub scope: Option<String>,
        expires_at: Instant,
    }
    impl TokenSet {
        pub fn is_expired(&self) -> bool {
            self.expires_at <= Instant::now()
        }
        pub fn expires_at(&self) -> Instant {
            self.expires_at
        }

        /// Encodes the token set for the native credential boundary. The
        /// resulting bytes are opaque to callers and must never cross IPC.
        pub fn encode_native(&self) -> Result<Vec<u8>, serde_json::Error> {
            #[derive(serde::Serialize)]
            struct Stored<'a> {
                access_token: &'a str,
                token_type: &'a str,
                refresh_token: Option<&'a str>,
                scope: Option<&'a str>,
                expires_at_unix: u64,
            }
            let expires_at_unix = self
                .expires_at
                .saturating_duration_since(Instant::now())
                .as_secs()
                + std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
            serde_json::to_vec(&Stored {
                access_token: &self.access_token,
                token_type: &self.token_type,
                refresh_token: self.refresh_token.as_deref(),
                scope: self.scope.as_deref(),
                expires_at_unix,
            })
        }

        /// Decodes bytes previously produced by `encode_native`.
        pub fn decode_native(bytes: &[u8]) -> Result<Self, serde_json::Error> {
            #[derive(serde::Deserialize)]
            struct Stored {
                access_token: String,
                token_type: String,
                refresh_token: Option<String>,
                scope: Option<String>,
                expires_at_unix: u64,
            }
            let stored: Stored = serde_json::from_slice(bytes)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let lifetime = Duration::from_secs(stored.expires_at_unix.saturating_sub(now));
            Ok(Self {
                access_token: stored.access_token,
                token_type: stored.token_type,
                refresh_token: stored.refresh_token,
                scope: stored.scope,
                expires_at: Instant::now() + lifetime,
            })
        }
    }

    pub struct Client {
        config: ProviderConfig,
        client_secret: String,
        transport: Arc<dyn Transport>,
        timeout: Duration,
    }
    impl Client {
        pub fn new(
            config: ProviderConfig,
            client_secret: String,
            transport: Arc<dyn Transport>,
            timeout: Duration,
        ) -> Result<Self, Error> {
            config.validate().map_err(|_| Error::NotConfigured)?;
            if !allowlisted_endpoint(&config.id, &config.token_endpoint) {
                return Err(Error::EndpointNotAllowed);
            }
            Ok(Self {
                config,
                client_secret,
                transport,
                timeout,
            })
        }

        pub fn provider_id(&self) -> &str {
            &self.config.id
        }

        pub async fn exchange_code(
            &self,
            code: &str,
            redirect_uri: &str,
            code_verifier: &str,
        ) -> Result<TokenSet, Error> {
            self.call(vec![
                ("grant_type".into(), "authorization_code".into()),
                ("code".into(), code.into()),
                ("redirect_uri".into(), redirect_uri.into()),
                ("code_verifier".into(), code_verifier.into()),
            ])
            .await
        }

        pub async fn refresh(&self, refresh_token: &str) -> Result<TokenSet, Error> {
            if refresh_token.is_empty() {
                return Err(Error::MissingRefreshToken);
            }
            let mut refreshed = self
                .call(vec![
                    ("grant_type".into(), "refresh_token".into()),
                    ("refresh_token".into(), refresh_token.into()),
                ])
                .await?;
            // OAuth providers commonly omit refresh_token on rotation-free refreshes.
            // Preserve the native secret so a later 401 can still be recovered once.
            if refreshed.refresh_token.is_none() {
                refreshed.refresh_token = Some(refresh_token.to_owned());
            }
            Ok(refreshed)
        }

        pub async fn profile(&self, tokens: &TokenSet) -> Result<Profile, Error> {
            let endpoint = match self.config.id.as_str() {
                "spotify" => "https://api.spotify.com/v1/me",
                "youtube-music" => "https://www.googleapis.com/oauth2/v3/userinfo",
                _ => return Err(Error::NotConfigured),
            };
            let response = time::timeout(
                self.timeout,
                self.transport.send(TokenRequest {
                    endpoint: endpoint.into(),
                    form: vec![("access_token".into(), tokens.access_token.clone())],
                }),
            )
            .await
            .map_err(|_| Error::Timeout)?
            .map_err(|_| Error::Transport)?;
            if !(200..300).contains(&response.status) {
                return Err(Error::Http(response.status));
            }
            let wire: WireProfile =
                serde_json::from_slice(&response.body).map_err(|_| Error::InvalidResponse)?;
            let account_id = wire
                .id
                .or(wire.sub)
                .filter(|id| !id.trim().is_empty())
                .ok_or(Error::InvalidResponse)?;
            Ok(Profile { account_id })
        }

        /// Refreshes an expired set at most once. A failed refresh is returned directly and
        /// never recursively retried, preventing refresh storms and duplicate side effects.
        pub async fn access_token(&self, current: TokenSet) -> Result<TokenSet, Error> {
            if !current.is_expired() {
                return Ok(current);
            }
            let refresh = current.refresh_token.ok_or(Error::MissingRefreshToken)?;
            self.refresh(&refresh).await
        }

        /// Calls the userinfo endpoint with one refresh/retry budget. Expiry is
        /// refreshed before the first call; a single HTTP 401 refreshes and
        /// retries once. The returned token set is authoritative and may have
        /// rotated credentials.
        pub async fn profile_with_refresh(
            &self,
            mut tokens: TokenSet,
        ) -> Result<(Profile, TokenSet), Error> {
            if tokens.is_expired() {
                let refresh = tokens
                    .refresh_token
                    .as_deref()
                    .ok_or(Error::MissingRefreshToken)?;
                tokens = self.refresh(refresh).await?;
            }
            match self.profile(&tokens).await {
                Ok(profile) => Ok((profile, tokens)),
                Err(Error::Http(401)) => {
                    let refresh = tokens
                        .refresh_token
                        .as_deref()
                        .ok_or(Error::MissingRefreshToken)?;
                    let refreshed = self.refresh(refresh).await?;
                    let profile = self.profile(&refreshed).await?;
                    Ok((profile, refreshed))
                }
                Err(error) => Err(error),
            }
        }

        async fn call(&self, mut fields: Vec<(String, String)>) -> Result<TokenSet, Error> {
            fields.push(("client_id".into(), self.config.client_id.clone()));
            if !self.client_secret.is_empty() {
                fields.push(("client_secret".into(), self.client_secret.clone()));
            }
            let request = TokenRequest {
                endpoint: self.config.token_endpoint.clone(),
                form: fields,
            };
            let response = time::timeout(self.timeout, self.transport.send(request))
                .await
                .map_err(|_| Error::Timeout)?
                .map_err(|e| match e {
                    TransportError::Timeout => Error::Timeout,
                    TransportError::Failed => Error::Transport,
                })?;
            if !(200..300).contains(&response.status) {
                return Err(Error::Http(response.status));
            }
            let parsed: WireTokenSet =
                serde_json::from_slice(&response.body).map_err(|_| Error::InvalidResponse)?;
            if parsed.access_token.is_empty() {
                return Err(Error::InvalidResponse);
            }
            let lifetime = Duration::from_secs(parsed.expires_in.unwrap_or(3600));
            Ok(TokenSet {
                access_token: parsed.access_token,
                token_type: parsed.token_type.unwrap_or_else(|| "Bearer".into()),
                refresh_token: parsed.refresh_token,
                scope: parsed.scope,
                expires_at: Instant::now() + lifetime,
            })
        }
    }

    fn allowlisted_endpoint(provider: &str, endpoint: &str) -> bool {
        let expected = match provider {
            "spotify" => "https://accounts.spotify.com/api/token",
            "youtube-music" => "https://oauth2.googleapis.com/token",
            _ => return false,
        };
        let parsed = match Url::parse(endpoint) {
            Ok(value) => value,
            Err(_) => return false,
        };
        endpoint == expected && parsed.scheme() == "https" && parsed.host_str().is_some()
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::{Arc, Mutex};
        struct Fake {
            responses: Mutex<Vec<Result<TokenResponse, TransportError>>>,
        }
        impl Transport for Fake {
            fn send<'a>(
                &'a self,
                _: TokenRequest,
            ) -> Pin<Box<dyn Future<Output = Result<TokenResponse, TransportError>> + Send + 'a>>
            {
                let result = self.responses.lock().unwrap().remove(0);
                Box::pin(async move { result })
            }
        }
        fn config() -> ProviderConfig {
            ProviderConfig {
                id: "spotify".into(),
                client_id: "id".into(),
                authorization_endpoint: "https://accounts.spotify.com/authorize".into(),
                token_endpoint: "https://accounts.spotify.com/api/token".into(),
                scopes: vec!["user-read-email".into()],
                enabled: true,
            }
        }
        fn response(body: &str) -> Result<TokenResponse, TransportError> {
            Ok(TokenResponse {
                status: 200,
                body: body.into(),
            })
        }
        #[tokio::test]
        async fn exchanges_and_refreshes_once() {
            let fake = Arc::new(Fake {
                responses: Mutex::new(vec![
                    response(r#"{"access_token":"a","refresh_token":"r","expires_in":0}"#),
                    response(r#"{"access_token":"b","refresh_token":"r2","expires_in":3600}"#),
                ]),
            });
            let client = Client::new(
                config(),
                "secret".into(),
                fake.clone(),
                Duration::from_secs(1),
            )
            .unwrap();
            let first = client
                .exchange_code("code", "http://127.0.0.1/callback", "verifier")
                .await
                .unwrap();
            let second = client.access_token(first).await.unwrap();
            assert_eq!(second.access_token, "b");
            assert!(fake.responses.lock().unwrap().is_empty());
        }
        #[tokio::test]
        async fn missing_refresh_token_never_calls_transport() {
            let fake = Arc::new(Fake {
                responses: Mutex::new(vec![response(r#"{"access_token":"a","expires_in":0}"#)]),
            });
            let client = Client::new(
                config(),
                String::new(),
                fake.clone(),
                Duration::from_secs(1),
            )
            .unwrap();
            let token = client
                .exchange_code("code", "redirect", "verifier")
                .await
                .unwrap();
            assert!(matches!(
                client.access_token(token).await,
                Err(Error::MissingRefreshToken)
            ));
            assert!(fake.responses.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn refresh_preserves_unrotated_refresh_token() {
            let fake = Arc::new(Fake {
                responses: Mutex::new(vec![response(
                    r#"{"access_token":"new","expires_in":3600}"#,
                )]),
            });
            let client =
                Client::new(config(), String::new(), fake, Duration::from_secs(1)).unwrap();
            let refreshed = client.refresh("old").await.unwrap();
            assert_eq!(refreshed.refresh_token.as_deref(), Some("old"));
        }

        #[tokio::test]
        async fn expired_profile_refreshes_and_persists_rotated_tokens() {
            let fake = Arc::new(Fake {
                responses: Mutex::new(vec![
                    response(r#"{"access_token":"expired","refresh_token":"old","expires_in":0}"#),
                    response(
                        r#"{"access_token":"fresh","refresh_token":"rotated","expires_in":3600}"#,
                    ),
                    response(r#"{"id":"account"}"#),
                ]),
            });
            let client = Client::new(
                config(),
                String::new(),
                fake.clone(),
                Duration::from_secs(1),
            )
            .unwrap();
            let initial = client
                .exchange_code("code", "redirect", "verifier")
                .await
                .unwrap();
            let (profile, tokens) = client.profile_with_refresh(initial).await.unwrap();
            assert_eq!(profile.account_id, "account");
            assert_eq!(tokens.access_token, "fresh");
            assert_eq!(tokens.refresh_token.as_deref(), Some("rotated"));
            assert!(fake.responses.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn one_401_refreshes_and_retries_once() {
            let fake = Arc::new(Fake {
                responses: Mutex::new(vec![
                    response(
                        r#"{"access_token":"access","refresh_token":"old","expires_in":3600}"#,
                    ),
                    Ok(TokenResponse {
                        status: 401,
                        body: b"ignored".to_vec(),
                    }),
                    response(
                        r#"{"access_token":"fresh","refresh_token":"rotated","expires_in":3600}"#,
                    ),
                    response(r#"{"sub":"account"}"#),
                ]),
            });
            let client = Client::new(
                config(),
                String::new(),
                fake.clone(),
                Duration::from_secs(1),
            )
            .unwrap();
            let initial = client
                .exchange_code("code", "redirect", "verifier")
                .await
                .unwrap();
            let (profile, tokens) = client.profile_with_refresh(initial).await.unwrap();
            assert_eq!(profile.account_id, "account");
            assert_eq!(tokens.refresh_token.as_deref(), Some("rotated"));
            assert!(fake.responses.lock().unwrap().is_empty());
        }

        #[tokio::test]
        async fn status_and_timeout_are_redacted() {
            let fake = Arc::new(Fake {
                responses: Mutex::new(vec![Ok(TokenResponse {
                    status: 401,
                    body: b"secret body".to_vec(),
                })]),
            });
            let client = Client::new(
                config(),
                "client-secret".into(),
                fake,
                Duration::from_secs(1),
            )
            .unwrap();
            let error = match client.exchange_code("code", "redirect", "verifier").await {
                Err(error) => error,
                Ok(_) => panic!("expected HTTP error"),
            };
            assert_eq!(error, Error::Http(401));
            assert!(!error.to_string().contains("secret"));
            assert!(!error.to_string().contains("client-secret"));
        }
    }
}

/// Normalized provider identity metadata. Tokens and authorization material never cross this boundary.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OAuthAccountMetadata {
    pub provider: String,
    pub subject: String,
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub capabilities: Vec<String>,
    pub connected_at: String,
}

/// Mockable native transport for provider profile/userinfo requests.
pub mod profile {
    use super::OAuthAccountMetadata;
    use serde::Deserialize;
    use thiserror::Error;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Request {
        pub method: String,
        pub url: String,
        pub headers: Vec<(String, String)>,
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Response {
        pub status: u16,
        pub body: Vec<u8>,
    }
    pub trait Transport {
        fn send(&mut self, request: Request) -> Result<Response, TransportError>;
    }
    #[derive(Debug, Error, Clone, PartialEq, Eq)]
    pub enum TransportError {
        #[error("profile transport failed")]
        Failed,
    }
    #[derive(Debug, Error, Clone, PartialEq, Eq)]
    pub enum Error {
        #[error("unsupported provider")]
        UnsupportedProvider,
        #[error("profile endpoint returned HTTP {0}")]
        Http(u16),
        #[error("profile transport failed")]
        Transport,
        #[error("invalid profile response")]
        InvalidResponse,
        #[error("profile subject is missing")]
        MissingSubject,
    }

    #[derive(Debug, Deserialize)]
    struct SpotifyProfile {
        id: String,
        display_name: Option<String>,
        images: Option<Vec<Image>>,
    }
    #[derive(Debug, Deserialize)]
    struct GoogleProfile {
        sub: String,
        name: Option<String>,
        email: Option<String>,
        picture: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct Image {
        url: String,
    }

    fn safe_text(value: Option<String>, fallback: &str) -> String {
        let value = value
            .unwrap_or_default()
            .trim()
            .chars()
            .take(200)
            .collect::<String>();
        if value.is_empty() {
            fallback.to_owned()
        } else {
            value
        }
    }
    fn safe_url(value: Option<String>) -> Option<String> {
        let value = value?.trim().to_owned();
        let parsed = url::Url::parse(&value).ok()?;
        (parsed.scheme() == "https" && parsed.host_str().is_some()).then_some(value)
    }

    /// Fetches and normalizes the current-user response for Spotify or Google.
    pub fn current_user<T: Transport>(
        transport: &mut T,
        provider: &str,
        access_token: &str,
        connected_at: String,
    ) -> Result<OAuthAccountMetadata, Error> {
        let url = match provider {
            "spotify" => "https://api.spotify.com/v1/me",
            "youtube-music" => "https://openidconnect.googleapis.com/v1/userinfo",
            _ => return Err(Error::UnsupportedProvider),
        };
        let response = transport
            .send(Request {
                method: "GET".into(),
                url: url.into(),
                headers: vec![("Authorization".into(), format!("Bearer {access_token}"))],
            })
            .map_err(|_| Error::Transport)?;
        if !(200..300).contains(&response.status) {
            return Err(Error::Http(response.status));
        }
        let mut metadata = match provider {
            "spotify" => {
                let p: SpotifyProfile =
                    serde_json::from_slice(&response.body).map_err(|_| Error::InvalidResponse)?;
                if p.id.trim().is_empty() {
                    return Err(Error::MissingSubject);
                }
                OAuthAccountMetadata {
                    provider: provider.into(),
                    subject: p.id,
                    display_name: safe_text(p.display_name, "Spotify user"),
                    avatar_url: safe_url(
                        p.images.and_then(|mut x| x.drain(..).next()).map(|x| x.url),
                    ),
                    capabilities: vec!["identity".into()],
                    connected_at,
                }
            }
            "youtube-music" => {
                let p: GoogleProfile =
                    serde_json::from_slice(&response.body).map_err(|_| Error::InvalidResponse)?;
                if p.sub.trim().is_empty() {
                    return Err(Error::MissingSubject);
                }
                OAuthAccountMetadata {
                    provider: provider.into(),
                    subject: p.sub,
                    display_name: safe_text(p.name.or(p.email), "Google user"),
                    avatar_url: safe_url(p.picture),
                    capabilities: vec!["identity".into()],
                    connected_at,
                }
            }
            _ => unreachable!(),
        };
        metadata.subject = metadata.subject.trim().chars().take(200).collect();
        Ok(metadata)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        struct Fake {
            response: Response,
            request: Option<Request>,
        }
        impl Transport for Fake {
            fn send(&mut self, request: Request) -> Result<Response, TransportError> {
                self.request = Some(request);
                Ok(self.response.clone())
            }
        }
        #[test]
        fn normalizes_spotify_without_exposing_token() {
            let mut fake = Fake { response: Response { status: 200, body: br#"{"id":"sp-1","display_name":"DJ","images":[{"url":"https://img.test/a"}]}"#.to_vec() }, request: None };
            let account = current_user(&mut fake, "spotify", "secret", "now".into()).unwrap();
            assert_eq!(account.subject, "sp-1");
            assert_eq!(account.avatar_url.as_deref(), Some("https://img.test/a"));
            assert!(!serde_json::to_string(&account).unwrap().contains("secret"));
            assert_eq!(fake.request.unwrap().url, "https://api.spotify.com/v1/me");
        }
        #[test]
        fn normalizes_google_userinfo_and_rejects_insecure_avatar() {
            let mut fake = Fake {
                response: Response {
                    status: 200,
                    body: br#"{"sub":"g-1","email":"a@example.test","picture":"http://bad"}"#
                        .to_vec(),
                },
                request: None,
            };
            let account = current_user(&mut fake, "youtube-music", "secret", "now".into()).unwrap();
            assert_eq!(account.display_name, "a@example.test");
            assert_eq!(account.avatar_url, None);
            assert_eq!(
                fake.request.unwrap().url,
                "https://openidconnect.googleapis.com/v1/userinfo"
            );
        }
    }
}

/// Completes the native code exchange and profile lookup. The returned token set must stay native.
#[allow(clippy::too_many_arguments)]
pub async fn complete_authorization<T: profile::Transport>(
    coordinator: &Coordinator,
    provider: &str,
    state: &str,
    code: Option<&str>,
    error: Option<&str>,
    redirect_uri: &str,
    token_client: &token::Client,
    profile_transport: &mut T,
    connected_at: String,
) -> Result<(OAuthAccountMetadata, token::TokenSet), String> {
    let callback = coordinator
        .complete_callback(provider, state, code, error, redirect_uri)
        .await
        .map_err(|e| e.to_string())?;
    let CallbackResult::Code { code, verifier } = callback else {
        return Err("authorization denied".into());
    };
    let tokens = token_client
        .exchange_code(&code, redirect_uri, &verifier)
        .await
        .map_err(|e| e.to_string())?;
    let account = profile::current_user(
        profile_transport,
        provider,
        &tokens.access_token,
        connected_at,
    )
    .map_err(|e| e.to_string())?;
    Ok((account, tokens))
}

/// Metadata-only credential boundary. Implementations keep the secret native-only.
pub mod credentials {
    use super::ProviderError;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CredentialMetadata {
        pub provider: String,
        pub account_id: String,
        pub present: bool,
    }

    pub trait Store {
        fn put(
            &mut self,
            provider: &str,
            account_id: &str,
            secret: &str,
        ) -> Result<CredentialMetadata, ProviderError>;
        fn remove(
            &mut self,
            provider: &str,
            account_id: &str,
        ) -> Result<CredentialMetadata, ProviderError>;
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::collections::HashSet;

        struct FakeStore {
            entries: HashSet<(String, String)>,
        }
        impl Store for FakeStore {
            fn put(
                &mut self,
                provider: &str,
                account_id: &str,
                secret: &str,
            ) -> Result<CredentialMetadata, ProviderError> {
                assert!(!secret.is_empty());
                self.entries.insert((provider.into(), account_id.into()));
                Ok(CredentialMetadata {
                    provider: provider.into(),
                    account_id: account_id.into(),
                    present: true,
                })
            }
            fn remove(
                &mut self,
                provider: &str,
                account_id: &str,
            ) -> Result<CredentialMetadata, ProviderError> {
                self.entries.remove(&(provider.into(), account_id.into()));
                Ok(CredentialMetadata {
                    provider: provider.into(),
                    account_id: account_id.into(),
                    present: false,
                })
            }
        }

        #[test]
        fn stores_and_disconnects_metadata_without_returning_secret() {
            let mut store = FakeStore {
                entries: HashSet::new(),
            };
            let metadata = store.put("spotify", "account-1", "fake-token").unwrap();
            assert_eq!(
                metadata,
                CredentialMetadata {
                    provider: "spotify".into(),
                    account_id: "account-1".into(),
                    present: true
                }
            );
            let removed = store.remove("spotify", "account-1").unwrap();
            assert!(!removed.present);
        }
    }
}

/// Provider-neutral HTTP boundary for playlist APIs. Tokens remain in native memory.
pub mod playlist {
    use aurora_core::domain::sync::{RemotePage, RemotePlaylist, RemoteProvider, SyncCapabilities};
    use serde::Deserialize;
    use thiserror::Error;
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Request {
        pub method: String,
        pub url: String,
        pub headers: Vec<(String, String)>,
    }
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Response {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }
    pub trait Http {
        fn send(&mut self, r: Request) -> Result<Response, Error>;
    }
    /// Sends a protected request and, on exactly one 401, refreshes the
    /// access token through the native token client before retrying once.
    pub async fn authorized_get<H: Http>(
        http: &mut H,
        tokens: &mut super::token::TokenSet,
        token_client: &super::token::Client,
        url: String,
        max_attempts: u32,
    ) -> Result<Response, Error> {
        match get(http, url.clone(), &tokens.access_token, max_attempts) {
            Err(Error::RefreshRequired) => {
                let refresh = tokens
                    .refresh_token
                    .as_deref()
                    .ok_or(Error::RefreshRequired)?;
                let updated = token_client
                    .refresh(refresh)
                    .await
                    .map_err(|_| Error::RefreshRequired)?;
                *tokens = updated;
                // The refreshed token is used for one retry only; a second 401
                // is returned as RefreshRequired and never refreshes recursively.
                get(http, url, &tokens.access_token, max_attempts)
            }
            result => result,
        }
    }
    #[derive(Debug, Error, Clone, PartialEq, Eq)]
    pub enum Error {
        #[error("HTTPS required")]
        InsecureUrl,
        #[error("authentication expired; refresh token and retry")]
        RefreshRequired,
        #[error("HTTP {status}")]
        Http {
            status: u16,
            retry_after_ms: Option<u64>,
        },
        #[error("invalid response")]
        InvalidResponse,
    }
    fn retry_after(h: &[(String, String)]) -> Option<u64> {
        h.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("retry-after"))
            .and_then(|(_, v)| v.parse::<u64>().ok())
            .and_then(|s| s.checked_mul(1000))
    }
    pub fn get<H: Http>(
        h: &mut H,
        url: String,
        token: &str,
        max_attempts: u32,
    ) -> Result<Response, Error> {
        let parsed = url::Url::parse(&url).map_err(|_| Error::InsecureUrl)?;
        if parsed.scheme() != "https" || parsed.host_str().is_none() {
            return Err(Error::InsecureUrl);
        }
        for n in 0..max_attempts.max(1) {
            let r = h.send(Request {
                method: "GET".into(),
                url: url.clone(),
                headers: vec![("Authorization".into(), format!("Bearer {token}"))],
            })?;
            if (200..300).contains(&r.status) {
                return Ok(r);
            };
            if r.status == 401 {
                return Err(Error::RefreshRequired);
            }
            let retry = r.status == 408 || r.status == 429 || r.status >= 500;
            if !retry || n + 1 >= max_attempts.max(1) {
                return Err(Error::Http {
                    status: r.status,
                    retry_after_ms: retry_after(&r.headers),
                });
            }
        }
        unreachable!()
    }
    #[derive(Debug, Deserialize)]
    struct SpotifyPage {
        items: Vec<Item>,
        next: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct Item {
        id: String,
        name: String,
        snapshot_id: Option<String>,
    }
    pub fn spotify_playlists<H: Http>(
        h: &mut H,
        base: &str,
        token: &str,
        cursor: Option<&str>,
    ) -> Result<RemotePage<RemotePlaylist>, Error> {
        let url = format!(
            "{}/me/playlists{}",
            base,
            cursor.map(|c| format!("?offset={c}")).unwrap_or_default()
        );
        let p: SpotifyPage = serde_json::from_slice(&get(h, url, token, 4)?.body)
            .map_err(|_| Error::InvalidResponse)?;
        Ok(RemotePage {
            items: p
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
            next_cursor: p.next.and_then(|next| {
                url::Url::parse(&next).ok().and_then(|u| {
                    u.query_pairs()
                        .find(|(k, _)| k == "offset")
                        .map(|(_, v)| v.into_owned())
                })
            }),
            snapshot: None,
        })
    }
    #[derive(Debug, Deserialize)]
    struct YoutubePage {
        items: Vec<YoutubeItem>,
        #[serde(default)]
        next_page_token: Option<String>,
    }
    #[derive(Debug, Deserialize)]
    struct YoutubeItem {
        id: String,
        snippet: YoutubeSnippet,
    }
    #[derive(Debug, Deserialize)]
    struct YoutubeSnippet {
        title: String,
    }

    pub fn youtube_playlists<H: Http>(
        h: &mut H,
        base: &str,
        token: &str,
        cursor: Option<&str>,
    ) -> Result<RemotePage<RemotePlaylist>, Error> {
        let mut url = url::Url::parse(&format!("{}/playlists", base.trim_end_matches('/')))
            .map_err(|_| Error::InsecureUrl)?;
        url.query_pairs_mut()
            .append_pair("part", "snippet")
            .append_pair("mine", "true")
            .append_pair("maxResults", "50");
        if let Some(cursor) = cursor {
            url.query_pairs_mut().append_pair("pageToken", cursor);
        }
        let p: YoutubePage = serde_json::from_slice(&get(h, url.to_string(), token, 4)?.body)
            .map_err(|_| Error::InvalidResponse)?;
        Ok(RemotePage {
            items: p
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
            next_cursor: p.next_page_token,
            snapshot: None,
        })
    }

    pub fn capabilities_for(provider: RemoteProvider) -> SyncCapabilities {
        match provider {
            RemoteProvider::Spotify | RemoteProvider::Youtube => SyncCapabilities::READ_ONLY,
            _ => SyncCapabilities {
                can_read: false,
                ..SyncCapabilities::READ_ONLY
            },
        }
    }

    pub fn capabilities() -> SyncCapabilities {
        SyncCapabilities::READ_ONLY
    }
}
