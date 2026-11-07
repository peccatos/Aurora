use aurora_api::{Config, ProviderSettings, memory_state, serve};
use std::{env, net::SocketAddr};

fn provider(prefix: &str) -> Result<Option<ProviderSettings>, Box<dyn std::error::Error>> {
    let enabled = env::var(format!("AURORA_{prefix}_ENABLED"))
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
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
    let frontend_url = env::var("AURORA_API_FRONTEND_URL")?;
    let state = memory_state(Config {
        allowed_origins: origins,
        frontend_url,
        spotify: provider("SPOTIFY")?,
        youtube: provider("YOUTUBE_MUSIC")?,
    });
    serve(state, "127.0.0.1:8080".parse::<SocketAddr>()?).await?;
    Ok(())
}
