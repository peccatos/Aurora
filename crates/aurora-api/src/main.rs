use aurora_api::{Config, ProviderSettings, serve, storage_state};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use std::{env, net::SocketAddr, path::Path};

fn provider(prefix: &str) -> Result<Option<ProviderSettings>, Box<dyn std::error::Error>> {
    let enabled = env::var(format!("AURORA_{prefix}_ENABLED"))
        .ok()
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if !enabled {
        return Ok(None);
    }
    Ok(Some(ProviderSettings {
        client_id: env::var(format!("AURORA_{prefix}_CLIENT_ID"))?,
        client_secret: env::var(format!("AURORA_{prefix}_CLIENT_SECRET"))?,
        redirect_uri: env::var(format!("AURORA_{prefix}_REDIRECT_URI"))?,
    }))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let origins: Vec<String> = env::var("AURORA_API_ALLOWED_ORIGINS")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(|v| {
            v.split(',')
                .map(str::trim)
                .filter(|x| !x.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if origins.is_empty() {
        return Err("AURORA_API_ALLOWED_ORIGINS must be configured; refusing open CORS".into());
    }
    if origins.iter().any(|o| !o.starts_with("https://")) {
        return Err("allowed origins must use HTTPS".into());
    }
    let key_raw = env::var("AURORA_OAUTH_ENCRYPTION_KEY")
        .map_err(|_| "AURORA_OAUTH_ENCRYPTION_KEY must be configured")?;
    let key_bytes = URL_SAFE_NO_PAD
        .decode(key_raw)
        .map_err(|_| "AURORA_OAUTH_ENCRYPTION_KEY must be base64url")?;
    let encryption_key: [u8; 32] = key_bytes
        .try_into()
        .map_err(|_| "AURORA_OAUTH_ENCRYPTION_KEY must decode to 32 bytes")?;
    let frontend_url = env::var("AURORA_API_FRONTEND_URL")?;
    let frontend = url::Url::parse(&frontend_url).map_err(|_| "invalid frontend URL")?;
    if frontend.scheme() != "https" || frontend.host_str().is_none() {
        return Err("frontend URL must be HTTPS".into());
    }
    let pool = aurora_storage::open(Path::new(
        &env::var("AURORA_DATABASE_PATH").unwrap_or_else(|_| "aurora.db".into()),
    ))
    .await?;
    let state = storage_state(
        Config {
            allowed_origins: origins,
            frontend_url,
            spotify: provider("SPOTIFY")?,
            youtube: provider("YOUTUBE_MUSIC")?,
        },
        pool,
        encryption_key,
    );
    serve(state, "127.0.0.1:8080".parse::<SocketAddr>()?).await?;
    Ok(())
}
