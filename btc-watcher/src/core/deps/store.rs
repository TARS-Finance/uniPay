use crate::core::{Swap, SwapEvent, Vec1};
use async_trait::async_trait;
use eyre::Result;

#[async_trait]
pub trait SwapStore {
    /// Returns all pending swaps.
    /// The query should also check the deadline, with an additional buffer of 30 minutes.
    async fn get_swaps(&self, chain: &str) -> Result<Vec<Swap>>;

    /// Updates the swap store with the given swap events.
    async fn update_swaps(&self, swap_events: Vec1<SwapEvent>) -> Result<()>;

    /// Updates the confirmations for the given chain.
    async fn update_confirmations(&self, chain: &str, current_block: i64) -> Result<()>;

    /// Returns swaps on the chain that have an initiate_tx_hash but no initiate_block_number
    /// yet. The vec contains (swap_id, initiate_tx_hash).
    async fn get_swaps_missing_initiate_block(
        &self,
        chain: &str,
    ) -> Result<Vec<(String, String)>>;

    /// Sets initiate_block_number (and filled_amount when currently zero) for a swap.
    async fn backfill_initiate(
        &self,
        swap_id: &str,
        block_number: i64,
        filled_amount: i64,
    ) -> Result<()>;

    /// Marks swaps as blacklisted by adding is_blacklisted: true to the additional_data in create_orders table
    async fn mark_blacklisted(&self, swap_ids: &Vec1<String>) -> Result<()>;
}
