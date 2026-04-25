use crate::core::{Swap, SwapEvent, SwapEventType, SwapStore, Vec1};
use async_trait::async_trait;
use eyre::Result;
use num_traits::ToPrimitive;
use sqlx::types::chrono;
use sqlx::{Pool, Postgres};
use std::collections::HashSet;
use std::time::Duration;
use tars::orderbook::primitives::SingleSwap;

const INITIATE_DETECTED_TIMESTAMP_KEY: &str = "initiate_detected_timestamp";
const REDEEM_DETECTED_TIMESTAMP_KEY: &str = "redeem_detected_timestamp";
const REFUND_DETECTED_TIMESTAMP_KEY: &str = "refund_detected_timestamp";

impl From<SingleSwap> for Swap {
    fn from(single_swap: SingleSwap) -> Self {
        let amount = single_swap.amount.to_i64().unwrap_or_else(|| {
            tracing::warn!(
                swap_id = %single_swap.swap_id,
                raw_amount = %single_swap.amount,
                "Failed to convert swap amount to i64, defaulting to 0"
            );
            0
        });
        Self {
            swap_id: single_swap.swap_id,
            amount,
        }
    }
}

pub struct GardenSwapStore {
    pool: Pool<Postgres>,
    deadline_buffer: i64, // in seconds
}

impl GardenSwapStore {
    pub fn new(pool: Pool<Postgres>, deadline_buffer: i64) -> Self {
        Self {
            pool,
            deadline_buffer,
        }
    }

    pub async fn from_db_url(db_url: &str, deadline_buffer: i64) -> Result<Self> {
        let sqlx_pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(10)
            .min_connections(2)
            .acquire_timeout(Duration::from_secs(5))
            .idle_timeout(Duration::from_secs(300))
            .connect(db_url)
            .await?;
        Ok(Self::new(sqlx_pool, deadline_buffer))
    }

    /// Handles detected timestamp updates for create_orders.additional_data in batch.
    /// Sets bitcoin timestamps in additional_data for swaps in a single query.
    /// The timestamp is only set if the field doesn't already exist or is empty.
    async fn handle_detected_timestamps(
        &self,
        // tuple of swap_id and detected timestamp
        timestamp_data: &[(String, chrono::DateTime<chrono::Utc>)],
        timestamp_key: &str,
        tx: &mut sqlx::Transaction<'_, Postgres>,
    ) -> Result<()> {
        if timestamp_data.is_empty() {
            return Ok(());
        }

        let mut swap_ids = Vec::with_capacity(timestamp_data.len());
        let mut timestamps = Vec::with_capacity(timestamp_data.len());

        for (swap_id, timestamp) in timestamp_data {
            swap_ids.push(swap_id.as_str());
            timestamps.push(timestamp.timestamp());
        }

        let query = "
            UPDATE create_orders
            SET additional_data = jsonb_set(
                COALESCE(additional_data, '{}'::jsonb),
                '{bitcoin}',
                COALESCE(additional_data->'bitcoin', '{}'::jsonb) || 
                    CASE 
                        WHEN (additional_data->'bitcoin'->$1) IS NULL 
                            OR (additional_data->'bitcoin'->>$1) = ''
                        THEN jsonb_build_object($1, to_timestamp(mo_data.timestamp_seconds))
                        ELSE '{}'::jsonb
                    END
            )
            FROM (
                SELECT mo.create_order_id, d.timestamp_seconds
                FROM (
                    SELECT * FROM UNNEST($2::TEXT[], $3::BIGINT[])
                    AS t(swap_id, timestamp_seconds)
                ) d
                JOIN matched_orders mo ON (
                    LOWER(mo.source_swap_id) = LOWER(d.swap_id) 
                    OR LOWER(mo.destination_swap_id) = LOWER(d.swap_id)
                )
            ) mo_data
            WHERE create_orders.create_id = mo_data.create_order_id
        ";

        sqlx::query(query)
            .bind(timestamp_key)
            .bind(&swap_ids)
            .bind(&timestamps)
            .execute(&mut **tx)
            .await?;

        Ok(())
    }

    /// Helper function to update detected timestamps for events.
    /// Collects detected timestamps from events and updates them in a separate transaction.
    async fn update_detected_timestamps(
        &self,
        events: &[&SwapEvent],
        timestamp_key: &str,
        event_type: &str,
    ) -> Result<()> {
        let detected_timestamps: Vec<(String, chrono::DateTime<chrono::Utc>)> = events
            .iter()
            .filter_map(|item| {
                item.tx_info
                    .detected_timestamp
                    .map(|ts| (item.swap_id.clone(), ts))
            })
            .collect();

        if detected_timestamps.is_empty() {
            return Ok(());
        }

        let mut timestamp_tx = self.pool.begin().await?;
        if let Err(e) = self
            .handle_detected_timestamps(&detected_timestamps, timestamp_key, &mut timestamp_tx)
            .await
        {
            tracing::warn!(
                "Failed to update detected timestamps for {} events: {}",
                event_type,
                e
            );
        }

        timestamp_tx.commit().await?;
        Ok(())
    }

    // Helper function to log batch update results and debug details
    fn log_batch_update(&self, event_type: &str, events: &Vec1<&SwapEvent>, actual_updates: usize) {
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
                    "Invalid {} update batch details - swap_id: {}",
                    event_type,
                    event.swap_id.as_str(),
                );
            }
            return;
        }

        tracing::info!(
            "Successfully batch updated {} swaps with {} events",
            actual_updates,
            event_type
        );
        // log all the events
        for event in events.as_ref().iter() {
            tracing::info!(
                "Updated swap: swap_id: {}, event_type: {:?}, tx_hash: {}, block_number: {}",
                event.swap_id,
                event.event_type,
                event.tx_info.tx_hash,
                event.tx_info.block_number
            );
        }
    }

    async fn handle_inits(&self, events: &Vec1<&SwapEvent>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let events_len = events.len();
        // Prepare batch data
        let mut filled_amounts = Vec::with_capacity(events_len);
        let mut initiate_tx_hashes = Vec::with_capacity(events_len);
        let mut initiate_block_numbers = Vec::with_capacity(events_len);
        let mut initiate_block_timestamps = Vec::with_capacity(events_len);
        let mut swap_ids = Vec::with_capacity(events_len);

        for e in events.as_ref() {
            filled_amounts.push(&e.amount);
            initiate_tx_hashes.push(&e.tx_info.tx_hash);
            initiate_block_numbers.push(&e.tx_info.block_number);
            initiate_block_timestamps.push(&e.tx_info.block_timestamp);
            swap_ids.push(e.swap_id.as_str());
        }

        let q = "
        UPDATE swaps SET
            filled_amount = swaps.amount,
            initiate_tx_hash = d.initiate_tx_hash,
            initiate_block_number = d.initiate_block_number,
            initiate_timestamp = d.initiate_timestamp
        FROM (
            SELECT * FROM UNNEST(
                $1::NUMERIC[], $2::TEXT[], $3::BIGINT[], $4::TIMESTAMP[], $5::TEXT[]
            ) AS t(filled_amount, initiate_tx_hash, initiate_block_number, initiate_timestamp, swap_id)
        ) d
        WHERE LOWER(swaps.swap_id)=LOWER(d.swap_id)";

        let res = match sqlx::query(q)
            .bind(&filled_amounts)
            .bind(&initiate_tx_hashes)
            .bind(&initiate_block_numbers)
            .bind(&initiate_block_timestamps)
            .bind(&swap_ids)
            .execute(&mut *tx)
            .await
        {
            Ok(res) => {
                tx.commit().await?;
                res
            }
            Err(e) => {
                return Err(eyre::eyre!("Failed to update inits: {}", e));
            }
        };

        self.log_batch_update("initiate", events, res.rows_affected() as usize);

        self.update_detected_timestamps(
            events.as_ref(),
            INITIATE_DETECTED_TIMESTAMP_KEY,
            "initiate",
        )
        .await?;

        Ok(())
    }

    // Handles redeem events and updates the swaps table accordingly using batch update.
    async fn handle_redeems(&self, events: &Vec1<(&SwapEvent, String)>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let events_len = events.len();

        // Prepare batch data
        let mut redeem_tx_hashes = Vec::with_capacity(events_len);
        let mut redeem_block_numbers = Vec::with_capacity(events_len);
        let mut redeem_block_timestamps = Vec::with_capacity(events_len);
        let mut secrets = Vec::with_capacity(events_len);
        let mut swap_ids = Vec::with_capacity(events_len);
        let mut redeem_events = Vec::with_capacity(events_len);

        for (event, secret) in events.as_ref().iter() {
            redeem_tx_hashes.push(&event.tx_info.tx_hash);
            redeem_block_numbers.push(&event.tx_info.block_number);
            redeem_block_timestamps.push(&event.tx_info.block_timestamp);
            secrets.push(secret);
            swap_ids.push(event.swap_id.as_str());
            redeem_events.push(*event);
        }

        let batch_update_query = "
        UPDATE swaps 
        SET redeem_tx_hash = data_table.redeem_tx_hash,
            redeem_block_number = data_table.redeem_block_number,
            redeem_timestamp = data_table.redeem_timestamp,
            secret = data_table.secret
        FROM (
            SELECT * FROM UNNEST($1::TEXT[], $2::BIGINT[], $3::TIMESTAMP[], $4::TEXT[], $5::TEXT[])
            AS t(redeem_tx_hash, redeem_block_number, redeem_timestamp, secret, swap_id)
        ) AS data_table
        WHERE LOWER(swaps.swap_id) = LOWER(data_table.swap_id)";

        let res = match sqlx::query(batch_update_query)
            .bind(&redeem_tx_hashes)
            .bind(&redeem_block_numbers)
            .bind(&redeem_block_timestamps)
            .bind(&secrets)
            .bind(&swap_ids)
            .execute(&mut *tx)
            .await
        {
            Ok(res) => {
                tx.commit().await?;
                res
            }
            Err(e) => {
                tx.commit().await?;
                return Err(eyre::eyre!("Failed to update redeems: {}", e));
            }
        };

        let actual_updates = res.rows_affected() as usize;
        let redeem_events = match Vec1::try_from(redeem_events) {
            Ok(events) => events,
            Err(_) => {
                tracing::error!(
                    "Batch redeem update mismatch: expected {} updates, but {} rows were affected",
                    events.len(),
                    actual_updates,
                );
                return eyre::Ok(());
            }
        };

        self.log_batch_update("redeem", &redeem_events, actual_updates);

        let redeem_swap_events: Vec<&SwapEvent> =
            events.as_ref().iter().map(|(event, _)| *event).collect();
        self.update_detected_timestamps(
            &redeem_swap_events,
            REDEEM_DETECTED_TIMESTAMP_KEY,
            "redeem",
        )
        .await?;

        Ok(())
    }

    // Handles refund events and updates the swaps table accordingly using batch update.
    async fn handle_refunds(&self, events: &Vec1<&SwapEvent>) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let events_len = events.len();

        // Prepare batch data
        let mut refund_tx_hashes = Vec::with_capacity(events_len);
        let mut refund_block_numbers = Vec::with_capacity(events_len);
        let mut refund_timestamps = Vec::with_capacity(events_len);
        let mut swap_ids = Vec::with_capacity(events_len);

        for event in events.as_ref().iter() {
            refund_tx_hashes.push(&event.tx_info.tx_hash);
            refund_block_numbers.push(&event.tx_info.block_number);
            refund_timestamps.push(&event.tx_info.block_timestamp);
            swap_ids.push(event.swap_id.as_str());
        }

        let batch_update_query = "
        UPDATE swaps 
        SET refund_tx_hash = data_table.refund_tx_hash,
            refund_block_number = data_table.refund_block_number,
            refund_timestamp = data_table.refund_timestamp
        FROM (
            SELECT * FROM UNNEST($1::TEXT[], $2::BIGINT[], $3::TIMESTAMP[], $4::TEXT[])
            AS t(refund_tx_hash, refund_block_number, refund_timestamp, swap_id)
        ) AS data_table
        WHERE LOWER(swaps.swap_id) = LOWER(data_table.swap_id)";

        let res = match sqlx::query(batch_update_query)
            .bind(&refund_tx_hashes)
            .bind(&refund_block_numbers)
            .bind(&refund_timestamps)
            .bind(&swap_ids)
            .execute(&mut *tx)
            .await
        {
            Ok(res) => {
                tx.commit().await?;
                res
            }
            Err(e) => {
                tx.commit().await?;
                return Err(eyre::eyre!("Failed to update refunds: {}", e));
            }
        };

        let actual_updates = res.rows_affected() as usize;
        self.log_batch_update("refund", events, actual_updates);
        self.update_detected_timestamps(events.as_ref(), REFUND_DETECTED_TIMESTAMP_KEY, "refund")
            .await?;

        Ok(())
    }
}

#[async_trait]
impl SwapStore for GardenSwapStore {
    async fn get_swaps(&self, chain: &str) -> Result<Vec<Swap>> {
        // Even with this query, we might get some pending not redeemed not refunded swaps
        // TODO: have to figure out what to do
        let swaps = sqlx::query_as::<_, SingleSwap>(
            r#"
            SELECT s.*
            FROM swaps s
            WHERE s.chain = $1
            AND (
                (s.initiate_tx_hash = ''
                 AND s.created_at + ($2 * INTERVAL '1 second') > NOW())
                OR
                (s.initiate_tx_hash != ''
                 AND COALESCE(s.redeem_block_number, 0) = 0
                 AND COALESCE(s.refund_block_number, 0) = 0)
            )
            "#,
        )
        .bind(chain)
        .bind(self.deadline_buffer)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| eyre::eyre!("Failed to fetch pending swaps: {e}"))?;

        Ok(swaps.into_iter().map(Swap::from).collect())
    }

    async fn update_swaps(&self, swap_events: Vec1<SwapEvent>) -> Result<()> {
        let (mut inits, mut redeems, mut refunds) = (Vec::new(), Vec::new(), Vec::new());
        for event in swap_events.as_ref().iter() {
            match &event.event_type {
                SwapEventType::Initiate => inits.push(event),
                SwapEventType::Refund => refunds.push(event),
                SwapEventType::Redeem(secret) => redeems.push((event, secret.to_string())),
            }
        }

        if let Ok(inits) = Vec1::new(inits) {
            self.handle_inits(&inits).await?;
        }
        if let Ok(redeems) = Vec1::new(redeems) {
            self.handle_redeems(&redeems).await?;
        }
        if let Ok(refunds) = Vec1::new(refunds) {
            self.handle_refunds(&refunds).await?;
        }

        Ok(())
    }

    async fn update_confirmations(&self, chain: &str, current_block: i64) -> Result<()> {
        // The least between required_confirmations and the calculated difference (ensuring it doesn't go negative).
        let query = r#"
            UPDATE swaps
            SET current_confirmations = LEAST(required_confirmations, GREATEST($2::BIGINT - initiate_block_number + 1, 0))
            WHERE chain = $1
              AND required_confirmations != current_confirmations
              AND initiate_tx_hash != ''
        "#;

        let res = sqlx::query(query)
            .bind(chain)
            .bind(current_block)
            .execute(&self.pool)
            .await
            .map_err(|e| eyre::eyre!("Failed to update confirmations: {}", e))?;

        tracing::info!(
            chain = %chain,
            current_block = %current_block,
            rows_affected = %res.rows_affected(),
            "update_confirmations SQL executed"
        );

        Ok(())
    }

    async fn get_swaps_missing_initiate_block(
        &self,
        chain: &str,
    ) -> Result<Vec<(String, String)>> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            r#"
            SELECT swap_id, initiate_tx_hash
            FROM swaps
            WHERE chain = $1
              AND initiate_tx_hash != ''
              AND (
                   initiate_block_number IS NULL
                OR initiate_block_number = 0
                OR filled_amount IS NULL
                OR filled_amount = 0
              )
            "#,
        )
        .bind(chain)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| eyre::eyre!("Failed to fetch swaps missing initiate_block_number: {e}"))?;

        Ok(rows)
    }

    async fn backfill_initiate(
        &self,
        swap_id: &str,
        block_number: i64,
        filled_amount: i64,
    ) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE swaps
            SET
                initiate_block_number = CASE
                    WHEN initiate_block_number IS NULL OR initiate_block_number = 0 THEN $2
                    ELSE initiate_block_number
                END,
                filled_amount = CASE
                    WHEN filled_amount IS NULL OR filled_amount = 0 THEN $3::NUMERIC
                    ELSE filled_amount
                END,
                amount = $3::NUMERIC
            WHERE swap_id = $1
            "#,
        )
        .bind(swap_id)
        .bind(block_number)
        .bind(filled_amount)
        .execute(&self.pool)
        .await
        .map_err(|e| eyre::eyre!("Failed to backfill initiate: {e}"))?;

        Ok(())
    }

    async fn mark_blacklisted(&self, swap_ids: &Vec1<String>) -> Result<()> {
        tracing::info!(swap_ids = ?swap_ids.as_ref(), "Marking swaps as blacklisted");

        let query = r#"
        UPDATE create_orders
        SET additional_data = jsonb_set(
            COALESCE(additional_data, '{}'::jsonb), '{is_blacklisted}', 'true'::jsonb
        )
        WHERE create_id IN (
            SELECT create_order_id FROM matched_orders
            WHERE source_swap_id = ANY($1) OR destination_swap_id = ANY($1)
        )
    "#;

        sqlx::query(query)
            .bind(swap_ids.as_ref())
            .execute(&self.pool)
            .await
            .map_err(|e| eyre::eyre!("Failed to mark as blacklisted: {}", e))?;

        Ok(())
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use crate::core::{OrderSecret, TxInfo};
//     use tars::orderbook::{
//         OrderbookProvider,
//         primitives::MatchedOrderVerbose,
//         test_utils::{
//             TestMatchedOrderConfig, create_test_matched_order, delete_all_matched_orders,
//             delete_matched_order, simulate_test_swap_initiate, simulate_test_swap_redeem,
//             simulate_test_swap_refund,
//         },
//         traits::Orderbook,
//     };
//     use serial_test::serial;
//     use sqlx::types::BigDecimal;
//     use tracing::info;

//     const DB_URL: &str = "postgres://postgres:postgres@localhost:5432/postgres";
//     pub async fn pool() -> Pool<Postgres> {
//         sqlx::postgres::PgPoolOptions::new()
//             .max_connections(2000)
//             .connect(DB_URL)
//             .await
//             .expect("Failed to create pool")
//     }

//     fn create_test_swap_event(
//         swap_id: &str,
//         event_type: SwapEventType,
//         tx_hash: &str,
//         amount: i64,
//         block_number: i64,
//         detected_timestamp: Option<chrono::DateTime<chrono::Utc>>,
//     ) -> SwapEvent {
//         SwapEvent {
//             event_type,
//             swap_id: swap_id.to_string(),
//             amount,
//             tx_info: TxInfo {
//                 tx_hash: tx_hash.to_string(),
//                 block_number,
//                 block_timestamp: None,
//                 detected_timestamp,
//             },
//             is_blacklisted: false,
//         }
//     }

//     async fn create_matched_order(
//         provider: &OrderbookProvider,
//         config: TestMatchedOrderConfig,
//     ) -> eyre::Result<MatchedOrderVerbose> {
//         let order = create_test_matched_order(&provider.sqlx_pool, config)
//             .await
//             .unwrap();
//         delete_matched_order(&provider.sqlx_pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//         provider.create_matched_order(&order).await?;
//         let order = provider
//             .get_matched_order(&order.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();
//         Ok(order)
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_get_swaps_empty() {
//         let _ = tracing_subscriber::fmt()
//             .with_max_level(tracing::Level::INFO)
//             .try_init();

//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         delete_all_matched_orders(&pool).await.unwrap();

//         let swaps = swap_store.get_swaps("bitcoin_regtest").await.unwrap();
//         assert!(swaps.is_empty());
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_get_swaps_with_pending_swaps() {
//         let pool = pool().await;
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);

//         delete_all_matched_orders(&pool).await.unwrap();

//         // Create test orders - these will have empty initiate_tx_hash initially
//         let order_config1 = TestMatchedOrderConfig::default();
//         let order1 = create_matched_order(&provider, order_config1)
//             .await
//             .unwrap();

//         let order_config2 = TestMatchedOrderConfig::default();
//         let order2 = create_matched_order(&provider, order_config2)
//             .await
//             .unwrap();

//         let swaps = swap_store
//             .get_swaps(order1.source_swap.chain.as_str())
//             .await
//             .unwrap();
//         assert_eq!(swaps.len(), 2);

//         // Verify that our test orders' swaps are included
//         let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
//         assert!(swap_ids.contains(&order1.source_swap.swap_id));
//         assert!(swap_ids.contains(&order2.source_swap.swap_id));

//         // Clean up
//         delete_matched_order(&pool, &order1.create_order.create_id)
//             .await
//             .unwrap();
//         delete_matched_order(&pool, &order2.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_get_swaps_with_initiated_swaps() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();
//         delete_all_matched_orders(&pool).await.unwrap();

//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         // Simulate initiate for source swap
//         simulate_test_swap_initiate(&provider.sqlx_pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();

//         let swaps = swap_store
//             .get_swaps(order.source_swap.chain.as_str())
//             .await
//             .unwrap();

//         // Should include the initiated swap (since it hasn't been redeemed/refunded)
//         let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
//         assert!(swap_ids.contains(&order.source_swap.swap_id));

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_get_swaps_excludes_completed_swaps() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();
//         delete_all_matched_orders(&pool).await.unwrap();

//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         // Simulate complete swap lifecycle (initiate -> redeem)
//         simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();
//         simulate_test_swap_redeem(
//             &provider.sqlx_pool,
//             &order.source_swap.swap_id,
//             &order.destination_swap.secret_hash,
//             None,
//         )
//         .await
//         .unwrap();

//         let swaps = swap_store
//             .get_swaps(order.source_swap.chain.as_str())
//             .await
//             .unwrap();

//         // Should not include the completed swap
//         let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
//         assert!(!swap_ids.contains(&order.source_swap.swap_id));

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_get_swaps_excludes_refunded_swaps() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();
//         delete_all_matched_orders(&pool).await.unwrap();

//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         // Simulate swap lifecycle (initiate -> refund)
//         simulate_test_swap_initiate(&provider.sqlx_pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();
//         simulate_test_swap_refund(&provider.sqlx_pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();

//         let swaps = swap_store
//             .get_swaps(order.source_swap.chain.as_str())
//             .await
//             .unwrap();

//         // Should not include the refunded swap
//         let swap_ids: Vec<String> = swaps.iter().map(|s| s.swap_id.clone()).collect();
//         assert!(!swap_ids.contains(&order.source_swap.swap_id));

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_handle_inits() {
//         let _ = tracing_subscriber::fmt().try_init();
//         let pool = pool().await;
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);

//         delete_all_matched_orders(&pool).await.unwrap();

//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         info!("source swap id: {:?}", order.source_swap.swap_id);
//         info!("destination swap id: {:?}", order.destination_swap.swap_id);
//         // Create initiate events
//         let source_event = create_test_swap_event(
//             &order.source_swap.swap_id,
//             SwapEventType::Initiate,
//             "1234567890123456789012345678901234567890",
//             order.source_swap.amount.to_string().parse::<i64>().unwrap(),
//             100,
//             None,
//         );
//         let dest_event = create_test_swap_event(
//             &order.destination_swap.swap_id,
//             SwapEventType::Initiate,
//             "abcdef1234567890abcdef1234567890abcdef12",
//             order
//                 .destination_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             101,
//             None,
//         );

//         let events_vec = vec![&source_event, &dest_event];
//         let init_events = Vec1::new(events_vec).unwrap();

//         // Handle the initiate events
//         let _ = swap_store.handle_inits(&init_events).await;

//         // Verify the swaps were updated
//         let updated_order = provider
//             .get_matched_order(&order.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();

//         // Check source swap
//         assert_eq!(
//             updated_order.source_swap.initiate_tx_hash.as_str(),
//             "1234567890123456789012345678901234567890"
//         );
//         assert_eq!(
//             updated_order.source_swap.initiate_block_number.unwrap(),
//             BigDecimal::from(100)
//         );

//         dbg!(updated_order.source_swap.initiate_timestamp);

//         // Check destination swap
//         assert_eq!(
//             updated_order.destination_swap.initiate_tx_hash.as_str(),
//             "abcdef1234567890abcdef1234567890abcdef12"
//         );
//         assert_eq!(
//             updated_order
//                 .destination_swap
//                 .initiate_block_number
//                 .unwrap(),
//             BigDecimal::from(101)
//         );

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_handle_redeems() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();

//         delete_all_matched_orders(&pool).await.unwrap();

//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         // First initiate the swaps
//         simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();
//         simulate_test_swap_initiate(&pool, &order.destination_swap.swap_id, None)
//             .await
//             .unwrap();

//         // Create redeem events with secrets
//         let secret1 = OrderSecret::new("1".repeat(64)).unwrap();
//         let secret2 = OrderSecret::new("2".repeat(64)).unwrap();

//         let source_event = create_test_swap_event(
//             &order.source_swap.swap_id,
//             SwapEventType::Redeem(secret1.clone()),
//             "1111111111111111111111111111111111111111",
//             order.source_swap.amount.to_string().parse::<i64>().unwrap(),
//             200,
//             None,
//         );
//         let dest_event = create_test_swap_event(
//             &order.destination_swap.swap_id,
//             SwapEventType::Redeem(secret2.clone()),
//             "2222222222222222222222222222222222222222",
//             order
//                 .destination_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             201,
//             None,
//         );

//         let redeem_events = vec![
//             (&source_event, secret1.to_string()),
//             (&dest_event, secret2.to_string()),
//         ];
//         let events_refs = Vec1::new(redeem_events).unwrap();

//         // Handle the redeem events
//         swap_store.handle_redeems(&events_refs).await.unwrap();

//         // Verify the swaps were updated
//         let updated_order = provider
//             .get_matched_order(&order.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();

//         // Check source swap
//         assert_eq!(
//             updated_order.source_swap.redeem_tx_hash.as_str(),
//             "1111111111111111111111111111111111111111"
//         );
//         assert_eq!(
//             updated_order.source_swap.redeem_block_number.unwrap(),
//             BigDecimal::from(200)
//         );
//         assert_eq!(updated_order.source_swap.secret.as_str(), secret1.as_str());

//         // Check destination swap
//         assert_eq!(
//             updated_order.destination_swap.redeem_tx_hash.as_str(),
//             "2222222222222222222222222222222222222222"
//         );
//         assert_eq!(
//             updated_order.destination_swap.redeem_block_number.unwrap(),
//             BigDecimal::from(201)
//         );
//         assert_eq!(
//             updated_order.destination_swap.secret.as_str(),
//             secret2.as_str()
//         );

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_handle_refunds() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();

//         delete_all_matched_orders(&pool).await.unwrap();

//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         // First initiate the swaps
//         simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();
//         simulate_test_swap_initiate(&pool, &order.destination_swap.swap_id, None)
//             .await
//             .unwrap();

//         // Create refund events
//         let source_event = create_test_swap_event(
//             &order.source_swap.swap_id,
//             SwapEventType::Refund,
//             "3333333333333333333333333333333333333333",
//             order.source_swap.amount.to_string().parse::<i64>().unwrap(),
//             300,
//             None,
//         );
//         let dest_event = create_test_swap_event(
//             &order.destination_swap.swap_id,
//             SwapEventType::Refund,
//             "4444444444444444444444444444444444444444",
//             order
//                 .destination_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             301,
//             None,
//         );

//         let refund_events_refs: Vec<_> = vec![&source_event, &dest_event];
//         let events = Vec1::new(refund_events_refs).unwrap();

//         // Handle the refund events
//         swap_store.handle_refunds(&events).await.unwrap();

//         // Verify the swaps were updated
//         let updated_order = provider
//             .get_matched_order(&order.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();

//         // Check source swap
//         assert_eq!(
//             updated_order.source_swap.refund_tx_hash.as_str(),
//             "3333333333333333333333333333333333333333"
//         );
//         assert_eq!(
//             updated_order.source_swap.refund_block_number.unwrap(),
//             BigDecimal::from(300)
//         );

//         // Check destination swap
//         assert_eq!(
//             updated_order.destination_swap.refund_tx_hash.as_str(),
//             "4444444444444444444444444444444444444444"
//         );
//         assert_eq!(
//             updated_order.destination_swap.refund_block_number.unwrap(),
//             BigDecimal::from(301)
//         );

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_update_swaps_mixed() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();

//         delete_all_matched_orders(&pool).await.unwrap();

//         // Create multiple orders for testing
//         let order1 = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();
//         let order2 = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();
//         let order3 = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();

//         // Prepare order2 and order3 with initiates (needed before redeem/refund)
//         simulate_test_swap_initiate(&pool, &order2.source_swap.swap_id, None)
//             .await
//             .unwrap();
//         simulate_test_swap_initiate(&pool, &order3.source_swap.swap_id, None)
//             .await
//             .unwrap();

//         // Prepare secret for redeem event
//         let secret = OrderSecret::new("1".repeat(64)).unwrap();

//         // Create mixed events
//         let init_event = create_test_swap_event(
//             &order1.source_swap.swap_id,
//             SwapEventType::Initiate,
//             "1111111111111111111111111111111111111111",
//             order1
//                 .source_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             100,
//             None,
//         );
//         let redeem_event = create_test_swap_event(
//             &order2.source_swap.swap_id,
//             SwapEventType::Redeem(secret.clone()),
//             "2222222222222222222222222222222222222222",
//             order2
//                 .source_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             200,
//             None,
//         );
//         let refund_event = create_test_swap_event(
//             &order3.source_swap.swap_id,
//             SwapEventType::Refund,
//             "3333333333333333333333333333333333333333",
//             order3
//                 .source_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             300,
//             None,
//         );

//         let mixed_events = Vec1::new(vec![init_event, redeem_event, refund_event]).unwrap();

//         // Handle all events at once via update_swaps
//         swap_store.update_swaps(mixed_events).await.unwrap();

//         // Verify initiate for order1
//         let updated_order1 = provider
//             .get_matched_order(&order1.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();
//         assert_eq!(
//             updated_order1.source_swap.initiate_tx_hash.as_str(),
//             "1111111111111111111111111111111111111111"
//         );

//         // Verify redeem for order2
//         let updated_order2 = provider
//             .get_matched_order(&order2.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();
//         assert_eq!(
//             updated_order2.source_swap.redeem_tx_hash.as_str(),
//             "2222222222222222222222222222222222222222"
//         );
//         assert_eq!(updated_order2.source_swap.secret.as_str(), secret.as_str());

//         // Verify refund for order3
//         let updated_order3 = provider
//             .get_matched_order(&order3.create_order.create_id)
//             .await
//             .unwrap()
//             .unwrap();
//         assert_eq!(
//             updated_order3.source_swap.refund_tx_hash.as_str(),
//             "3333333333333333333333333333333333333333"
//         );

//         // Clean up
//         delete_matched_order(&pool, &order1.create_order.create_id)
//             .await
//             .unwrap();
//         delete_matched_order(&pool, &order2.create_order.create_id)
//             .await
//             .unwrap();
//         delete_matched_order(&pool, &order3.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_detected_timestamps_updated() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();

//         delete_all_matched_orders(&pool).await.unwrap();

//         // Create three orders for testing initiate, redeem, and refund timestamps
//         let init_order = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();
//         let redeem_order = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();
//         let refund_order = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();

//         // Prepare redeem and refund orders with initiates (needed before redeem/refund)
//         simulate_test_swap_initiate(&pool, &redeem_order.source_swap.swap_id, None)
//             .await
//             .unwrap();
//         simulate_test_swap_initiate(&pool, &refund_order.source_swap.swap_id, None)
//             .await
//             .unwrap();

//         // Create detected timestamps (using fixed timestamps for testing)
//         let init_timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-01T10:00:00Z")
//             .unwrap()
//             .with_timezone(&chrono::Utc);
//         let redeem_timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-01T11:00:00Z")
//             .unwrap()
//             .with_timezone(&chrono::Utc);
//         let refund_timestamp = chrono::DateTime::parse_from_rfc3339("2024-01-01T12:00:00Z")
//             .unwrap()
//             .with_timezone(&chrono::Utc);

//         // Create events with detected timestamps
//         let init_event = create_test_swap_event(
//             &init_order.source_swap.swap_id,
//             SwapEventType::Initiate,
//             "init_tx_hash_1234567890123456789012345678901234567890",
//             init_order
//                 .source_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             100,
//             Some(init_timestamp),
//         );

//         let secret = OrderSecret::new("1".repeat(64)).unwrap();
//         let redeem_event = create_test_swap_event(
//             &redeem_order.source_swap.swap_id,
//             SwapEventType::Redeem(secret.clone()),
//             "redeem_tx_hash_1111111111111111111111111111111111111111",
//             redeem_order
//                 .source_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             200,
//             Some(redeem_timestamp),
//         );

//         let refund_event = create_test_swap_event(
//             &refund_order.source_swap.swap_id,
//             SwapEventType::Refund,
//             "refund_tx_hash_3333333333333333333333333333333333333333",
//             refund_order
//                 .source_swap
//                 .amount
//                 .to_string()
//                 .parse::<i64>()
//                 .unwrap(),
//             300,
//             Some(refund_timestamp),
//         );

//         // Process initiate event
//         let init_events_vec = vec![&init_event];
//         let init_events = Vec1::new(init_events_vec).unwrap();
//         swap_store.handle_inits(&init_events).await.unwrap();

//         // Process redeem event
//         let redeem_events_vec = vec![(&redeem_event, secret.to_string())];
//         let redeem_events = Vec1::new(redeem_events_vec).unwrap();
//         swap_store.handle_redeems(&redeem_events).await.unwrap();

//         // Process refund event
//         let refund_events_vec = vec![&refund_event];
//         let refund_events = Vec1::new(refund_events_vec).unwrap();
//         swap_store.handle_refunds(&refund_events).await.unwrap();

//         // Verify detected timestamps in create_orders.additional_data
//         // Query for initiate timestamp - PostgreSQL stores timestamps in JSONB as ISO strings
//         let init_timestamp_result: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
//             r#"
//             SELECT (additional_data->'bitcoin'->>$1)::text::timestamp AT TIME ZONE 'UTC'
//             FROM create_orders co
//             JOIN matched_orders mo ON co.create_id = mo.create_order_id
//             WHERE LOWER(mo.source_swap_id) = LOWER($2)
//             "#,
//         )
//         .bind(INITIATE_DETECTED_TIMESTAMP_KEY)
//         .bind(&init_order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();

//         assert!(
//             init_timestamp_result.is_some(),
//             "Initiate detected timestamp should be set"
//         );
//         let init_timestamp_parsed = init_timestamp_result.unwrap();
//         // Allow 1 second difference due to timestamp conversion
//         assert!(
//             (init_timestamp_parsed - init_timestamp).num_seconds().abs() <= 1,
//             "Initiate detected timestamp should match"
//         );

//         // Query for redeem timestamp
//         let redeem_timestamp_result: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
//             r#"
//             SELECT (additional_data->'bitcoin'->>$1)::text::timestamp AT TIME ZONE 'UTC'
//             FROM create_orders co
//             JOIN matched_orders mo ON co.create_id = mo.create_order_id
//             WHERE LOWER(mo.source_swap_id) = LOWER($2)
//             "#,
//         )
//         .bind(REDEEM_DETECTED_TIMESTAMP_KEY)
//         .bind(&redeem_order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();

//         assert!(
//             redeem_timestamp_result.is_some(),
//             "Redeem detected timestamp should be set"
//         );
//         let redeem_timestamp_parsed = redeem_timestamp_result.unwrap();
//         assert!(
//             (redeem_timestamp_parsed - redeem_timestamp)
//                 .num_seconds()
//                 .abs()
//                 <= 1,
//             "Redeem detected timestamp should match"
//         );

//         // Query for refund timestamp
//         let refund_timestamp_result: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
//             r#"
//             SELECT (additional_data->'bitcoin'->>$1)::text::timestamp AT TIME ZONE 'UTC'
//             FROM create_orders co
//             JOIN matched_orders mo ON co.create_id = mo.create_order_id
//             WHERE LOWER(mo.source_swap_id) = LOWER($2)
//             "#,
//         )
//         .bind(REFUND_DETECTED_TIMESTAMP_KEY)
//         .bind(&refund_order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();

//         assert!(
//             refund_timestamp_result.is_some(),
//             "Refund detected timestamp should be set"
//         );
//         let refund_timestamp_parsed = refund_timestamp_result.unwrap();
//         assert!(
//             (refund_timestamp_parsed - refund_timestamp)
//                 .num_seconds()
//                 .abs()
//                 <= 1,
//             "Refund detected timestamp should match"
//         );

//         // Clean up
//         delete_matched_order(&pool, &init_order.create_order.create_id)
//             .await
//             .unwrap();
//         delete_matched_order(&pool, &redeem_order.create_order.create_id)
//             .await
//             .unwrap();
//         delete_matched_order(&pool, &refund_order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_update_confirmations() {
//         let pool = pool().await;
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();

//         delete_all_matched_orders(&pool).await.unwrap();

//         // Create a matched order
//         let order_config = TestMatchedOrderConfig::default();
//         let order = create_matched_order(&provider, order_config).await.unwrap();

//         // Initiate the swap and set initiate_block_number to 100
//         simulate_test_swap_initiate(&pool, &order.source_swap.swap_id, None)
//             .await
//             .unwrap();

//         // Set initiate_block_number to 100 and required_confirmations to 3, current_confirmations to 0
//         sqlx::query(
//             r#"
//             UPDATE swaps
//             SET initiate_block_number = 100, required_confirmations = 3, current_confirmations = 0
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .execute(&pool)
//         .await
//         .unwrap();

//         // Test case 1: Current block is 102 (2 blocks ahead), should have 3 confirmations
//         // Formula: LEAST(3, GREATEST(102 - 100 + 1, 0)) = LEAST(3, 3) = 3
//         let current_block = 102i64;
//         swap_store
//             .update_confirmations(&order.source_swap.chain, current_block)
//             .await
//             .unwrap();

//         let current_confirmations: Option<i32> = sqlx::query_scalar(
//             r#"
//             SELECT current_confirmations
//             FROM swaps
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();

//         assert_eq!(
//             current_confirmations,
//             Some(3),
//             "Confirmations should be 3 when current_block is 102 and initiate_block is 100"
//         );

//         // Test case 2: Update again with current_block = 101 (should be 2 confirmations)
//         // Formula: LEAST(3, GREATEST(101 - 100 + 1, 0)) = LEAST(3, 2) = 2
//         let current_block = 101i64;
//         // First set current_confirmations to a different value so it gets updated
//         sqlx::query(
//             r#"
//             UPDATE swaps
//             SET current_confirmations = 0
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .execute(&pool)
//         .await
//         .unwrap();

//         swap_store
//             .update_confirmations(&order.source_swap.chain, current_block)
//             .await
//             .unwrap();

//         let current_confirmations: Option<i32> = sqlx::query_scalar(
//             r#"
//             SELECT current_confirmations
//             FROM swaps
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();
//         assert_eq!(
//             current_confirmations,
//             Some(2),
//             "Confirmations should be 2 when current_block is 101 and initiate_block is 100"
//         );

//         // Test case 3: Confirmations exceed required_confirmations (should cap at required)
//         // Set required_confirmations to 2, current_block to 105
//         // Formula: LEAST(2, GREATEST(105 - 100 + 1, 0)) = LEAST(2, 6) = 2
//         sqlx::query(
//             r#"
//             UPDATE swaps
//             SET required_confirmations = 2, current_confirmations = 0
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .execute(&pool)
//         .await
//         .unwrap();

//         let current_block = 105i64;
//         swap_store
//             .update_confirmations(&order.source_swap.chain, current_block)
//             .await
//             .unwrap();

//         let current_confirmations: Option<i32> = sqlx::query_scalar(
//             r#"
//             SELECT current_confirmations
//             FROM swaps
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();
//         assert_eq!(
//             current_confirmations,
//             Some(2),
//             "Confirmations should be capped at required_confirmations (2)"
//         );

//         // Test case 5: current_confirmations already equals required_confirmations (should not update)
//         // Set both to 3, then try to update - should not change
//         sqlx::query(
//             r#"
//             UPDATE swaps
//             SET required_confirmations = 3, current_confirmations = 3
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .execute(&pool)
//         .await
//         .unwrap();

//         let current_block = 110i64;
//         swap_store
//             .update_confirmations(&order.source_swap.chain, current_block)
//             .await
//             .unwrap();

//         let current_confirmations: Option<i32> = sqlx::query_scalar(
//             r#"
//             SELECT current_confirmations
//             FROM swaps
//             WHERE swap_id = $1
//             "#,
//         )
//         .bind(&order.source_swap.swap_id)
//         .fetch_optional(&pool)
//         .await
//         .unwrap();
//         assert_eq!(
//             current_confirmations,
//             Some(3),
//             "Confirmations should remain 3 when already equal to required_confirmations"
//         );

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }

//     #[tokio::test]
//     #[serial]
//     async fn test_mark_blacklisted() {
//         let pool = pool().await;
//         let provider = OrderbookProvider::from_db_url(DB_URL).await.unwrap();
//         let swap_store = GardenSwapStore::new(pool.clone(), 120);

//         delete_all_matched_orders(&pool).await.unwrap();

//         let order = create_matched_order(&provider, TestMatchedOrderConfig::default())
//             .await
//             .unwrap();

//         // Verify additional_data has no is_blacklisted before marking
//         let before: Option<serde_json::Value> =
//             sqlx::query_scalar(r#"SELECT additional_data FROM create_orders WHERE create_id = $1"#)
//                 .bind(&order.create_order.create_id)
//                 .fetch_optional(&pool)
//                 .await
//                 .unwrap();

//         let is_blacklisted_before = before
//             .as_ref()
//             .and_then(|v| v.get("is_blacklisted"))
//             .and_then(|v| v.as_bool())
//             .unwrap_or(false);
//         assert!(
//             !is_blacklisted_before,
//             "should not be blacklisted initially"
//         );

//         // Mark via source swap id
//         swap_store
//             .mark_blacklisted(&Vec1::new(vec![order.source_swap.swap_id.clone()]).unwrap())
//             .await
//             .unwrap();

//         let after: Option<serde_json::Value> =
//             sqlx::query_scalar(r#"SELECT additional_data FROM create_orders WHERE create_id = $1"#)
//                 .bind(&order.create_order.create_id)
//                 .fetch_optional(&pool)
//                 .await
//                 .unwrap();

//         let is_blacklisted_after = after
//             .as_ref()
//             .and_then(|v| v.get("is_blacklisted"))
//             .and_then(|v| v.as_bool())
//             .unwrap_or(false);
//         assert!(
//             is_blacklisted_after,
//             "should be blacklisted after marking via source swap id"
//         );

//         // Reset additional_data for next assertion
//         sqlx::query(
//             r#"UPDATE create_orders SET additional_data = '{}'::jsonb WHERE create_id = $1"#,
//         )
//         .bind(&order.create_order.create_id)
//         .execute(&pool)
//         .await
//         .unwrap();

//         // Mark via destination swap id
//         swap_store
//             .mark_blacklisted(&Vec1::new(vec![order.destination_swap.swap_id.clone()]).unwrap())
//             .await
//             .unwrap();

//         let after_dest: Option<serde_json::Value> =
//             sqlx::query_scalar(r#"SELECT additional_data FROM create_orders WHERE create_id = $1"#)
//                 .bind(&order.create_order.create_id)
//                 .fetch_optional(&pool)
//                 .await
//                 .unwrap();

//         let is_blacklisted_dest = after_dest
//             .as_ref()
//             .and_then(|v| v.get("is_blacklisted"))
//             .and_then(|v| v.as_bool())
//             .unwrap_or(false);
//         assert!(
//             is_blacklisted_dest,
//             "should be blacklisted after marking via destination swap id"
//         );

//         // Clean up
//         delete_matched_order(&pool, &order.create_order.create_id)
//             .await
//             .unwrap();
//     }
// }
