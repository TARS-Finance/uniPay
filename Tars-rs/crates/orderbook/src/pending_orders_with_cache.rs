use crate::{pending_orders::PendingOrdersProvider, primitives::MatchedOrderVerbose};
use eyre::{eyre, Result};
use moka::future::Cache;
use std::{collections::HashMap, sync::Arc, thread::sleep, time::Duration};
use tokio::time::interval;
use tracing::warn;

/// Maximum number of entries the cache can hold.
const MAX_CACHE_SIZE: u64 = 1000;

/// Time-to-live (TTL) for cached entries.
const CACHE_TTL: Duration = Duration::from_secs(60);

/// The interval in milliseconds for updating the pending orders cache.
const DEFAULT_PENDING_ORDERS_CACHE_UPDATE_INTERVAL_MS: u64 = 2000;

/// A wrapper around [`PendingOrdersProvider`] that adds caching capabilities.
///
/// This struct reduces redundant remote API calls by storing fetched
/// pending orders in an in-memory cache. Cached entries are automatically
/// evicted based on the configured TTL and maximum capacity.
pub struct PendingOrdersProviderWithCache {
    /// The underlying provider used to fetch pending orders from the remote API.
    provider: Arc<PendingOrdersProvider>,

    /// In-memory cache of pending orders, keyed by chain identifier.
    cache: Cache<String, Vec<MatchedOrderVerbose>>,
}

impl PendingOrdersProviderWithCache {
    /// Creates a new [`PendingOrdersProviderWithCache`] instance.
    ///
    /// # Arguments
    /// * `provider` - An [`Arc`] pointing to a [`PendingOrdersProvider`] instance.
    /// * `chain` - An optional string specifying the chain to fetch pending orders for.
    ///   If `None`, pending orders for all chains are fetched.
    /// * `update_interval_ms` - An optional number of milliseconds to update the cache.
    ///   If `None`, the default interval of 2000ms is used.
    ///
    /// # Returns
    /// A new [`Arc`] pointing to a [`PendingOrdersProviderWithCache`] with an initialized cache.
    pub fn new(
        provider: Arc<PendingOrdersProvider>,
        chain: Option<String>,
        update_interval_ms: Option<u64>,
    ) -> Arc<Self> {
        let cache = Cache::builder()
            .max_capacity(MAX_CACHE_SIZE)
            .time_to_live(CACHE_TTL)
            .build();

        let pending_orders_provider = Arc::new(Self { provider, cache });

        // Run the cache updater in a separate thread.
        let pending_orders_provider_clone = pending_orders_provider.clone();
        tokio::spawn(async move {
            pending_orders_provider_clone
                .run(
                    update_interval_ms.unwrap_or(DEFAULT_PENDING_ORDERS_CACHE_UPDATE_INTERVAL_MS),
                    chain,
                )
                .await;
        });

        // Wait for the cache to be updated initially.
        sleep(Duration::from_secs(5));

        pending_orders_provider
    }

    /// Runs the cache updater.
    ///
    /// # Arguments
    /// * `interval_ms` - The interval in milliseconds to fetch the pending orders.
    /// * `chain` - An optional string specifying the chain to fetch pending orders for.
    ///   If `None`, pending orders for all chains are fetched.
    async fn run(&self, interval_ms: u64, chain: Option<String>) {
        let mut interval = interval(Duration::from_millis(interval_ms));

        loop {
            interval.tick().await;

            // Fetch the pending orders.
            let pending_orders = match self.provider.get_pending_orders(chain.clone()).await {
                Ok(pending_orders) => pending_orders,
                Err(e) => {
                    warn!("Failed to fetch pending orders: {:#?}", e.to_string());
                    continue;
                }
            };

            // Collect all orders by chain first
            let mut chain_orders: HashMap<String, Vec<MatchedOrderVerbose>> = HashMap::new();

            for order in pending_orders.iter() {
                let source_chain = order.source_swap.chain.clone();
                let destination_chain = order.destination_swap.chain.clone();

                // Add to source chain
                chain_orders
                    .entry(source_chain)
                    .or_default()
                    .push(order.clone());

                // Add to destination chain
                chain_orders
                    .entry(destination_chain)
                    .or_default()
                    .push(order.clone());
            }

            // Batch insert all orders into cache
            for (chain, orders) in chain_orders {
                self.cache.insert(chain, orders).await;
            }
        }
    }

    /// Retrieves pending orders from the cache.
    ///
    /// # Arguments
    /// * `chain` - A string specifying the chain to retrieve pending orders for.
    ///
    /// # Returns
    /// * `Ok(Vec<MatchedOrderVerbose>)` if cached orders are found.
    /// * `Err(e)` if no cache entry exists for the given chain.
    pub async fn get_pending_orders(&self, chain: &str) -> Result<Vec<MatchedOrderVerbose>> {
        self.cache
            .get(chain)
            .await
            .ok_or(eyre!("No pending orders found for chain: {}", chain))
    }
}

/// A reference-counted wrapper around [`PendingOrdersProviderWithCache`].
pub type ArcPendingOrdersProviderWithCache = Arc<PendingOrdersProviderWithCache>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use api::primitives::{Response, Status};
    use reqwest::StatusCode;
    use wiremock::{
        matchers::{method, path_regex},
        Mock, MockServer, ResponseTemplate,
    };

    async fn setup_server() -> String {
        let mock_server = MockServer::start().await;

        let order1 = test_utils::default_matched_order();
        let order2 = test_utils::default_matched_order();
        let orders = vec![order1, order2];

        let response = Response {
            status: Status::Ok,
            result: Some(orders),
            error: None,
            status_code: StatusCode::OK,
        };

        Mock::given(method("GET"))
            .and(path_regex(r"/arbitrum_localnet$")) // Match any URL ending with /arbitrum_localnet
            .respond_with(ResponseTemplate::new(200).set_body_json(&response)) // Pass the orders array directly
            .mount(&mock_server)
            .await;

        mock_server.uri()
    }

    #[tokio::test]
    async fn test_get_pending_orders_with_cache() {
        let _ = tracing_subscriber::fmt().try_init();

        let uri = setup_server().await;

        let provider = PendingOrdersProvider::new(&uri, None);

        let provider_with_cache = PendingOrdersProviderWithCache::new(
            Arc::new(provider),
            Some("arbitrum_localnet".to_string()),
            None,
        );

        tokio::time::sleep(Duration::from_secs(1)).await;

        let result = provider_with_cache
            .get_pending_orders("arbitrum_localnet")
            .await;

        assert!(result.is_ok());

        let orders = result.unwrap();
        assert_eq!(orders.len(), 2);
        let order = orders.first().unwrap();
        assert_eq!(order.create_order.create_id, "1");

        // Should fail because no cache entry exists for all chains.
        let result = provider_with_cache
            .get_pending_orders("ethereum_localnet")
            .await;

        assert!(result.is_err());
    }
}
