use crate::{
    errors::OrderbookError,
    primitives::{
        self, Claim, MatchedOrderVerbose, OrderQueryFilters, PaginatedData, StatsQueryFilters,
        SwapChain,
    },
};
use async_trait::async_trait;
use bigdecimal::{num_bigint::BigInt, BigDecimal};
use eyre::Result;
use mockall::automock;
use std::collections::HashMap;

#[automock]
#[async_trait]
pub trait Orderbook {
    //
    // Query methods - Swap operations
    //

    /// Returns the swap model
    async fn get_swap(
        &self,
        order_id: &str,
        chain: SwapChain,
    ) -> Result<Option<primitives::SingleSwap>, OrderbookError>;

    //
    // Query methods - Order operations
    //

    /// Returns a matched order with the given create_id
    async fn get_matched_order(
        &self,
        create_id: &str,
    ) -> Result<Option<primitives::MatchedOrderVerbose>, OrderbookError>;

    /// Returns a user's matched orders in paginated format
    async fn get_matched_orders(
        &self,
        user: &str,
        filters: OrderQueryFilters,
    ) -> Result<PaginatedData<primitives::MatchedOrderVerbose>, OrderbookError>;

    /// Returns all the matched orders in paginated format
    async fn get_all_matched_orders(
        &self,
        filters: OrderQueryFilters,
    ) -> Result<PaginatedData<primitives::MatchedOrderVerbose>, OrderbookError>;

    /// Returns all the filler pending orders
    async fn get_filler_pending_orders(
        &self,
        chain_name: &str,
        filler_id: &str,
    ) -> Result<Vec<primitives::MatchedOrderVerbose>, OrderbookError>;

    /// Returns all the pending orders
    async fn get_solver_pending_orders(
        &self,
    ) -> Result<Vec<primitives::MatchedOrderVerbose>, OrderbookError>;

    /// Returns the total amount yet to be initiated by solver which is already initiated by user
    async fn get_solver_committed_funds(
        &self,
        addr: &str,
        chain: &str,
        asset: &str,
    ) -> Result<BigDecimal, OrderbookError>;

    //
    // Update methods - Swap lifecycle operations
    //

    /// Updates a swap with initiate details
    async fn update_swap_initiate(
        &self,
        order_id: &str,
        filled_amount: BigDecimal,
        initiate_tx_hash: &str,
        initiate_block_number: i64,
        initiate_timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), OrderbookError>;

    /// Updates a swap with redeem details
    async fn update_swap_redeem(
        &self,
        order_id: &str,
        redeem_tx_hash: &str,
        secret: &str,
        redeem_block_number: i64,
        redeem_timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), OrderbookError>;

    /// Updates a swap with refund details
    async fn update_swap_refund(
        &self,
        order_id: &str,
        refund_tx_hash: &str,
        refund_block_number: i64,
        refund_timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), OrderbookError>;

    /// Updates current_confirmations of all the swaps on given chain
    async fn update_confirmations(
        &self,
        chain_identifier: &str,
        latest_block: u64,
    ) -> Result<(), OrderbookError>;

    /// Checks if an order with same secret_hash already exists
    async fn exists(&self, secret_hash: &str) -> Result<bool, OrderbookError>;

    //
    // Update methods - Additional data operations
    //

    /// Adds bitcoin instant_refund_tx_bytes to additional_data
    async fn add_instant_refund_sacp(
        &self,
        order_id: &str,
        instant_refund_tx_bytes: &str,
    ) -> Result<(), OrderbookError>;

    /// Adds bitcoin redeem_tx_bytes to additional_data
    async fn add_redeem_sacp(
        &self,
        order_id: &str,
        redeem_tx_bytes: &str,
        redeem_tx_id: &str,
        secret: &str,
    ) -> Result<(), OrderbookError>;

    /// Returns the swaps volume for a given query
    async fn get_volume(
        &self,
        query: StatsQueryFilters,
        asset_decimals: &HashMap<(String, String), u32>, // (chain, asset) -> decimals mapping
    ) -> Result<BigDecimal, OrderbookError>;

    /// Returns the fees for a given query
    async fn get_fees(
        &self,
        query: StatsQueryFilters,
        asset_decimals: &HashMap<(String, String), u32>, // (chain, asset) -> decimals mapping
    ) -> Result<BigDecimal, OrderbookError>;

    /// Returns the fees for a given integrator
    async fn get_integrator_fees(&self, integrator: &str) -> Result<Vec<Claim>, OrderbookError>;

    /// Inserts both swaps, create_order and matched_order into the orderbook
    async fn create_matched_order(
        &self,
        matched_order: &MatchedOrderVerbose,
    ) -> Result<(), OrderbookError>;

    /// Returns the volume and fees for a given query
    async fn get_volume_and_fees(
        &self,
        query: StatsQueryFilters,
        asset_decimals: &HashMap<(String, String), u32>, // (chain, asset) -> decimals mapping
    ) -> Result<(BigInt, BigInt), OrderbookError>;
}
