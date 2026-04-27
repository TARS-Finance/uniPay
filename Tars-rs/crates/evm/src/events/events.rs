/// This module contains the prelude for the events crate.
/// It is essential to import this prelude for the `event_provider!` macro to work correctly,
/// as it brings in all necessary types and traits required by the macro and event querying.
///
/// # Important
/// You must import the `prelude` module from this crate before using the `event_provider!` macro:
/// ```ignore
/// use prelude::*;
/// ```
pub mod prelude {

    pub use crate::count_types;
    pub use crate::events::primitives::EventExt;
    pub use alloy::{
        primitives::Address, providers::Provider, rpc::types::Filter, sol_types::SolEventInterface,
    };
    pub use eyre::Result;
    pub use futures::{stream::FuturesUnordered, StreamExt};
    pub use std::collections::HashMap;
    pub use std::sync::Arc;
    pub use utils::retry_with_backoff;

    /// Decodes logs for a specific event type, filtering by provided addresses.
    ///
    /// # Arguments
    /// - `logs`: List of raw logs to process.
    /// - `addresses`: List of contract addresses to filter logs.
    ///
    /// # Returns
    /// A vector of decoded events wrapped in `EventExt`.
    pub fn process_logs<T: SolEventInterface>(
        logs: &[alloy::rpc::types::Log],
        addresses: &[Address],
    ) -> Vec<EventExt<T>> {
        // Group logs by address
        let mut addr_to_log_map = HashMap::new();
        logs.iter().for_each(|log| {
            if addresses.contains(&log.address()) {
                addr_to_log_map
                    .entry(log.address())
                    .or_insert(vec![])
                    .push(log.clone());
            }
        });

        // Decode logs for each address
        addresses
            .iter()
            .flat_map(|addr| {
                addr_to_log_map
                    .get(addr)
                    .unwrap_or(&Vec::new())
                    .iter()
                    .filter_map(|log| {
                        T::decode_raw_log(log.topics(), &log.data().data)
                            .ok()
                            .map(|event| EventExt::from((log, event)))
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Generates block ranges for batched queries to limit the number of blocks per query.
    ///
    /// # Arguments
    /// - `from_block`: Starting block number.
    /// - `to_block`: Ending block number.
    /// - `max_block_span`: Maximum number of blocks per range.
    ///
    /// # Returns
    /// A vector of tuples representing block ranges `(start, end)`.
    pub fn get_block_ranges(
        from_block: u64,
        to_block: u64,
        max_block_span: u64,
    ) -> Vec<(u64, u64)> {
        (from_block..=to_block)
            .step_by(max_block_span as usize + 1)
            .map(|start| {
                let end = (start + max_block_span).min(to_block);
                (start, end)
            })
            .collect()
    }
}

#[macro_export]
/// Macro to generate a batched event provider for querying multiple Ethereum contract types events in parallel.
///
/// # Usage
/// Before using the `event_provider!` macro, import the `prelude` module from this crate:
/// ```ignore
/// use crate::events::prelude::*;
/// ```
///
/// This macro generates a provider struct and associated methods to efficiently query and decode logs for the specified event types
/// from an Ethereum node. It supports batching queries over block ranges and limits concurrency to avoid overloading the node.
///
/// # Parameters
/// - `$mod_name`: The module name for the generated provider.
/// - `($alias, $event_type)`: One or more pairs specifying an alias and the event type to query. Each event type must implement the required traits.
///
/// # Generated Items
/// - A module `$mod_name` containing:
///   - A struct `EventProvider<P>` where `P` is a generic Ethereum provider implementing `Provider + Clone + Send + Sync`.
///   - An `AddressFilter` struct for specifying addresses per event type.
///   - An async `new` constructor to initialize the provider with configuration options.
///   - An async `query` method to fetch and decode events in batches for a block range and addresses.
///
/// # Example
/// ```ignore
/// use unipay::evm::{count_types, event_provider, events::prelude};
/// pub use prelude::*;
/// event_provider!(
/// unipay_htlc,
/// (htlc_v1, UnipayHTLCEvents),
/// (htlc_v2, UnipayHTLCv2Events),
/// (htlc_v3, UnipayHTLCv3Events)
/// );
/// let provider = unipay_htlc::EventProvider::new(alloy_provider, None, None).unwrap();
/// let filter = unipay_htlc::AddressFilter::new()
/// .htlc_v1(vec![htlcv1_addr1,htlcv1_addr2])
/// .htlc_v2(vec![htlcv2_addr1,htlcv2_addr2])
/// .htlc_v3(vec![htlcv3_addr1,htlcv3_addr2]);
/// let (htlc_v1_events, htlc_v2_events, htlc_v3_events) = provider.query(from, to, filter).await?;
/// ```
///
/// # Errors
/// - Returns an error if the chain ID cannot be retrieved during initialization.
/// - Returns an error if log queries fail after all retry attempts.
///
/// # Type Parameters
/// - `P`: Ethereum provider type, must implement `Provider + Clone + Send + Sync`.
macro_rules! event_provider {
    ($mod_name:ident, $(($alias:ident, $event_type:ty)),+ $(,)?) => {
        pub mod $mod_name {
            use super::*;

            // Configuration constants for retry and concurrency
            const DEFAULT_RETRY_DELAY_MS: u64 = 500;
            const MAX_RETRY_ATTEMPTS: usize = 5;
            const DEFAULT_CONCURRENT_TASKS_LIMIT: usize = 5;
            const DEFAULT_MAX_BLOCK_SPAN: u64 = 10000;

            /// Struct to hold address filters for each event type.
            #[derive(Clone, Debug)]
            pub struct AddressFilter {
                $(
                    $alias: Vec<Address>,
                )+
            }

            impl AddressFilter {
                /// Creates a new `AddressFilter` with empty address vectors.
                pub fn new() -> Self {
                    Self {
                        $(
                            $alias: vec![],
                        )+
                    }
                }

                $(
                    /// Sets addresses for the `$alias` event type.
                    pub fn $alias(mut self, addresses: Vec<Address>) -> Self {
                        self.$alias = addresses;
                        self
                    }
                )+

                /// Builds an array of address vectors for all event types.
                pub fn build(&self) -> [Vec<Address>; count_types!($($event_type),+)] {
                    [
                        $(
                            self.$alias.clone(),
                        )+
                    ]
                }
            }

            /// Batched event provider for querying Unipay HTLC contract events in parallel.
            pub struct EventProvider{
                provider: Arc<dyn Provider>,
                max_block_span: u64,
                concurrent_tasks_limit: usize,
            }

            impl EventProvider
            {
                /// Initializes a new event provider with the specified configuration.
                ///
                /// # Arguments
                /// - `provider`: Ethereum provider for querying logs.
                /// - `max_block_span`: Maximum block range per query (defaults to `DEFAULT_MAX_BLOCK_SPAN`).
                /// - `concurrent_tasks_limit`: Maximum concurrent tasks (defaults to `DEFAULT_CONCURRENT_TASKS_LIMIT`).
                ///
                /// # Returns
                /// A `Result` containing the initialized provider or an error if chain ID retrieval fails.
                pub fn new(
                    provider: Arc<dyn Provider>,
                    max_block_span: Option<u64>,
                    concurrent_tasks_limit: Option<usize>,
                ) -> Self {
                    Self {
                        provider,
                        max_block_span: max_block_span.unwrap_or(DEFAULT_MAX_BLOCK_SPAN),
                        concurrent_tasks_limit: concurrent_tasks_limit.unwrap_or(DEFAULT_CONCURRENT_TASKS_LIMIT),
                    }
                }

                #[allow(unused_assignments)]
                /// Queries events for the specified block range and address filter in batches.
                ///
                /// # Arguments
                /// - `from_block`: Starting block number.
                /// - `to_block`: Ending block number.
                /// - `filter`: Address filter for each event type.
                ///
                /// # Returns
                /// A `Result` containing a tuple of event vectors for each event type.
                pub async fn query(
                    &self,
                    from_block: u64,
                    to_block: u64,
                    filter: &AddressFilter,
                ) -> Result<($(Vec<EventExt<$event_type>>,)+)> {
                    // Validate block range
                    if to_block < from_block {
                        return Err(eyre::eyre!("Invalid block range: `to_block` must be greater than `from_block`"));
                    }
                    let addresses = filter.build();
                    // Combine all addresses into a single vector for filtering
                    let all_addresses: Vec<Address> = addresses.iter().flatten().cloned().collect();
                    let block_ranges = get_block_ranges(from_block, to_block, self.max_block_span);
                    let mut logs = Vec::new();

                    // Process block ranges in chunks to respect concurrency limit
                    for chunk in block_ranges.chunks(self.concurrent_tasks_limit) {
                        let mut tasks = FuturesUnordered::new();

                        // Create tasks for each block range in the chunk
                        for &(chunk_from, chunk_to) in chunk {
                            let provider = self.provider.clone();
                            let all_addresses = all_addresses.clone();
                            let provider_name = stringify!($mod_name);
                            tasks.push(async move {
                                retry_with_backoff(
                                    || async {
                                        tracing::info!("Querying logs in {} from block {} to {}", provider_name, chunk_from, chunk_to);
                                        let filter = Filter::new()
                                            .address(all_addresses.clone())
                                            .from_block(chunk_from)
                                            .to_block(chunk_to);

                                        provider.get_logs(&filter).await.map_err(|e| {
                                            eyre::eyre!(
                                                "Failed to fetch logs from block {} to {}: {}",
                                                chunk_from,
                                                chunk_to,
                                                e
                                            )
                                        })
                                    },
                                    MAX_RETRY_ATTEMPTS,
                                    DEFAULT_RETRY_DELAY_MS,
                                )
                                .await
                                .map_err(|e| {
                                    eyre::eyre!(
                                        "Failed to query logs in {}: {}",
                                        provider_name,
                                        e
                                    )
                                })
                            });
                        }

                        // Collect logs from all tasks in the chunk
                        while let Some(result) = tasks.next().await {
                            logs.extend(result?);
                        }
                    }

                    // Decode logs for each event type
                    let mut index = 0;
                    Ok((
                        $(
                            {
                                let events = process_logs::<$event_type>(&logs, &addresses[index]);
                                index += 1;
                                events
                            },
                        )+
                    ))
                }
            }
        }
    };
}

#[macro_export]
/// Counts the number of event types at compile time for array sizing.
macro_rules! count_types {
    () => (0usize);
    ($head:ty $(, $tail:ty)*) => (1usize + count_types!($($tail),*));
}

#[cfg(test)]
mod tests {

    use super::prelude::*;
    use crate::{
        events::primitives::EventExt, UnipayHTLC::UnipayHTLCEvents,
        UnipayHTLCv2::UnipayHTLCv2Events, UnipayHTLCv3::UnipayHTLCv3Events,
    };
    use alloy::providers::{Provider, ProviderBuilder};
    use std::sync::Arc;

    // Generate event provider for Unipay HTLC contracts (V1, V2, V3)
    event_provider!(
        unipay_htlc,
        (htlc_v1, UnipayHTLCEvents),
        (htlc_v2, UnipayHTLCv2Events),
        (htlc_v3, UnipayHTLCv3Events)
    );

    const ARB_SEPOLIA_URL: &str = "https://arbitrum-sepolia.drpc.org";
    const V3_HTLC_ADDR: &str = "0xb8cEf87D2E4521d24627322FBE773D4F7e91c95E";

    /// Creates a provider for the Arbitrum Sepolia testnet.
    fn get_provider() -> impl Provider {
        ProviderBuilder::new().connect_http(ARB_SEPOLIA_URL.parse().unwrap())
    }

    /// Sets up an event provider for testing.
    async fn setup_event_provider() -> unipay_htlc::EventProvider {
        let provider = Arc::new(get_provider());
        unipay_htlc::EventProvider::new(Arc::new(provider), Some(9000), None)
    }

    /// Fetches events for the specified block range.
    async fn fetch_events(
        from_block: u64,
        to_block: u64,
    ) -> (
        Vec<EventExt<UnipayHTLCEvents>>,
        Vec<EventExt<UnipayHTLCv2Events>>,
        Vec<EventExt<UnipayHTLCv3Events>>,
    ) {
        let event_provider = setup_event_provider().await;
        let filter = unipay_htlc::AddressFilter::new()
            .htlc_v1(vec![])
            .htlc_v2(vec![])
            .htlc_v3(vec![V3_HTLC_ADDR.parse().unwrap()]);

        event_provider
            .query(from_block, to_block, &filter)
            .await
            .expect("Failed to fetch events")
    }

    /// Tests event fetching over a large block span.
    #[tokio::test]
    async fn should_handle_multiple_events_with_large_block_span() {
        let _ = tracing_subscriber::fmt().try_init();
        tracing::info!("Testing block span {}", 181806279 - 181213666);
        let (_, _, events) = fetch_events(181213666, 181806279).await;
        let init_count = events
            .iter()
            .filter(|e| matches!(e.event, UnipayHTLCv3Events::Initiated(_)))
            .count();
        let redeemed_count = events
            .iter()
            .filter(|e| matches!(e.event, UnipayHTLCv3Events::Redeemed(_)))
            .count();
        assert_eq!(init_count, 14);
        assert_eq!(redeemed_count, 11);
    }

    /// Tests event fetching over an extremely large block span.
    #[tokio::test]
    async fn should_handle_extreme_block_span() {
        let _ = tracing_subscriber::fmt().try_init();
        tracing::info!("Testing block span {}", 181806279 - 178777490);
        let (_, _, events) = fetch_events(178777490, 181806279).await;
        let init_count = events
            .iter()
            .filter(|e| matches!(e.event, UnipayHTLCv3Events::Initiated(_)))
            .count();
        assert!(init_count >= 27);
    }

    /// Tests block range generation.
    #[test]
    fn test_get_block_ranges() {
        let block_ranges = get_block_ranges(1000, 10000, 5000);
        assert_eq!(block_ranges.len(), 2);
        assert_eq!(block_ranges[0], (1000, 6000));
        assert_eq!(block_ranges[1], (6001, 10000));

        let block_ranges = get_block_ranges(1000, 10000, 10000);
        assert_eq!(block_ranges.len(), 1);
        assert_eq!(block_ranges[0], (1000, 10000));

        let block_ranges = get_block_ranges(1000, 10000, 100000);
        assert_eq!(block_ranges.len(), 1);
        assert_eq!(block_ranges[0], (1000, 10000));
    }

    /// Asserts properties of an `Initiated` event.
    fn assert_initiated_event(
        event: &UnipayHTLCv3Events,
        order_id: &str,
        secret_hash: &str,
        amount: &str,
    ) {
        match event {
            UnipayHTLCv3Events::Initiated(event) => {
                assert_eq!(event.orderID.to_string(), order_id);
                assert_eq!(event.secretHash.to_string(), secret_hash);
                assert_eq!(event.amount.to_string(), amount);
            }
            _ => panic!("Expected Initiated event"),
        }
    }

    /// Asserts properties of a `Redeemed` event.
    fn assert_redeemed_event(
        event: &UnipayHTLCv3Events,
        order_id: &str,
        secret: &str,
        secret_hash: &str,
    ) {
        match event {
            UnipayHTLCv3Events::Redeemed(event) => {
                assert_eq!(event.orderID.to_string(), order_id);
                assert_eq!(event.secret.to_string(), secret);
                assert_eq!(event.secretHash.to_string(), secret_hash);
            }
            _ => panic!("Expected Redeemed event"),
        }
    }

    /// Tests fetching of `Initiated` events.
    #[tokio::test]
    async fn should_provide_initiated_events() {
        let (_, _, v3_events) = fetch_events(181798961, 181798962).await;
        assert_eq!(v3_events.len(), 1);
        assert_initiated_event(
            &v3_events[0].event,
            "0x080b717eb2abd8b592034fc5bfd1e79f3b24dc2b473cd79745937dfe6558c74e",
            "0x10eddb10909eccd48ba0ec646b20d99ba1390038b6bef5193896a72a0f4bc513",
            "1000",
        );
    }

    /// Tests fetching of `Redeemed` events.
    #[tokio::test]
    async fn should_provide_redeemed_events() {
        let (_, _, events) = fetch_events(181545214, 181545214).await;
        assert_eq!(events.len(), 1);
        assert_redeemed_event(
            &events[0].event,
            "0xb67dbc2159edc2869f7b959ba4f14daf0fa6e9ac4c27e21cc07eaaef43c735eb",
            "0xbfa4f12544907c6ac1c5237853c5ab8a7b660606936bc19baf0b483012062aac",
            "0x3a5de24f754cc3f8dfc7f208b269f63ab72f4d7d83dd23c49450311de9672b2a",
        );
    }

    /// Tests fetching multiple event types in a block range.
    #[tokio::test]
    async fn should_handle_multiple_events() {
        let (_, _, events) = fetch_events(181805278, 181806279).await;
        let init_count = events
            .iter()
            .filter(|e| matches!(e.event, UnipayHTLCv3Events::Initiated(_)))
            .count();
        let redeemed_count = events
            .iter()
            .filter(|e| matches!(e.event, UnipayHTLCv3Events::Redeemed(_)))
            .count();
        assert_eq!(init_count, 4);
        assert_eq!(redeemed_count, 1);
    }
}
