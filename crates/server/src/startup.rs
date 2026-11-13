//! Serving the router: the desktop's built-in server and the standalone exe with its self-update restart.

use super::*;

/// The tasks every server runs next to its listener: the media revocation
/// reconciler and the upload sweeps. Both `serve` (the desktop's built-in
/// server) and `run_standalone` (the server exe) start them once; dropping
/// the value stops them.
pub(crate) struct BackgroundTasks(pub(crate) Vec<tokio::task::JoinHandle<()>>);

impl BackgroundTasks {
    pub(crate) fn spawn(state: &AppState) -> Self {
        Self(vec![
            tokio::spawn(media_revocation::run(state.clone())),
            tokio::spawn(run_unclaimed_upload_sweep(state.clone())),
        ])
    }
}

impl Drop for BackgroundTasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}

pub async fn serve(listener: tokio::net::TcpListener, state: AppState) -> std::io::Result<()> {
    let _background = BackgroundTasks::spawn(&state);
    axum::serve(listener, router(state)).await
}

pub async fn bind_default(
    addr: SocketAddr,
    database_path: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    run_standalone(addr, database_path, None).await
}

/// How long in-flight requests may finish when the self-updater stops the
/// listener; WebSockets keep running until this process exits.
pub(crate) const SELF_UPDATE_DRAIN: Duration = Duration::from_secs(10);

/// The standalone server (`main.rs`). With `self_update` (its release
/// channel) it also runs the self-updater, which may stop the listener to
/// swap the exe and start it again if the new exe fails (see `self_update`).
pub async fn run_standalone(
    addr: SocketAddr,
    database_path: &str,
    self_update: Option<self_update::UpdateChannel>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let db = open_database(database_path).await?;
    let attachment_dir = env::var_os("GHOSTCHAT_ATTACHMENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(database_path)
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("ghostchat-attachments")
        });
    let state = build_state_with_attachment_dir(db.clone(), attachment_dir).await;
    // Started once, outside the rebind loop of `serve_standalone`.
    let _background = BackgroundTasks::spawn(&state);
    // `control` lives as long as this loop, so `wanted` never closes.
    let (control, mut signals) = self_update::ServeControl::new();
    if let Some(channel) = self_update {
        let rooms = state.media_rooms.clone();
        self_update::start(
            db,
            database_path.to_owned(),
            addr,
            control.clone(),
            std::sync::Arc::new(move || rooms.any_active()),
            channel,
        );
    }
    let served = serve_standalone(addr, state, &mut signals).await;
    drop(control);
    served
}

/// Serves until Ctrl+C; releases the port while `signals.wanted` is false
/// (the self-updater swapping the exe) and listens again when it is true.
pub(crate) async fn serve_standalone(
    addr: SocketAddr,
    state: AppState,
    signals: &mut self_update::ServeSignals,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut first_bind = true;
    loop {
        let _ = signals.wanted.wait_for(|wanted| *wanted).await;
        let listener = if first_bind {
            tokio::net::TcpListener::bind(addr).await?
        } else {
            bind_after_failed_update(addr).await?
        };
        first_bind = false;
        tracing::info!(address = %listener.local_addr()?, "GhostChat server listening");
        signals.listening.send_replace(true);
        let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);
        let app = router(state.clone());
        let mut server = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = stop_rx.wait_for(|stop| *stop).await;
                })
                .await
        });
        let exiting = tokio::select! {
            result = &mut server => {
                result??;
                return Ok(());
            }
            _ = tokio::signal::ctrl_c() => true,
            _ = signals.wanted.wait_for(|wanted| !*wanted) => false,
        };
        stop_tx.send_replace(true);
        if exiting {
            server.await??;
            return Ok(());
        }
        if tokio::time::timeout(SELF_UPDATE_DRAIN, &mut server)
            .await
            .is_err()
        {
            server.abort();
            let _ = server.await;
        }
        signals.listening.send_replace(false);
        tracing::info!("GhostChat server stopped listening for the self-update");
    }
}

/// The new exe was stopped a moment ago, so its port may still be closing.
pub(crate) async fn bind_after_failed_update(
    addr: SocketAddr,
) -> std::io::Result<tokio::net::TcpListener> {
    let mut attempt = 0;
    loop {
        match tokio::net::TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(error) if attempt < 30 => {
                attempt += 1;
                tracing::warn!(%error, "could not listen again yet; retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(error) => return Err(error),
        }
    }
}
