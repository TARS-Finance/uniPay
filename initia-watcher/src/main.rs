mod config;
mod errors;
mod events;
mod storage;
mod watcher;

use std::env;

use tracing::info;
use tracing_subscriber::EnvFilter;

use config::Config;
use watcher::manager::WatcherManager;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "config.toml".to_string());

    let config = Config::load(&config_path)?;

    info!("Initialising orderbook and checkpoint store...");
    let manager = WatcherManager::new(config).await?;

    info!("Spawning HTLC watchers...");
    let (handles, shutdown_tx) = manager.spawn().await?;

    tokio::signal::ctrl_c().await?;
    info!("Shutdown signal received, stopping watchers...");

    let _ = shutdown_tx.send(());
    for handle in handles {
        let _ = handle.await;
    }

    info!("All watchers stopped.");
    Ok(())
}
