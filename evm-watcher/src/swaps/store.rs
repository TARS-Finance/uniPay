use crate::swaps::{SwapEvent, SwapEventType};
use alloy::primitives::map::HashSet;
use async_trait::async_trait;
use eyre::Result;
use tars::{orderbook::primitives::SingleSwap, utils::NonEmptyVec};
use sqlx::{Pool, Postgres, QueryBuilder};
use std::collections::HashMap;

#[async_trait]
pub trait SwapStore {
    /// Returns all pending swaps.
    /// The query should also check the deadline, with an additional buffer of 30 minutes.
    async fn get_swaps(&self) -> Result<Vec<SingleSwap>>;

    /// Updates the store with new swap events.
    ///
    /// # Arguments
    /// * `swaps` - A non-empty vector of `SwapEvent` to update in the store.
    async fn update_events(&self, swaps: &NonEmptyVec<&SwapEvent>) -> Result<()>;

    /// Updates the confirmations for swaps in the store.
    ///
    /// # Arguments
    /// * `swap_confs` - A map from confirmations in i64 to string of swap_ids
    async fn update_confirmations<'a>(&self, swap_confs: HashMap<i64, &'a [String]>) -> Result<()>;
}

pub struct GardenSwapStore {
    pool: Pool<Postgres>,
    ignore_chains: Vec<String>,
    supported_assets: Option<Vec<String>>,
    deadline_buffer: i64, // in seconds
}

// Migrates indexes for the swaps table if they do not exist.
async fn migrate_indexes(pool: &Pool<Postgres>) -> Result<()> {
    let indexes = [
        "CREATE INDEX IF NOT EXISTS idx_swaps_chain_initiate_created ON swaps (chain, initiate_tx_hash, created_at DESC);",
        "CREATE INDEX IF NOT EXISTS idx_swaps_initiate_redeem_refund ON swaps (initiate_tx_hash, redeem_tx_hash, refund_tx_hash);",
        "CREATE INDEX IF NOT EXISTS idx_swaps_empty_initiate_created ON swaps (created_at DESC) WHERE initiate_tx_hash = '';",
        "CREATE INDEX IF NOT EXISTS idx_swaps_pending_transactions ON swaps (initiate_tx_hash, redeem_tx_hash, refund_tx_hash) WHERE initiate_tx_hash != '' AND redeem_tx_hash = '' AND refund_tx_hash = '';",
        "CREATE INDEX IF NOT EXISTS idx_swaps_created_at_desc ON swaps (created_at DESC);",
    ];
    for sql in indexes {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}
impl GardenSwapStore {
    // Constructs a new GardenSwapStore and migrates indexes.
    pub async fn new(
        pool: Pool<Postgres>,
        ignore_chains: Vec<String>,
        deadline_buffer: i64,
        supported_assets: Option<Vec<String>>,
    ) -> Self {
        migrate_indexes(&pool)
            .await
            .expect("Failed to migrate indexes");
        let ignore_chains = ignore_chains.iter().map(|c| c.to_lowercase()).collect();
        let supported_assets =
            supported_assets.map(|assets| assets.iter().map(|a| a.to_lowercase()).collect());
        GardenSwapStore {
            pool,
            ignore_chains,
            deadline_buffer,
            supported_assets,
        }
    }

    // Constructs a new GardenSwapStore from a database URL.
    pub async fn from_db_url(
        db_url: &str,
        ignore_chains: Vec<String>,
        deadline_buffer: i64,
        supported_assets: Option<Vec<String>>,
    ) -> Result<Self> {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2000)
            .connect(db_url)
            .await?;
        Ok(Self::new(pool, ignore_chains, deadline_buffer, supported_assets).await)
    }

    // Helper function to log batch update results and debug details
    async fn log_batch_update(
        &self,
        event_type: &str,
        events: &NonEmptyVec<&SwapEvent>,
        actual_updates: usize,
    ) {
        //expected updates are unique swap_ids
        let expected_updates = events
            .as_ref()
            .iter()
            .map(|event| event.swap_id.as_str())
            .collect::<HashSet<_>>()
            .len();
        if actual_updates != expected_updates {
            tracing::warn!(
                "Batch {} update mismatch: expected {} updates, but {} rows were affected. Some swaps may not exist or have mismatched conditions.",
                event_type,
                expected_updates,
                actual_updates
            );
            for event in events.as_ref().iter() {
                tracing::debug!(
                    "Invalid {} update batch details - swap_id: {}, redeemer: {}, asset: {}, chain: {}, timelock: {}",
                    event_type,
                    event.swap_id.as_str(),
                    event.order.redeemer,
                    event.order.asset_address,
                    event.order.chain,
                    event.order.timelock
                );
            }
            return;
        }

        tracing::info!(
            "Successfully batch updated {} swaps with {} events",
            actual_updates,
            event_type
        );
    }

    // Handles initiate events with a single batch update to the swaps table.
    async fn handle_inits(&self, events: &NonEmptyVec<&SwapEvent>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let events_len = events.len();
        // Prepare batch data
        let mut filled_amounts = Vec::with_capacity(events_len);
        let mut initiate_tx_hashes = Vec::with_capacity(events_len);
        let mut initiate_block_numbers = Vec::with_capacity(events_len);
        let mut initiate_timestamps = Vec::with_capacity(events_len);
        let mut swap_ids = Vec::with_capacity(events_len);
        let mut redeemers = Vec::with_capacity(events_len);
        let mut asset_addresses = Vec::with_capacity(events_len);
        let mut chains = Vec::with_capacity(events_len);
        let mut timelocks = Vec::with_capacity(events_len);

        for e in events.as_ref() {
            filled_amounts.push(&e.order.amount);
            initiate_tx_hashes.push(&e.tx_info.tx_hash);
            initiate_block_numbers.push(&e.tx_info.block_number);
            initiate_timestamps.push(&e.tx_info.timestamp);
            swap_ids.push(e.swap_id.as_str());
            redeemers.push(e.order.redeemer.to_lowercase());
            asset_addresses.push(e.order.asset_address.to_lowercase());
            chains.push(e.order.chain.as_str());
            timelocks.push(&e.order.timelock);
        }

        let q = "
        UPDATE swaps SET 
            filled_amount = d.filled_amount,
            initiate_tx_hash = d.initiate_tx_hash,
            initiate_block_number = d.initiate_block_number,
            initiate_timestamp = d.initiate_timestamp,
            current_confirmations = 1
        FROM (
            SELECT * FROM UNNEST(
                $1::NUMERIC[], $2::TEXT[], $3::BIGINT[], $4::timestamptz[], 
                $5::TEXT[], $6::TEXT[], $7::TEXT[], $8::TEXT[], $9::BIGINT[]
            ) AS t(filled_amount, initiate_tx_hash, initiate_block_number, initiate_timestamp, swap_id, redeemer, asset_address, chain, timelock)
        ) d
        WHERE LOWER(swaps.swap_id)=LOWER(d.swap_id)
        AND LOWER(swaps.redeemer)=LOWER(d.redeemer)
        AND LOWER(swaps.asset)=LOWER(d.asset_address)
        AND LOWER(swaps.chain)=LOWER(d.chain)
        AND swaps.timelock=d.timelock
        AND (d.initiate_timestamp IS NULL OR d.initiate_timestamp > swaps.created_at - INTERVAL '1 minute')";

        let res = sqlx::query(q)
            .bind(&filled_amounts)
            .bind(&initiate_tx_hashes)
            .bind(&initiate_block_numbers)
            .bind(&initiate_timestamps)
            .bind(&swap_ids)
            .bind(&redeemers)
            .bind(&asset_addresses)
            .bind(&chains)
            .bind(&timelocks)
            .execute(&mut *tx)
            .await?;

        self.log_batch_update("initiate", events, res.rows_affected() as usize)
            .await;
        tx.commit().await?;
        Ok(())
    }

    // Handles redeem events and updates the swaps table accordingly using batch update.
    async fn handle_redeems(&self, events: &NonEmptyVec<(&SwapEvent, String)>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let events_len = events.len();

        // Prepare batch data
        let mut redeem_tx_hashes = Vec::with_capacity(events_len);
        let mut redeem_block_numbers = Vec::with_capacity(events_len);
        let mut redeem_timestamps = Vec::with_capacity(events_len);
        let mut secrets = Vec::with_capacity(events_len);
        let mut swap_ids = Vec::with_capacity(events_len);
        let mut redeemers = Vec::with_capacity(events_len);
        let mut asset_addresses = Vec::with_capacity(events_len);
        let mut chains = Vec::with_capacity(events_len);
        let mut timelocks = Vec::with_capacity(events_len);
        let mut redeem_events = Vec::with_capacity(events_len);

        for (event, secret) in events.as_ref().iter() {
            redeem_tx_hashes.push(&event.tx_info.tx_hash);
            redeem_block_numbers.push(&event.tx_info.block_number);
            redeem_timestamps.push(&event.tx_info.timestamp);
            secrets.push(secret);
            swap_ids.push(event.swap_id.as_str());
            redeemers.push(event.order.redeemer.clone().to_lowercase());
            asset_addresses.push(event.order.asset_address.clone().to_lowercase());
            chains.push(event.order.chain.clone());
            timelocks.push(&event.order.timelock);
            redeem_events.push(*event);
        }

        let batch_update_query = "
        UPDATE swaps 
        SET redeem_tx_hash = data_table.redeem_tx_hash,
            redeem_block_number = data_table.redeem_block_number,
            redeem_timestamp = data_table.redeem_timestamp,
            secret = data_table.secret
        FROM (
            SELECT * FROM UNNEST($1::TEXT[], $2::BIGINT[], $3::timestamptz[], $4::TEXT[], $5::TEXT[], $6::TEXT[], $7::TEXT[], $8::TEXT[], $9::BIGINT[])
            AS t(redeem_tx_hash, redeem_block_number, redeem_timestamp, secret, swap_id, redeemer, asset_address, chain, timelock)
        ) AS data_table
        WHERE LOWER(swaps.swap_id) = LOWER(data_table.swap_id)
        AND LOWER(swaps.redeemer) = LOWER(data_table.redeemer)
        AND LOWER(swaps.asset) = LOWER(data_table.asset_address)
        AND LOWER(swaps.chain) = LOWER(data_table.chain)
        AND swaps.timelock = data_table.timelock";

        let res = sqlx::query(batch_update_query)
            .bind(&redeem_tx_hashes)
            .bind(&redeem_block_numbers)
            .bind(&redeem_timestamps)
            .bind(&secrets)
            .bind(&swap_ids)
            .bind(&redeemers)
            .bind(&asset_addresses)
            .bind(&chains)
            .bind(&timelocks)
            .execute(&mut *tx)
            .await?;

        let actual_updates = res.rows_affected() as usize;
        self.log_batch_update(
            "redeem",
            &NonEmptyVec::try_from(redeem_events).unwrap(),
            actual_updates,
        )
        .await;

        tx.commit().await?;
        Ok(())
    }

    // Handles refund events and updates the swaps table accordingly using batch update.
    async fn handle_refunds(&self, events: &NonEmptyVec<&SwapEvent>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let events_len = events.len();

        // Prepare batch data
        let mut refund_tx_hashes = Vec::with_capacity(events_len);
        let mut refund_block_numbers = Vec::with_capacity(events_len);
        let mut refund_timestamps = Vec::with_capacity(events_len);
        let mut swap_ids = Vec::with_capacity(events_len);
        let mut redeemers = Vec::with_capacity(events_len);
        let mut asset_addresses = Vec::with_capacity(events_len);
        let mut chains = Vec::with_capacity(events_len);
        let mut timelocks = Vec::with_capacity(events_len);

        for event in events.as_ref().iter() {
            refund_tx_hashes.push(&event.tx_info.tx_hash);
            refund_block_numbers.push(&event.tx_info.block_number);
            refund_timestamps.push(&event.tx_info.timestamp);
            swap_ids.push(event.swap_id.as_str());
            redeemers.push(event.order.redeemer.clone().to_lowercase());
            asset_addresses.push(event.order.asset_address.clone().to_lowercase());
            chains.push(event.order.chain.clone());
            timelocks.push(&event.order.timelock);
        }

        let batch_update_query = "
        UPDATE swaps 
        SET refund_tx_hash = data_table.refund_tx_hash,
            refund_block_number = data_table.refund_block_number,
            refund_timestamp = data_table.refund_timestamp
        FROM (
            SELECT * FROM UNNEST($1::TEXT[], $2::BIGINT[], $3::timestamptz[], $4::TEXT[], $5::TEXT[], $6::TEXT[], $7::TEXT[], $8::BIGINT[])
            AS t(refund_tx_hash, refund_block_number, refund_timestamp, swap_id, redeemer, asset_address, chain, timelock)
        ) AS data_table
        WHERE LOWER(swaps.swap_id) = LOWER(data_table.swap_id)
        AND LOWER(swaps.redeemer) = LOWER(data_table.redeemer)
        AND LOWER(swaps.asset) = LOWER(data_table.asset_address)    
        AND LOWER(swaps.chain) = LOWER(data_table.chain)
        AND swaps.timelock = data_table.timelock";

        let res = sqlx::query(batch_update_query)
            .bind(&refund_tx_hashes)
            .bind(&refund_block_numbers)
            .bind(&refund_timestamps)
            .bind(&swap_ids)
            .bind(&redeemers)
            .bind(&asset_addresses)
            .bind(&chains)
            .bind(&timelocks)
            .execute(&mut *tx)
            .await?;

        let actual_updates = res.rows_affected() as usize;
        self.log_batch_update("refund", events, actual_updates)
            .await;

        tx.commit().await?;
        Ok(())
    }
}
#[async_trait]
impl SwapStore for GardenSwapStore {
    /// Fetches all pending swaps from the database.
    ///
    /// The query does the following:
    /// - Selects all columns from the `swaps` table.
    /// - Filters out swaps whose `chain` (case-insensitive) is in the `ignore_chains` list.
    /// - If `supported_assets` is set, further restricts to swaps whose `asset` (case-insensitive) is in the supported assets list.
    /// - Then, selects swaps that are "pending", defined as either:
    ///     1. The swap has not been initiated yet (`initiate_tx_hash` is empty string) and its creation time plus a buffer (in seconds) is still in the future (i.e., not expired).
    ///     2. The swap has been initiated (`initiate_tx_hash` is not empty), but neither redeemed nor refunded (`redeem_tx_hash` and `refund_tx_hash` are both empty).
    async fn get_swaps(&self) -> Result<Vec<SingleSwap>> {
        // Start building the SQL query
        let mut query_builder = QueryBuilder::new(
            r#"
            SELECT s.*
            FROM swaps s
            WHERE NOT (s.chain = ANY("#,
        );
        // Exclude swaps on ignored chains
        query_builder.push_bind(&self.ignore_chains);

        // If supported_assets is set, restrict to those assets
        if let Some(assets) = &self.supported_assets {
            query_builder.push(r#")) AND (LOWER(s.asset) = ANY("#);
            query_builder.push_bind(assets);
        }
        query_builder.push(r#"))"#);

        // Pending swaps logic:
        // 1. Not yet initiated and not expired (created_at + buffer > now)
        // 2. Initiated but not yet redeemed or refunded
        query_builder.push(
            r#"
            AND (
                (s.initiate_tx_hash = ''
                AND s.created_at + ("#,
        );
        query_builder.push_bind(self.deadline_buffer);
        query_builder.push(
            r#" * INTERVAL '1 second') > NOW())
                OR
                (s.initiate_tx_hash != ''
                AND s.redeem_tx_hash = ''
                AND s.refund_tx_hash = '')
                );
            "#,
        );

        // Execute the query and return the results
        let swaps = query_builder
            .build_query_as::<SingleSwap>()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| eyre::eyre!("Failed to fetch pending swaps: {}", e))?;
        Ok(swaps)
    }

    // Updates the store with new swap events.
    async fn update_events(&self, swaps: &NonEmptyVec<&SwapEvent>) -> Result<()> {
        let (mut inits, mut redeems, mut refunds) = (Vec::new(), Vec::new(), Vec::new());
        for event in swaps.as_ref().iter() {
            let event_type = event.event_type.clone();
            match event_type {
                SwapEventType::Initiate => inits.push(*event),
                SwapEventType::Refund => refunds.push(*event),
                SwapEventType::Redeem(secret) => {
                    redeems.push((*event, secret.as_str().to_string()))
                }
            }
        }

        if let Ok(inits) = NonEmptyVec::try_from(inits) {
            self.handle_inits(&inits).await?;
        }
        if let Ok(redeems) = NonEmptyVec::try_from(redeems) {
            self.handle_redeems(&redeems).await?;
        }
        if let Ok(refunds) = NonEmptyVec::try_from(refunds) {
            self.handle_refunds(&refunds).await?;
        }

        Ok(())
    }

    // Updates the confirmations for swaps in the store.
    async fn update_confirmations<'a>(&self, swap_confs: HashMap<i64, &'a [String]>) -> Result<()> {
        for (conf, swap_ids) in swap_confs {
            let update_query = "UPDATE swaps
                SET current_confirmations = LEAST(required_confirmations, $1)
                WHERE swap_id = ANY($2)";
            sqlx::query(update_query)
                .bind(conf)
                .bind(&swap_ids)
                .execute(&self.pool)
                .await
                .map_err(|e| {
                    eyre::eyre!(
                        "Failed to update confirmations for swaps {:?}: {}",
                        swap_ids,
                        e
                    )
                })?;
        }
        Ok(())
    }
}

/// Groups a slice of items by their associated chain.
///
/// # Arguments
///
/// * `swaps` - A slice of items implementing the `HasChain` trait.
///
/// # Returns
///
/// A `HashMap` where the key is the chain and the value is a vector of items belonging to that chain.
pub fn group_by_chains<T>(swaps: &[T]) -> HashMap<T::Chain, Vec<T>>
where
    T: HasChain + Clone,
    T::Chain: Eq + std::hash::Hash,
{
    let mut map: HashMap<T::Chain, Vec<T>> = HashMap::new();
    for swap in swaps {
        // Insert the swap into the vector for its chain, creating the vector if necessary.
        map.entry(swap.chain().clone())
            .or_default()
            .push(swap.clone());
    }
    map
}

/// Trait for types that have an associated chain.
///
/// The associated type `Chain` represents the type of the chain identifier.
pub trait HasChain {
    type Chain: Clone;
    /// Returns a reference to the chain identifier for this item.
    fn chain(&self) -> &Self::Chain;
}

// Implement the HasChain trait for SingleSwap.
// This allows SingleSwap to be grouped by its chain using `group_by_chains`.
impl HasChain for SingleSwap {
    type Chain = String; // The chain field is of type String.

    fn chain(&self) -> &Self::Chain {
        &self.chain
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swaps::{
        GardenSwapStore, HTLCOrder, HasChain, OrderSecret, OrderSwapId, SwapEventType, SwapStore,
        group_by_chains,
    };
    use tars::orderbook::{
        OrderbookProvider,
        primitives::MatchedOrderVerbose,
        test_utils::{
            TestMatchedOrderConfig, TestTxData, create_test_matched_order,
            delete_all_matched_orders, delete_matched_order, simulate_test_swap_initiate,
            simulate_test_swap_redeem, simulate_test_swap_refund,
        },
        traits::Orderbook,
    };
    use serial_test::serial;
    use sqlx::types::{BigDecimal, chrono::Utc};

    pub async fn pool() -> Pool<Postgres> {
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2000)
            .connect("postgres://postgres:postgres@localhost:5432/postgres")
            .await
            .expect("Failed to create pool")
    }

    fn create_test_swap_event(
        swap_id: &str,
        event_type: SwapEventType,
        tx_hash: &str,
        amount: i64,
        block_number: i64,
        redeemer: String,
        timelock: i128,
        asset: String,
        chain: String,
    ) -> SwapEvent {
        let timelock = BigDecimal::from(timelock);
        let test_swap_id = OrderSwapId::from("0x".to_string() + swap_id);
        dbg!(&test_swap_id);
        SwapEvent {
            event_type,
            swap_id: test_swap_id.clone(),
            tx_info: crate::swaps::EventTxInfo {
                tx_hash: tx_hash.to_string(),
                block_number: block_number.into(),
                timestamp: Utc::now(),
            },
            order: HTLCOrder {
                redeemer,
                timelock,
                amount: amount.into(),
                asset_address: asset,
                chain: chain,
            },
        }
    }
    async fn create_matched_order(
        provider: &OrderbookProvider,
        config: TestMatchedOrderConfig,
    ) -> eyre::Result<MatchedOrderVerbose> {
        let order = create_test_matched_order(&provider.pool, config)
            .await
            .unwrap();
        delete_matched_order(&provider.pool, &order.create_order.create_id)
            .await
            .unwrap();
        provider.create_matched_order(&order).await?;
        let order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();
        Ok(order)
    }

    #[tokio::test]
    #[serial]
    async fn test_update_confirmations() {
        let pool = pool().await;
        let provider = OrderbookProvider::new(pool.clone());
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;

        delete_all_matched_orders(&pool).await.unwrap();

        // Create a test order
        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Simulate initiate for source swap
        simulate_test_swap_initiate(
            &pool,
            &order.source_swap.swap_id,
            Some(TestTxData {
                tx_hash: "1234567890123456789012345678901234567890".to_string(),
                block_number: 132,
                filled_amount: BigDecimal::from(10000),
                current_confirmations: 0,
                timestamp: Utc::now(),
            }),
        )
        .await
        .unwrap();

        let source_swap_ids = vec![order.source_swap.swap_id.clone()];
        let destination_swap_ids = vec![order.destination_swap.swap_id.clone()];
        let swap_confs: HashMap<i64, &[String]> = [
            (1, source_swap_ids.as_slice()),
            (0, destination_swap_ids.as_slice()),
        ]
        .iter()
        .cloned()
        .collect();

        // Update confirmations for the swap
        swap_store.update_confirmations(swap_confs).await.unwrap();

        // Verify the swap was updated correctly
        let updated_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();

        assert!(updated_order.source_swap.current_confirmations == 1);
        assert!(updated_order.destination_swap.current_confirmations == 0);

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_empty() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;

        delete_all_matched_orders(&pool).await.unwrap();

        let swaps = swap_store.get_swaps().await.unwrap();
        assert!(swaps.is_empty());
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_with_pending_swaps() {
        let pool = pool().await;
        let provider = OrderbookProvider::new(pool.clone());
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;

        delete_all_matched_orders(&pool).await.unwrap();

        // Create test orders - these will have empty initiate_tx_hash initially
        let order_config1 = TestMatchedOrderConfig::default();
        let order1 = create_matched_order(&provider, order_config1)
            .await
            .unwrap();

        let order_config2 = TestMatchedOrderConfig::default();
        let order2 = create_matched_order(&provider, order_config2)
            .await
            .unwrap();

        let swaps = swap_store.get_swaps().await.unwrap();
        assert_eq!(swaps.len(), 4); // 2 orders × 2 swaps each

        // Verify that our test orders' swaps are included
        let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
        assert!(swap_ids.contains(&order1.source_swap.swap_id));
        assert!(swap_ids.contains(&order1.destination_swap.swap_id));
        assert!(swap_ids.contains(&order2.source_swap.swap_id));
        assert!(swap_ids.contains(&order2.destination_swap.swap_id));

        // Clean up
        delete_matched_order(&pool, &order1.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&pool, &order2.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_with_initiated_swaps() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());
        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Simulate initiate for source swap
        simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();

        let swaps = swap_store.get_swaps().await.unwrap();

        // Should include the initiated swap (since it hasn't been redeemed/refunded)
        let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
        assert!(swap_ids.contains(&order.source_swap.swap_id));

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_handle_inits() {
        let _ = tracing_subscriber::fmt().try_init();
        let pool = pool().await;
        let provider = OrderbookProvider::new(pool.clone());
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;

        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Create initiate events as NonEmptyVec<&SwapEvent>
        let source_event = create_test_swap_event(
            &order.source_swap.swap_id,
            SwapEventType::Initiate,
            "1234567890123456789012345678901234567890",
            1000000,
            100,
            order.source_swap.redeemer.clone(),
            order.source_swap.timelock as i128,
            order.source_swap.asset.clone(),
            order.source_swap.chain.clone(),
        );
        let dest_event = create_test_swap_event(
            &order.destination_swap.swap_id,
            SwapEventType::Initiate,
            "abcdef1234567890abcdef1234567890abcdef12",
            2000000,
            101,
            order.destination_swap.redeemer.clone(),
            order.destination_swap.timelock as i128,
            order.destination_swap.asset.clone(),
            order.destination_swap.chain.clone(),
        );
        let events_vec = vec![&source_event, &dest_event];
        let init_events: tars::utils::NonEmptyVec<&SwapEvent> =
            tars::utils::NonEmptyVec::new(events_vec).unwrap(); // Handle the initiate events
        swap_store.handle_inits(&init_events).await.unwrap();

        // Verify the swaps were updated
        let updated_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();

        let init_events = [&updated_order.source_swap, &updated_order.destination_swap];

        for (i, swap) in init_events.iter().enumerate() {
            assert_eq!(
                swap.initiate_tx_hash.as_str(),
                init_events[i].initiate_tx_hash.as_str()
            );
            assert_eq!(swap.filled_amount, init_events[i].filled_amount);
            assert_eq!(
                swap.initiate_block_number.clone().unwrap(),
                init_events[i].initiate_block_number.clone().unwrap()
            );
            dbg!(swap);
            assert_eq!(swap.current_confirmations, 1)
        }

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_handle_redeems() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());

        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // First initiate the swaps
        simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_initiate(&pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();

        // Create redeem events with secrets
        let secret1 = OrderSecret::try_from("1".repeat(64)).unwrap();
        let secret2 = OrderSecret::try_from("2".repeat(64)).unwrap();
        let secrets = [secret1.clone(), secret2.clone()];
        let tx_hashes = [
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
        ];

        let binding = create_test_swap_event(
            &order.source_swap.swap_id,
            SwapEventType::Redeem(secret1.clone()),
            tx_hashes[0],
            1000000,
            200,
            order.source_swap.redeemer.clone(),
            order.source_swap.timelock as i128,
            order.source_swap.asset.clone(),
            order.source_swap.chain.clone(),
        );
        let binding2 = create_test_swap_event(
            &order.destination_swap.swap_id,
            SwapEventType::Redeem(secret2.clone()),
            tx_hashes[1],
            2000000,
            201,
            order.destination_swap.redeemer.clone(),
            order.destination_swap.timelock as i128,
            order.destination_swap.asset.clone(),
            order.destination_swap.chain.clone(),
        );

        let redeem_events = vec![
            (&binding, secret1.clone().to_string()),
            (&binding2, secret2.clone().to_string()),
        ];

        // Handle the redeem events
        let events_refs: tars::utils::NonEmptyVec<(&SwapEvent, String)> =
            tars::utils::NonEmptyVec::new(redeem_events).unwrap();
        swap_store.handle_redeems(&events_refs).await.unwrap();

        // Verify the swaps were updated
        let updated_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();

        let swaps = [&updated_order.source_swap, &updated_order.destination_swap];

        let block_numbers = [200, 201];

        for (i, swap) in swaps.iter().enumerate() {
            assert_eq!(swap.redeem_tx_hash.as_str(), tx_hashes[i]);
            assert_eq!(
                swap.redeem_block_number.clone().unwrap(),
                BigDecimal::from(block_numbers[i])
            );
            assert_eq!(swap.secret.as_str(), secrets[i].to_string());
        }

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_handle_refunds() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());

        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // First initiate the swaps
        simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_initiate(&pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();

        // Create refund events
        let refund_events = vec![
            create_test_swap_event(
                &order.source_swap.swap_id,
                SwapEventType::Refund,
                "3333333333333333333333333333333333333333",
                1000000,
                300,
                order.source_swap.redeemer.clone(),
                order.source_swap.timelock as i128,
                order.source_swap.asset.clone(),
                order.source_swap.chain.clone(),
            ),
            create_test_swap_event(
                &order.destination_swap.swap_id,
                SwapEventType::Refund,
                "4444444444444444444444444444444444444444",
                2000000,
                301,
                order.destination_swap.redeemer.clone(),
                order.destination_swap.timelock as i128,
                order.destination_swap.asset.clone(),
                order.destination_swap.chain.clone(),
            ),
        ];

        // Handle the refund events
        let refund_events_refs: Vec<_> = refund_events.iter().collect();
        swap_store
            .handle_refunds(&NonEmptyVec::new(refund_events_refs).unwrap())
            .await
            .unwrap();

        // Verify the swaps were updated
        let updated_order = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();

        let swaps = [&updated_order.source_swap, &updated_order.destination_swap];

        for (i, swap) in swaps.iter().enumerate() {
            assert_eq!(
                swap.refund_tx_hash.as_str(),
                refund_events[i].tx_info.tx_hash.as_str()
            );
            assert_eq!(
                swap.refund_block_number.clone().unwrap(),
                refund_events[i].tx_info.block_number.clone().into()
            );
        }

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_update_events_mixed() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());

        delete_all_matched_orders(&pool).await.unwrap();

        // Create multiple orders for testing
        let order_config1 = TestMatchedOrderConfig::default();
        let order1 = create_matched_order(&provider, order_config1)
            .await
            .unwrap();

        let order_config2 = TestMatchedOrderConfig::default();
        let order2 = create_matched_order(&provider, order_config2)
            .await
            .unwrap();

        let order_config3 = TestMatchedOrderConfig::default();
        let order3 = create_matched_order(&provider, order_config3)
            .await
            .unwrap();

        // Prepare secrets for redeem event
        let secret1 = OrderSecret::try_from("1".repeat(64)).unwrap();

        // Create mixed events
        let mixed_events = vec![
            // Initiate events
            create_test_swap_event(
                &order1.source_swap.swap_id,
                SwapEventType::Initiate,
                "1111111111111111111111111111111111111111",
                1000000,
                100,
                order1.source_swap.redeemer.clone(),
                order1.source_swap.timelock as i128,
                order1.source_swap.asset.clone(),
                order1.source_swap.chain.clone(),
            ),
            create_test_swap_event(
                &order1.destination_swap.swap_id,
                SwapEventType::Initiate,
                "2222222222222222222222222222222222222222",
                2000000,
                101,
                order1.destination_swap.redeemer.clone(),
                order1.destination_swap.timelock as i128,
                order1.destination_swap.asset.clone(),
                order1.destination_swap.chain.clone(),
            ),
            // Redeem events
            create_test_swap_event(
                &order2.source_swap.swap_id,
                SwapEventType::Redeem(secret1.clone()),
                "3333333333333333333333333333333333333333",
                1500000,
                200,
                order2.source_swap.redeemer.clone(),
                order2.source_swap.timelock as i128,
                order2.source_swap.asset.clone(),
                order2.source_swap.chain.clone(),
            ),
            // Refund events
            create_test_swap_event(
                &order3.source_swap.swap_id,
                SwapEventType::Refund,
                "4444444444444444444444444444444444444444",
                1200000,
                300,
                order3.source_swap.redeemer.clone(),
                order3.source_swap.timelock as i128,
                order3.source_swap.asset.clone(),
                order3.source_swap.chain.clone(),
            ),
        ];

        // Handle all events at once
        swap_store
            .update_events(&NonEmptyVec::new(mixed_events.iter().collect()).unwrap())
            .await
            .unwrap();

        // Verify all updates for order1 (initiates)
        let updated_order1 = provider
            .get_matched_order(&order1.create_order.create_id)
            .await
            .unwrap()
            .unwrap();
        let swaps1 = [
            &updated_order1.source_swap,
            &updated_order1.destination_swap,
        ];
        let tx_hashes1 = [
            "1111111111111111111111111111111111111111",
            "2222222222222222222222222222222222222222",
        ];
        for (i, swap) in swaps1.iter().enumerate() {
            assert_eq!(swap.initiate_tx_hash.as_str(), tx_hashes1[i]);
        }

        // Verify redeem for order2
        let updated_order2 = provider
            .get_matched_order(&order2.create_order.create_id)
            .await
            .unwrap()
            .unwrap();
        let swap2 = &updated_order2.source_swap;
        assert_eq!(
            swap2.redeem_tx_hash.as_str(),
            "3333333333333333333333333333333333333333"
        );
        assert_eq!(swap2.secret.as_str(), secret1.to_string());

        // Verify refund for order3
        let updated_order3 = provider
            .get_matched_order(&order3.create_order.create_id)
            .await
            .unwrap()
            .unwrap();
        let swap3 = &updated_order3.source_swap;
        assert_eq!(
            swap3.refund_tx_hash.as_str(),
            "4444444444444444444444444444444444444444"
        );

        // Clean up
        delete_matched_order(&pool, &order1.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&pool, &order2.create_order.create_id)
            .await
            .unwrap();
        delete_matched_order(&pool, &order3.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_excludes_completed_swaps() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());
        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Simulate complete swap lifecycle (initiate -> redeem)
        simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_redeem(
            &pool,
            &order.source_swap.swap_id,
            &order.destination_swap.secret_hash,
            None,
        )
        .await
        .unwrap();

        let swaps = swap_store.get_swaps().await.unwrap();

        // Should not include the completed swap
        let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
        assert!(!swap_ids.contains(&order.source_swap.swap_id));

        // But should still include the destination swap which hasn't been initiated
        assert!(swap_ids.contains(&order.destination_swap.swap_id));

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_excludes_refunded_swaps() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());
        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Simulate swap lifecycle (initiate -> refund)
        simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_refund(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();

        let swaps = swap_store.get_swaps().await.unwrap();

        // Should not include the refunded swap
        let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
        assert!(!swap_ids.contains(&order.source_swap.swap_id));

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_updates_with_invalid_swap_id() {
        let pool = pool().await;
        let provider = OrderbookProvider::new(pool.clone());
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 120, None).await;

        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Create events with one invalid swap_id to trigger a rollback
        let invalid_events = vec![
            create_test_swap_event(
                &order.source_swap.swap_id,
                SwapEventType::Initiate,
                "1111111111111111111111111111111111111111",
                1000000,
                100,
                order.source_swap.redeemer.clone(),
                order.source_swap.timelock as i128,
                order.source_swap.asset.clone(),
                order.source_swap.chain.clone(),
            ),
            create_test_swap_event(
                "invalid_swap_id",
                SwapEventType::Initiate,
                "2222222222222222222222222222222222222222",
                2000000,
                101,
                order.destination_swap.redeemer.clone(),
                order.destination_swap.timelock as i128,
                order.destination_swap.asset.clone(),
                order.destination_swap.chain.clone(),
            ),
        ];

        // This should fail and rollback
        let _ = swap_store
            .handle_inits(&NonEmptyVec::new(invalid_events.iter().collect()).unwrap())
            .await;

        // Verify that the first swap was not updated due to rollback
        let order_after = provider
            .get_matched_order(&order.create_order.create_id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(order_after.source_swap.initiate_tx_hash.as_str(), "");

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swap_for_ignored_chains() {
        let pool = pool().await;
        let swap_store =
            GardenSwapStore::new(pool.clone(), vec!["chain1".to_string()], 120, None).await;
        let provider = OrderbookProvider::new(pool.clone());
        delete_all_matched_orders(&pool).await.unwrap();

        // Create a test order on chain1
        let order_config = TestMatchedOrderConfig {
            source_chain: "chain1".to_string(),
            ..Default::default()
        };
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Get swaps
        let swaps = swap_store.get_swaps().await.unwrap();

        // Should not include the swap from ignored chain
        assert!(swaps.len() == 1);

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_with_deadline_buffer() {
        let pool = pool().await;
        let swap_store = GardenSwapStore::new(pool.clone(), vec![], 0, None).await;
        let provider = OrderbookProvider::new(pool.clone());
        delete_all_matched_orders(&pool).await.unwrap();

        // Create a test order with a short deadline
        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();

        // Get swaps - should include the swap since it is within the deadline buffer
        let swaps = swap_store.get_swaps().await.unwrap();
        assert_eq!(swaps.len(), 0); // 0 swaps per order

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[tokio::test]
    #[serial]
    async fn test_get_swaps_with_supported_assets() {
        let pool = pool().await;
        let provider = OrderbookProvider::new(pool.clone());
        delete_all_matched_orders(&pool).await.unwrap();
        let swap_store =
            GardenSwapStore::new(pool.clone(), vec![], 120, Some(vec!["primary".to_string()]))
                .await;

        delete_all_matched_orders(&pool).await.unwrap();

        let order_config = TestMatchedOrderConfig::default();
        let order = create_matched_order(&provider, order_config).await.unwrap();
        dbg!(&order.source_swap.asset);
        dbg!(&order.destination_swap.asset);
        // initiate s
        simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
            .await
            .unwrap();
        simulate_test_swap_initiate(&pool, &order.destination_swap.swap_id, None)
            .await
            .unwrap();
        // get the swaps
        let swaps = swap_store.get_swaps().await.unwrap();
        dbg!(&swaps);
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].asset, "primary");

        // Clean up
        delete_matched_order(&pool, &order.create_order.create_id)
            .await
            .unwrap();
    }

    #[test]
    fn test_group_by_chains() {
        #[derive(Clone)]
        struct Swaps {
            chain: String,
        }

        impl HasChain for Swaps {
            type Chain = String;

            fn chain(&self) -> &Self::Chain {
                &self.chain
            }
        }

        let mut swaps = vec![
            Swaps {
                chain: "chain1".to_string(),
            },
            Swaps {
                chain: "chain1".to_string(),
            },
            Swaps {
                chain: "chain2".to_string(),
            },
        ];

        let grouped = group_by_chains(&swaps);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped["chain1"].len(), 2);
        assert_eq!(grouped["chain2"].len(), 1);

        swaps.push(Swaps {
            chain: "chain2".to_string(),
        });

        let grouped = group_by_chains(&swaps);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped["chain1"].len(), 2);
        assert_eq!(grouped["chain2"].len(), 2);
    }
}
