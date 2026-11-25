use anyhow::Result;
use tokio::sync::watch;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stagelock_daemon=info,stagelock_media=info,stagelock_ptz=info,tower_http=info".into()),
        )
        .init();

    let (shutdown_tx, _shutdown_rx) = watch::channel(false);
    stagelock_daemon::run_daemon(stagelock_daemon::DaemonOptions::from_env(), shutdown_tx).await
}
