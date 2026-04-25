use crate::{
    config::solver::{ChainConfig, SolverSettings},
    liquidity::{
        fetchers::{LiquidityFetcher, build_fetcher},
        primitives::{AssetLiquidity, SolverLiquidity},
    },
    metadata::MetadataIndex,
};
use bigdecimal::{BigDecimal, Zero};
use std::{collections::HashMap, str::FromStr, sync::Arc, time::Duration};
use tars::orderbook::{OrderbookProvider, traits::Orderbook};
use tokio::sync::RwLock;

/// Maintains an in-memory view of solver liquidity across all configured chains.
pub struct LiquidityWatcher {
    settings: SolverSettings,
    metadata: Arc<MetadataIndex>,
    orderbook: Arc<OrderbookProvider>,
    state: Arc<RwLock<SolverLiquidity>>,
    fetchers: HashMap<String, Box<dyn LiquidityFetcher>>,
}

impl LiquidityWatcher {
    /// Creates the watcher and prebuilds one fetcher per configured chain.
    pub async fn new(
        settings: SolverSettings,
        metadata: Arc<MetadataIndex>,
        orderbook: Arc<OrderbookProvider>,
    ) -> eyre::Result<Self> {
        let mut fetchers = HashMap::new();
        for (chain, config) in &settings.chains {
            fetchers.insert(chain.clone(), build_fetcher(chain, &config.rpc_url).await?);
        }

        Ok(Self {
            settings: settings.clone(),
            metadata,
            orderbook,
            state: Arc::new(RwLock::new(SolverLiquidity {
                solver_id: settings.solver_id.clone(),
                liquidity: Vec::new(),
            })),
            fetchers,
        })
    }

    /// Starts the recurring background refresh loop.
    pub fn start(self: &Arc<Self>) {
        let watcher = self.clone();
        tokio::spawn(async move {
            if let Err(error) = watcher.refresh().await {
                tracing::warn!(?error, "initial liquidity refresh failed");
            }

            let interval = Duration::from_millis(watcher.settings.polling_interval_ms);
            loop {
                tokio::time::sleep(interval).await;
                if let Err(error) = watcher.refresh().await {
                    tracing::warn!(?error, "liquidity refresh failed");
                }
            }
        });
    }

    /// Returns the configured solver identifier for public API responses.
    pub fn solver_id(&self) -> &str {
        &self.settings.solver_id
    }

    /// Returns the latest liquidity snapshot.
    pub async fn all(&self) -> SolverLiquidity {
        self.state.read().await.clone()
    }

    /// Checks whether the solver can cover a destination-side payout amount.
    pub async fn has_destination_liquidity(
        &self,
        chain: &str,
        asset: &str,
        amount: &BigDecimal,
    ) -> bool {
        let Some(metadata_asset) = self.metadata.get_asset_by_chain_and_htlc(chain, asset) else {
            return false;
        };
        let key = metadata_asset.asset.id.to_string().to_lowercase();
        self.state
            .read()
            .await
            .liquidity
            .iter()
            .find(|entry| entry.asset.eq_ignore_ascii_case(&key))
            .and_then(|entry| BigDecimal::from_str(&entry.virtual_balance).ok())
            .is_some_and(|balance| balance >= *amount)
    }

    /// Applies the current liquidity rules for a candidate strategy.
    pub async fn can_fulfill(
        &self,
        strategy: &crate::registry::Strategy,
        destination_amount: &BigDecimal,
    ) -> bool {
        self.has_destination_liquidity(
            &strategy.dest_chain,
            &strategy.dest_asset.htlc_address,
            destination_amount,
        )
        .await
    }

    /// Rebuilds the full liquidity snapshot from chain balances and committed funds.
    async fn refresh(&self) -> eyre::Result<()> {
        let mut entries = Vec::new();

        // Recompute the snapshot from scratch to avoid carrying stale per-asset state.
        for (chain, config) in &self.settings.chains {
            self.refresh_chain(chain, config, &mut entries).await;
        }

        let mut guard = self.state.write().await;
        guard.solver_id = self.settings.solver_id.clone();
        guard.liquidity = entries;
        Ok(())
    }

    /// Refreshes all supported assets for a single configured chain.
    async fn refresh_chain(
        &self,
        chain: &str,
        config: &ChainConfig,
        entries: &mut Vec<AssetLiquidity>,
    ) {
        let Some(fetcher) = self.fetchers.get(chain) else {
            tracing::warn!(chain, "missing fetcher for configured chain");
            return;
        };

        for asset_id in &config.supported_assets {
            let Some(asset) = self.metadata.get_asset_by_id(asset_id) else {
                tracing::warn!(chain, asset_id, "solver asset missing from metadata");
                continue;
            };

            let token = if asset.asset.token.address == "primary" {
                "primary"
            } else {
                asset.asset.token.address.as_str()
            };

            // The fetcher returns the raw on-chain balance before accounting for open orders.
            let balance = match fetcher.fetch(config.liquidity_account(), token).await {
                Ok(balance) => balance,
                Err(error) => {
                    tracing::warn!(chain, asset_id, ?error, "failed to fetch solver balance");
                    continue;
                }
            };

            // Subtract funds already reserved by persisted matched orders to get spendable liquidity.
            let committed = self
                .orderbook
                .get_solver_committed_funds(
                    config.order_identity(),
                    chain,
                    &asset.asset.htlc.address,
                )
                .await
                .unwrap_or_else(|_| BigDecimal::zero());

            let virtual_balance = (balance.clone() - committed).max(BigDecimal::zero());
            entries.push(AssetLiquidity {
                asset: asset.asset.id.to_string(),
                balance: balance.to_string(),
                virtual_balance: virtual_balance.to_string(),
                readable_balance: format_balance(&balance, asset.asset.decimals),
            });
        }
    }
}

/// Formats raw integer balances using the asset decimals for easier inspection.
fn format_balance(balance: &BigDecimal, decimals: u8) -> String {
    let divisor = BigDecimal::from(10_u64.pow(decimals as u32));
    let result = balance / divisor;
    format!("{result:.8}")
}
