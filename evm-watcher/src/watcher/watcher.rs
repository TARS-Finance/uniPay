use crate::{
    block_range,
    onchain_orders::{
        OnChainOrders,
        primitives::{OnChainOrder, OnchainRequest},
    },
    prepare_event,
    primitives::{BlockRange, OrderStatus},
    stream_event::{self, garden_htlc},
    swaps::{SwapEvent, SwapEventType, SwapStore},
    validate_event, validate_swap,
    watcher::{multi::MultiChainSwapsProvider, primitives::ChainConfig},
};
use alloy::providers::Provider;
use futures::StreamExt;
use tars::{orderbook::primitives::SingleSwap, utils::NonEmptyVec};
use sqlx::types::BigDecimal;
use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    sync::Arc,
};
use tracing::warn;

/// Watches for HTLC events across configured blockchain
pub struct HTLCEventWatcher {
    pending_swaps: Arc<MultiChainSwapsProvider>,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
    onchain_orders_provider: Arc<dyn OnChainOrders + Send + Sync>,
    event_provider: Arc<garden_htlc::EventProvider>,
    block_chain_provider: Arc<dyn Provider>,
    chain_config: ChainConfig,
    previous_block: u64,
}

impl HTLCEventWatcher {
    /// Creates a new HTLCEventWatcher instance
    pub fn new(
        pending_swaps: Arc<MultiChainSwapsProvider>,
        swap_store: Arc<dyn SwapStore + Send + Sync>,
        onchain_orders_provider: Arc<dyn OnChainOrders + Send + Sync>,
        event_provider: Arc<garden_htlc::EventProvider>,
        block_chain_provider: Arc<dyn Provider>,
        chain_config: ChainConfig,
    ) -> Self {
        Self {
            pending_swaps,
            swap_store,
            onchain_orders_provider,
            event_provider,
            block_chain_provider,
            chain_config,
            previous_block: 0,
        }
    }

    /// Starts the event watcher with continuous polling
    pub async fn start(&mut self) -> eyre::Result<()> {
        tracing::info!("Starting watcher for chain: '{}'", self.chain_config.name);
        loop {
            self.chain_config.polling_interval.tick().await;
            if let Err(err) = self.watch().await {
                tracing::error!(
                    "Error watching for chain '{}': {}",
                    self.chain_config.name,
                    err
                );
            }
        }
    }

    /// Handles the processing of swap events for the configured blockchain.
    ///
    /// Steps:
    /// 1. Retrieve all pending swaps for the current chain using the MultiChainSwapsProvider.
    /// 2. Obtain on-chain orders that correspond to these pending swaps.
    /// 3. Validate swaps by:
    ///    - Fetching the latest block number from the blockchain provider.
    ///    - Filtering both pending swaps and on-chain orders based on the current block and their status
    /// 4. Process relevant events by:
    ///    a. Identifying blocks containing events and group them to blockranges using max block span.
    ///    b. Streaming on-chain events within those ranges.
    ///    c. Preparing swap events for store.
    ///    d. Validating swap events against the set of pending swaps.
    ///    e. Updating the event store and collecting confirmations for init events.
    ///    f. Updating confirmations in the store as needed.
    async fn watch(&mut self) -> eyre::Result<()> {
        tracing::info!("Watching for chain: '{}' ", self.chain_config.name);
        // Fetch pending swaps
        let pending_swaps = match self
            .pending_swaps
            .get_swaps(&self.chain_config.name)
            .await?
        {
            Some(swaps) if !swaps.is_empty() => swaps,
            _ => return Ok(()),
        };
        tracing::info!(chain = %self.chain_config.name, pending_swaps = %pending_swaps.len(), "Fetched pending swaps");

        // Fetch onchain orders
        let onchain_orders = self.fetch_onchain_orders(&pending_swaps).await?;
        if onchain_orders.is_empty() {
            return Ok(());
        }

        tracing::info!(chain = %self.chain_config.name, onchain_orders = %onchain_orders.len(), "Fetched onchain orders");

        // Validate Swaps
        let current_block = self.block_chain_provider.get_block_number().await?;
        if current_block < self.previous_block {
            warn!(
                "current block: '{}' is less than previous processed block: '{}'",
                current_block, self.previous_block
            );
            return Ok(());
        }

        let (pending_swaps, onchain_orders) =
            Self::validate_swaps(&pending_swaps, onchain_orders, current_block);
        tracing::info!(chain = %self.chain_config.name, pending_swaps = %pending_swaps.len(), onchain_orders = %onchain_orders.len(), "Validated swaps");

        if pending_swaps.is_empty() || onchain_orders.is_empty() {
            return Ok(());
        }

        // handle Events
        self.process_events(&pending_swaps, onchain_orders, current_block)
            .await?;
        self.previous_block = current_block;
        Ok(())
    }

    /// Filters and returns only those swaps that are eligible for processing by comparing the status
    /// of on-chain orders with pending swaps. Includes swaps where there is a status mismatch or
    /// where the 'initiated' event does not yet have the required number of confirmations.
    fn validate_swaps(
        pending_swaps: &[SingleSwap],
        onchain_orders: HashMap<String, OnChainOrder>,
        current_block: u64,
    ) -> (Vec<&SingleSwap>, HashMap<String, OnChainOrder>) {
        // Validate swaps
        let onchain_order_status: HashMap<_, _> = onchain_orders
            .iter()
            .map(|(k, v)| (k.clone(), OrderStatus::from(v)))
            .collect();
        let pending_swaps_map: HashMap<_, _> = pending_swaps
            .iter()
            .map(|swap| (swap.swap_id.deref().to_string(), swap))
            .collect();
        let valid_swap_ids: HashSet<_> =
            validate_swap::validate_swaps(&onchain_order_status, &pending_swaps_map, current_block)
                .into_iter()
                .collect();

        // Filter valid swaps and orders
        let pending_swaps: Vec<_> = pending_swaps
            .iter()
            .filter(|swap| valid_swap_ids.contains(&swap.swap_id.deref().to_string()))
            .collect();
        let onchain_orders: HashMap<_, _> = onchain_orders
            .into_iter()
            .filter(|(k, _)| valid_swap_ids.contains(k))
            .collect();

        (pending_swaps, onchain_orders)
    }

    /// Generates grouped block ranges from the provided onchain orders to optimize event queries.
    fn create_event_blocks_ranges(
        onchain_orders: &HashMap<String, OnChainOrder>,
        max_block_span: u64,
    ) -> Option<NonEmptyVec<BlockRange>> {
        let block_numbers: Vec<u64> = onchain_orders
            .iter()
            .flat_map(|(_, order)| {
                [&order.initiated_at, &order.fulfilled_at]
                    .into_iter()
                    .filter_map(|b| {
                        if b > &&BigDecimal::from(0) {
                            b.to_string().parse::<u64>().ok()
                        } else {
                            None
                        }
                    })
            })
            .collect();

        let grouped = block_range::group_block_ranges(block_numbers, max_block_span);

        let filtered: Vec<BlockRange> = grouped
            .into_iter()
            .filter(|block_range| !block_range.is_empty())
            .collect();

        NonEmptyVec::new(filtered).ok()
    }

    /// Processes events for valid swaps
    async fn process_events(
        &self,
        pending_swaps: &[&SingleSwap],
        onchain_orders: HashMap<String, OnChainOrder>,
        current_block: u64,
    ) -> eyre::Result<()> {
        // Create event blocks ranges
        let Some(event_blocks) =
            Self::create_event_blocks_ranges(&onchain_orders, self.chain_config.max_block_span)
        else {
            return Ok(());
        };

        // Get Event Stream Reciever
        let mut event_stream = stream_event::stream_garden_events(
            self.event_provider.clone(),
            &event_blocks,
            garden_htlc::AddressFilter::new()
                .htlc_v2(self.chain_config.contract_addresses.v2.clone())
                .htlc_v3(self.chain_config.contract_addresses.v3.clone()),
        )
        .await;

        // Collect confirmations of init events for batch udpates
        let mut confirmations_map = HashMap::new();
        // Create required confirmations map used to collect confirmations for init events
        let required_confirmations_map: HashMap<_, _> = pending_swaps
            .iter()
            .map(|swap| {
                (
                    swap.swap_id.deref().to_string(),
                    swap.required_confirmations,
                )
            })
            .collect();

        // Process event stream
        while let Some(result) = event_stream.next().await {
            // collect swap events for store update
            let swap_events = match result {
                Ok((v2_events, v3_events)) => {
                    prepare_event::prepare_store_events(
                        &self.chain_config.name,
                        self.block_chain_provider.clone(),
                        &v2_events,
                        &v3_events,
                        &onchain_orders,
                    )
                    .await?
                }
                Err(e) => return Err(eyre::eyre!("Error handling event stream: {}", e)),
            };
            tracing::info!(chain = %self.chain_config.name, swap_events = %swap_events.len(), "Prepared swap events");

            // Validate and store events
            let validated_swap_events: Vec<_> =
                validate_event::validate_events(pending_swaps, &swap_events)
                    .into_iter()
                    .collect();

            // Update events in store and collect confirmations for batch update
            if let Ok(validated_events) = NonEmptyVec::new(validated_swap_events) {
                // update events in store
                self.swap_store
                    .update_events(&validated_events)
                    .await
                    .map_err(|e| eyre::eyre!("Error updating swap events: {}", e))?;

                // Collect confirmations
                confirmations_map = Self::collect_initiated_event_confirmations(
                    &validated_events,
                    confirmations_map,
                    &required_confirmations_map,
                    current_block,
                );
            }
        }

        // Store confirmations
        self.store_confirmations(confirmations_map).await
    }

    /// Store confirmations
    async fn store_confirmations(
        &self,
        confirmations_map: HashMap<i64, Vec<String>>,
    ) -> eyre::Result<()> {
        let confirmations_map_refs: HashMap<_, _> = confirmations_map
            .iter()
            .map(|(k, v)| (*k as i64, v.as_slice()))
            .collect();
        self.swap_store
            .update_confirmations(confirmations_map_refs)
            .await
            .map_err(|e| eyre::eyre!("Error updating swap confirmations: {}", e))?;
        Ok(())
    }

    /// Collect confirmations for initiate events
    fn collect_initiated_event_confirmations(
        validated_events: &NonEmptyVec<&SwapEvent>,
        mut current_conf_map: HashMap<i64, Vec<String>>,
        required_confirmations_map: &HashMap<String, i32>,
        current_block: u64,
    ) -> HashMap<i64, Vec<String>> {
        for (confirmations, swap_id) in validated_events
            .iter()
            .filter(|event| matches!(event.event_type, SwapEventType::Initiate))
            .filter_map(|event| {
                let event_block = event.tx_info.block_number.to_string().parse::<u64>().ok()?;
                let required_confirmations =
                    *required_confirmations_map.get(event.swap_id.as_str())? as u64;
                Some((
                    (current_block + 1)
                        .saturating_sub(event_block)
                        .min(required_confirmations),
                    event.swap_id.to_string(),
                ))
            })
        {
            current_conf_map
                .entry(confirmations as i64)
                .or_insert_with(Vec::new)
                .push(swap_id);
        }
        current_conf_map
    }

    /// Fetches and filters valid onchain orders for the given swaps
    async fn fetch_onchain_orders(
        &self,
        swaps: &[SingleSwap],
    ) -> eyre::Result<HashMap<String, OnChainOrder>> {
        // Create onchain requests
        let requests: Vec<_> = swaps
            .iter()
            .filter_map(|swap| {
                OnchainRequest::try_from(swap)
                    .map_err(|e| {
                        tracing::warn!(
                            "Failed to convert swap '{}' to OnchainRequest: {}",
                            swap.swap_id,
                            e
                        );
                        e
                    })
                    .ok()
            })
            .collect();

        if requests.is_empty() {
            return Ok(HashMap::new());
        }

        // Fetch onchain orders
        let orders = self
            .onchain_orders_provider
            .get_orders(&requests)
            .await
            .map_err(|e| {
                tracing::error!(
                    "Failed to fetch onchain orders for chain '{}': {}",
                    self.chain_config.name,
                    e
                );
                e
            })?;

        // Filter valid onchain orders
        Ok(orders
            .into_iter()
            .zip(swaps.iter())
            .filter_map(|(order_option, swap)| {
                order_option
                    .filter(|order| !order.is_empty())
                    .map(|order| (swap.swap_id.deref().to_string(), order))
                    .or_else(|| {
                        tracing::debug!(
                            "No valid onchain order found for swap: '{}'",
                            swap.swap_id
                        );
                        None
                    })
            })
            .collect())
    }
}
