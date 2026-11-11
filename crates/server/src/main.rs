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
    // An exe the self-updater started must be the version it installed;
    // anything else stops here, before the database is opened.
    let expected = env::var(ghostchat_server::self_update::EXPECT_VERSION_ENV).ok();
    if let Err(reason) = ghostchat_server::self_update::check_expected_version(
        expected.as_deref(),
        ghostchat_server::SERVER_VERSION,
    ) {
        tracing::error!(%reason, "self-update: refusing to start");
        std::process::exit(2);
    }
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
