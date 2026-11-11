use std::{env, net::SocketAddr};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ghostchat_server=info,tower_http=warn".into()),
        )
        .with_target(false)
        .init();
    let address: SocketAddr = env::var("GHOSTCHAT_BIND")
        .unwrap_or_else(|_| "0.0.0.0:38241".into())
        .parse()?;
    let database = env::var("GHOSTCHAT_DATABASE").unwrap_or_else(|_| "ghostchat.sqlite".into());
    // Only this standalone binary updates itself (Windows); the desktop's
    // built-in server calls `ghostchat_server::serve` and never does.
    let auto_update = env::var("GHOSTCHAT_AUTO_UPDATE").ok();
    let self_update = ghostchat_server::self_update::enabled(auto_update.as_deref(), cfg!(windows));
    if cfg!(windows) && !self_update {
        tracing::info!("self-update: off (GHOSTCHAT_AUTO_UPDATE)");
    }
    ghostchat_server::run_standalone(address, &database, self_update).await
}
