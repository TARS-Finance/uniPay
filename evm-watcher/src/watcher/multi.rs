use crate::swaps::{SwapStore, group_by_chains};
use eyre::Result;
use tars::orderbook::primitives::SingleSwap;
use moka::future::Cache;
use std::{sync::Arc, time::Duration};

/// Manages multi-chain swap data with caching and periodic polling.
pub struct MultiChainSwapsProvider {
    // Shared cache for storing pending swaps by chain
    cache: Arc<Cache<String, Vec<SingleSwap>>>,
    store: Arc<dyn SwapStore + Send + Sync>,
    polling_interval: u64,
}

impl MultiChainSwapsProvider {
    /// Creates a new `MultiChainSwapsProvider`.
    ///
    /// # Arguments
    /// * `cache` - Shared cache for storing swaps by chain
    /// * `store` - Swap storage backend
    /// * `polling_interval` - Interval for polling new swaps
    pub fn new(
        cache: Arc<Cache<String, Vec<SingleSwap>>>,
        store: Arc<dyn SwapStore + Send + Sync>,
        polling_interval: u64,
    ) -> Arc<Self> {
        let ret = Arc::new(Self {
            cache,
            store,
            polling_interval,
        });

        let ret_clone = ret.clone();
        tokio::spawn(async move {
            ret_clone.watch().await;
        });

        return ret;
    }

    /// Starts the swap polling loop, continuously updating the cache with new swaps.
    async fn watch(&self) {
        tracing::info!("Starting MultiChainSwapProvider watch loop");
        loop {
            // Wait for the next polling interval
            tokio::time::sleep(Duration::from_secs(self.polling_interval)).await;
            let swaps = match self.store.get_swaps().await {
                Ok(swaps) => swaps,
                Err(e) => {
                    tracing::error!("Failed to get pending swaps: {}", e);
                    continue;
                }
            };
            // Clear existing cache entries
            self.cache.invalidate_all();

            // Group swaps by chain and insert into cache
            for (chain, chain_swaps) in group_by_chains(&swaps) {
                self.cache.insert(chain.to_string(), chain_swaps).await;
            }
        }
    }

    pub async fn get_swaps(&self, chain: &str) -> Result<Option<Vec<SingleSwap>>> {
        Ok(self.cache.get(chain).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eyre::Ok;
    use tars::orderbook::test_utils::default_matched_order;
    use std::collections::HashMap;

    pub struct MockSwapStore {
        swaps: Vec<SingleSwap>,
    }

    impl MockSwapStore {
        pub fn new(swaps: Vec<SingleSwap>) -> Self {
            Self { swaps }
        }
        pub fn add(&mut self, new_swaps: Vec<SingleSwap>) {
            self.swaps.extend(new_swaps);
        }
    }

    #[async_trait::async_trait]
    impl SwapStore for MockSwapStore {
        async fn get_swaps(&self) -> Result<Vec<SingleSwap>> {
            return Ok(self.swaps.clone());
        }

        async fn update_events(
            &self,
            _: &tars::utils::NonEmptyVec<&crate::swaps::SwapEvent>,
        ) -> Result<()> {
            Ok(())
        }

        async fn update_confirmations<'a>(&self, _: HashMap<i64, &'a [String]>) -> Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_multi_chain_swaps_provider() {
        let cache = Cache::new(100);
        let mut store = MockSwapStore::new(vec![]);
        store.add(vec![default_matched_order().source_swap]);
        store.add(vec![default_matched_order().destination_swap]);
        let provider = MultiChainSwapsProvider::new(Arc::new(cache), Arc::new(store), 1);
        tokio::time::sleep(Duration::from_secs(2)).await;
        let swaps = provider
            .get_swaps("arbitrum_localnet")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(swaps.len(), 1);
    }
}
