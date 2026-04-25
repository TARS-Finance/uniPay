use crate::core::{Cache, Swap, SwapStore};
use std::{sync::Arc, time::Duration};
use tokio::time::sleep;
use tracing::{error, info};

pub async fn listen_for_pending_swaps(
    chain_name: &str,
    poll_interval_ms: u64,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
    swap_cache: Arc<dyn Cache<String, Swap> + Send + Sync>,
) {
    loop {
        let swaps = match swap_store.get_swaps(chain_name).await {
            Ok(swaps) => swaps,
            Err(e) => {
                error!("Failed to get pending swaps: {e}");
                sleep(Duration::from_millis(poll_interval_ms)).await;
                continue;
            }
        };

        info!(pending_swaps = %swaps.len(), "updated swap cache with pending swaps");

        let kv_pairs: Vec<(String, Swap)> = swaps
            .into_iter()
            .map(|swap| (swap.swap_id.clone(), swap))
            .collect();
        swap_cache.clear().await;
        swap_cache.set(&kv_pairs).await;

        sleep(Duration::from_millis(poll_interval_ms)).await;
    }
}
