use std::sync::Arc;

use alloy::providers::ProviderBuilder;
use tokio::task::JoinHandle;
use tracing::info;

use orderbook::OrderbookProvider;

use crate::config::Config;
use crate::errors::{Result, WatcherError};
use crate::storage::PgStore;
use crate::watcher::htlc_watcher::HtlcWatcher;

pub struct WatcherManager {
    config: Config,
    orderbook: Arc<OrderbookProvider>,
    checkpoints: PgStore,
}

impl WatcherManager {
    pub async fn new(config: Config) -> Result<Self> {
        let orderbook = OrderbookProvider::from_db_url(&config.database_url)
            .await
            .map_err(|e| WatcherError::Database(e.to_string()))?;

        // Dedicated sqlx pool for checkpoint table; connection count is minimal.
        let checkpoint_pool = sqlx::PgPool::connect(&config.database_url)
            .await
            .map_err(|e| WatcherError::Database(e.to_string()))?;
        let checkpoints = PgStore::from_pool(checkpoint_pool);
        checkpoints.ensure_checkpoint_table().await?;

        Ok(Self {
            config,
            orderbook: Arc::new(orderbook),
            checkpoints,
        })
    }

    /// Spawns one `HtlcWatcher` per (chain, htlc_address) pair.
    pub async fn spawn(
        &self,
    ) -> Result<(Vec<JoinHandle<()>>, tokio::sync::broadcast::Sender<()>)> {
        let (shutdown_tx, _) = tokio::sync::broadcast::channel::<()>(1);
        let mut handles = Vec::new();

        for chain in &self.config.chains {
            let url: url::Url = chain
                .rpc_url
                .parse()
                .map_err(|e| WatcherError::Config(format!("Invalid RPC URL '{}': {e}", chain.rpc_url)))?;

            let provider = ProviderBuilder::new().connect_http(url);

            for pair in &chain.pairs {
                let htlc_address: alloy::primitives::Address = pair
                    .htlc_address
                    .parse()
                    .map_err(|_| WatcherError::InvalidAddress(pair.htlc_address.clone()))?;

                let token_address: alloy::primitives::Address = pair
                    .token_address
                    .parse()
                    .map_err(|_| WatcherError::InvalidAddress(pair.token_address.clone()))?;

                let start_block = self
                    .checkpoints
                    .get_checkpoint(&chain.name, &pair.htlc_address)
                    .await?
                    .unwrap_or(0);

                let watcher = HtlcWatcher {
                    chain_name: chain.name.clone(),
                    htlc_address,
                    token_address,
                    start_block,
                    interval_ms: self.config.interval_ms(),
                    block_span: self.config.block_span(),
                    provider: provider.clone(),
                    orderbook: Arc::clone(&self.orderbook),
                    checkpoints: self.checkpoints.clone(),
                };

                let shutdown_rx = shutdown_tx.subscribe();

                info!(
                    chain = %chain.name,
                    htlc  = %htlc_address,
                    token = %token_address,
                    start_block,
                    "Spawning watcher"
                );

                handles.push(tokio::spawn(async move {
                    watcher.run(shutdown_rx).await;
                }));
            }
        }

        Ok((handles, shutdown_tx))
    }
}
