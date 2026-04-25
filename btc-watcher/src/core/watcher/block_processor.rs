use super::tx_processor::store_swap_events;
use crate::core::{SwapStore, TxEventParser, remove_duplicates};
use bitcoin::Block;
use eyre::Result;
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct BlockProcessor {
    block_receiver: mpsc::Receiver<Block>,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
    tx_event_processor: Arc<TxEventParser>,
}

impl BlockProcessor {
    pub fn new(
        block_receiver: mpsc::Receiver<Block>,
        swap_store: Arc<dyn SwapStore + Send + Sync>,
        tx_event_processor: Arc<TxEventParser>,
    ) -> Self {
        Self {
            block_receiver,
            swap_store,
            tx_event_processor,
        }
    }

    pub async fn process(&mut self) {
        while let Some(block) = self.block_receiver.recv().await {
            if let Err(e) = self.process_block(block).await {
                tracing::error!("Failed to process block: {e}");
            }
        }
        tracing::warn!("Block receiver channel closed");
    }

    async fn process_block(&self, block: Block) -> Result<()> {
        let block_ts = chrono::DateTime::from_timestamp(block.header.time as i64, 0)
            .ok_or_else(|| eyre::eyre!("Failed to get block timestamp"))?;

        let block_height = block
            .bip34_block_height()
            .map_err(|e| eyre::eyre!("Failed to get block height: {e}"))?;

        let mut swap_events = Vec::new();
        for tx in block.txdata {
            swap_events.extend(
                self.tx_event_processor
                    .parse_swap_events(tx, block_height, Some(block_ts), None)
                    .await,
            );
        }
        swap_events = remove_duplicates(&swap_events);

        store_swap_events(swap_events, self.swap_store.clone()).await
    }
}
