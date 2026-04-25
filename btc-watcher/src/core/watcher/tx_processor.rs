use crate::core::{RPCClient, SwapEvent, SwapStore, TxEventParser, Vec1, remove_duplicates};
use bitcoin::Transaction;
use chrono::{DateTime, Utc};
use eyre::Result;
use moka::future::Cache as MokaCache;
use std::{sync::Arc, time::Duration};
use tokio::{sync::mpsc, time::sleep};

const MAX_TXS_BATCH_SIZE: usize = 256;
const PROCESSED_EVENTS_TTL: Duration = Duration::from_secs(4800); // 1 hour + 20 minutes
const REVALIDATE_TX_BUFFER: Duration = Duration::from_secs(3 * 60); // 3 minutes

// Cache key: (swap_id, tx_hash, block_number)
type ProcessedEventKey = (String, String, i64);

pub struct TxIndexer {
    tx_receiver: mpsc::Receiver<Transaction>,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
    rpc_client: Arc<dyn RPCClient + Send + Sync>,
    tx_event_parser: Arc<TxEventParser>,
    processed_events_cache: Arc<MokaCache<ProcessedEventKey, i64>>,
}

impl TxIndexer {
    pub fn new(
        tx_receiver: mpsc::Receiver<Transaction>,
        swap_store: Arc<dyn SwapStore + Send + Sync>,
        rpc_client: Arc<dyn RPCClient + Send + Sync>,
        tx_event_parser: Arc<TxEventParser>,
    ) -> Self {
        let processed_events_cache = Arc::new(
            MokaCache::builder()
                .time_to_live(PROCESSED_EVENTS_TTL)
                .build(),
        );

        Self {
            tx_receiver,
            swap_store,
            rpc_client,
            tx_event_parser,
            processed_events_cache,
        }
    }

    pub async fn index(&mut self) {
        loop {
            let mut txs = Vec::with_capacity(64);
            let _ = self
                .tx_receiver
                .recv_many(&mut txs, MAX_TXS_BATCH_SIZE)
                .await;

            if let Err(e) = self.process_batch(txs).await {
                tracing::error!("Failed to process txs batch: {e}");
            }

            sleep(Duration::from_millis(100)).await;
        }
    }

    async fn process_batch(&self, txs: Vec<Transaction>) -> Result<()> {
        let now: DateTime<Utc> = Utc::now();

        let mut swap_events = Vec::new();
        for tx in txs {
            swap_events.extend(
                self.tx_event_parser
                    .parse_swap_events(tx, 0, None, Some(now))
                    .await,
            );
        }
        swap_events = remove_duplicates(&swap_events);

        let new_events = self.dedup_processed_events(swap_events, &now).await;

        store_swap_events(new_events, self.swap_store.clone()).await
    }

    /// Filters out duplicate/already-processed swap events.
    /// Re-includes a previously seen event only if the revalidation buffer has elapsed
    /// and the transaction is back in the mempool.
    async fn dedup_processed_events(
        &self,
        swap_events: Vec<SwapEvent>,
        now: &DateTime<Utc>,
    ) -> Vec<SwapEvent> {
        let mut new_events = Vec::new();

        for event in swap_events {
            let key: ProcessedEventKey = (
                event.swap_id.clone(),
                event.tx_info.tx_hash.clone(),
                event.tx_info.block_number,
            );

            match self.processed_events_cache.get(&key).await {
                Some(timestamp) => {
                    // REVALIDATE_TX_BUFFER for ignoring zmq republished txs
                    if timestamp + REVALIDATE_TX_BUFFER.as_secs() as i64 > now.timestamp() {
                        continue;
                    }

                    // Check if the tx is still in mempool
                    // tx_hash format is "{txid}:{block_height}", RPC expects just the txid
                    let txid = event.tx_info.tx_hash.rsplit_once(':').map_or(
                        event.tx_info.tx_hash.as_str(),
                        |(txid, _)| txid,
                    );
                    match self.rpc_client.get_mempool_entry(txid).await
                    {
                        Ok(entry) => {
                            if entry.is_some() {
                                new_events.push(event);
                            }
                            self.processed_events_cache
                                .insert(key, now.timestamp())
                                .await;
                        }
                        Err(e) => {
                            tracing::error!("Failed to get mempool entry: {e}");
                            continue;
                        }
                    }
                }

                None => {
                    self.processed_events_cache
                        .insert(key, now.timestamp())
                        .await;
                    new_events.push(event);
                }
            }
        }

        new_events
    }
}

pub(super) async fn store_swap_events(
    swap_events: Vec<SwapEvent>,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
) -> Result<()> {
    // filter blacklisted and non blacklisted events
    let blacklisted_swap_ids = swap_events
        .iter()
        .filter(|event| event.is_blacklisted)
        .map(|event| event.swap_id.clone())
        .collect::<Vec<String>>();

    let non_blacklisted_events = swap_events
        .into_iter()
        .filter(|event| !event.is_blacklisted)
        .collect::<Vec<_>>();

    // update non-blacklisted events in swap store
    if let Ok(validated_events) = Vec1::new(non_blacklisted_events)
        && let Err(e) = swap_store.update_swaps(validated_events).await
    {
        tracing::error!("Failed to update swap store: {e}");
    }

    // mark blacklisted swaps in swap store
    if let Ok(blacklisted_ids) = Vec1::new(blacklisted_swap_ids)
        && let Err(e) = swap_store.mark_blacklisted(&blacklisted_ids).await
    {
        tracing::error!("Failed to mark blacklisted swaps: {e}");
    }

    Ok(())
}
