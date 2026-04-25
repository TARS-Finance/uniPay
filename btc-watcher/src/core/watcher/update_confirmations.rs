use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::core::{BlockchainIndexer, SwapStore};
use std::{sync::Arc, time::Duration};

const UPDATE_CONFIRMATIONS_INTERVAL: Duration = Duration::from_secs(2);

pub async fn update_confirmations(
    chain: &str,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
    indexer: Arc<dyn BlockchainIndexer + Send + Sync>,
) {
    loop {
        sleep(UPDATE_CONFIRMATIONS_INTERVAL).await;

        let current_block = match indexer.get_block_height().await {
            Ok(block) => block,
            Err(e) => {
                error!("Failed to get block height: {e}");
                continue;
            }
        };

        backfill_initiate_block_numbers(chain, swap_store.as_ref(), indexer.as_ref()).await;

        if let Err(e) = swap_store
            .update_confirmations(chain, current_block as i64)
            .await
        {
            error!("Failed to update confirmations: {e}");
            continue;
        }

        info!(chain = %chain, current_block = %current_block, "Updated confirmations");
    }
}

async fn backfill_initiate_block_numbers(
    chain: &str,
    swap_store: &(dyn SwapStore + Send + Sync),
    indexer: &(dyn BlockchainIndexer + Send + Sync),
) {
    let rows = match swap_store.get_swaps_missing_initiate_block(chain).await {
        Ok(rows) => rows,
        Err(e) => {
            error!("Failed to fetch swaps missing initiate_block_number: {e}");
            return;
        }
    };

    for (swap_id, initiate_tx_hash) in rows {
        let txid = initiate_tx_hash
            .split(',')
            .next()
            .unwrap_or(&initiate_tx_hash)
            .trim()
            .split(':')
            .next()
            .unwrap_or("")
            .to_string();
        if txid.is_empty() {
            continue;
        }

        let metadata = match indexer.get_tx(&txid).await {
            Ok(m) => m,
            Err(e) => {
                warn!(swap_id = %swap_id, txid = %txid, "get_tx failed: {e}");
                continue;
            }
        };

        let block_height = match metadata.status.block_height {
            Some(h) if metadata.status.confirmed => h as i64,
            _ => continue,
        };

        // filled_amount = sum of outputs paying the HTLC address (swap_id is the bech32 address)
        let filled_amount: i64 = metadata
            .vout
            .iter()
            .filter(|o| o.script_pubkey_address == swap_id)
            .map(|o| o.value as i64)
            .sum();

        match swap_store
            .backfill_initiate(&swap_id, block_height, filled_amount)
            .await
        {
            Ok(()) => info!(
                swap_id = %swap_id,
                txid = %txid,
                block_height = %block_height,
                filled_amount = %filled_amount,
                "Backfilled initiate"
            ),
            Err(e) => error!(swap_id = %swap_id, "Failed to backfill initiate: {e}"),
        }
    }
}
