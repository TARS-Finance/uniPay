use crate::{
    errors::MulticallError,
    primitives::{UnipayActionType, CallParams},
};
use alloy::primitives::Address;
use orderbook::primitives::EVMSwap;

/// Trait for handling different types of contract actions and generating their corresponding calldata.
///
/// This trait provides a unified interface for generating transaction calldata across different contract types
/// and action categories. Currently supports HTLC operations (initiate, redeem, refund) but is designed to be
/// extensible for other contract types like HTLC registries, token contracts, or any future contract integrations.
/// 
/// The trait abstracts away the specific implementation details of each action type, allowing the caller
/// to work with a consistent interface regardless of the underlying contract or operation being performed.
///
/// See [`ActionType`] for the complete list of supported action types.
#[async_trait::async_trait]
pub trait UnipayActionHandler : Send + Sync {
    /// Generates calldata for a specific contract action.
    ///
    /// This method takes an action type (which can be any variant of [`ActionType`]), swap details, and asset
    /// address, then generates the appropriate calldata that can be used to execute the action on the blockchain.
    /// The calldata includes the target contract address, function call data, and any value to be sent with
    /// the transaction.
    ///
    /// The method handles different action types uniformly:
    /// - **HTLC Actions**: Generate calldata for HTLC contract operations (initiate, redeem, refund, etc.)
    /// - **Future Actions**: Can be extended to support other contract types like registries, token operations, etc.
    ///
    /// # Arguments
    ///
    /// * `action` - The type of action to perform, can be any variant of [`ActionType`] (e.g., 
    ///   `ActionType::HTLC(HTLCAction::Initiate)`, `ActionType::HTLCRegistry(...)`)
    /// * `swap` - The swap details containing all necessary parameters for the action
    /// * `asset` - The address of the target contract or asset to interact with
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<CallParams>)` - A vector of call parameters if calldata generation succeeds
    /// * `Err(ContractError)` - An error if calldata generation fails
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let htlc_calldata = action_handler.get_calldata(
    ///     &ActionType::HTLC(HTLCAction::Initiate),
    ///     &swap,
    ///     &htlc_contract_address
    /// ).await?;
    /// ```
    async fn get_calldata(
        &self,
        action: &UnipayActionType,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, MulticallError>;
}
