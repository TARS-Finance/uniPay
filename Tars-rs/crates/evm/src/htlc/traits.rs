use crate::{
    errors::HTLCError,
    primitives::{CallParams, InitiateTypedData, SwapInfo},
};
use alloy::{
    dyn_abi::Eip712Domain,
    primitives::{Address, Bytes, FixedBytes},
    signers::{k256::ecdsa::SigningKey, local::LocalSigner},
};
use async_trait::async_trait;
use orderbook::primitives::EVMSwap;

#[async_trait]
pub trait HTLCInterface: Send + Sync {
    /// Generates the calldata required to initiate an HTLC swap for the given asset.
    ///
    /// # Arguments
    ///
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be swapped.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` required to initiate the swap, or an `HTLCError`.
    async fn initiate_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Generates the calldata required to initiate an HTLC swap for the given asset from a contract.
    ///
    /// # Arguments
    ///
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be swapped.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` required to initiate the swap, or an `HTLCError`.
    async fn initiate_on_behalf_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Generates the calldata to initiate an HTLC swap using a provided signature.
    ///
    /// # Arguments
    ///
    /// * `signature` - Signature bytes for the swap.
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be swapped.
    /// * `domain` - EIP-712 domain for signing.
    /// * `signer` - Signer for signing the initiate.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` for the signed initiation, or an `HTLCError`.
    async fn initiate_with_signature_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
        domain: &Eip712Domain,
        signer: &LocalSigner<SigningKey>,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Generates the calldata to initiate an HTLC swap using a user-provided signature.
    ///
    /// # Arguments
    ///
    /// * `signature` - User's signature bytes.
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be swapped.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` for the user-signed initiation, or an `HTLCError`.
    async fn initiate_with_user_signature_calldata(
        &self,
        signature: &Bytes,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Generates the calldata required to redeem an HTLC swap using the secret.
    ///
    /// # Arguments
    ///
    /// * `secret` - Secret bytes to unlock the swap.
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be redeemed.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` for redeeming the swap, or an `HTLCError`.
    async fn redeem_calldata(
        &self,
        secret: &Bytes,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Generates the calldata required to refund an expired HTLC swap.
    ///
    /// # Arguments
    ///
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be refunded.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` for refunding the swap, or an `HTLCError`.
    async fn refund_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Generates the calldata required for an instant refund of an HTLC swap using a signature.
    ///
    /// # Arguments
    ///
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the asset to be refunded.
    /// * `domain` - EIP-712 domain for signing.
    /// * `signer` - Signer for signing the refund.
    ///
    /// # Returns
    ///
    /// A vector of `CallParams` for the instant refund, or an `HTLCError`.
    async fn instant_refund_calldata(
        &self,
        swap: &EVMSwap,
        asset: &Address,
        domain: &Eip712Domain,
        signer: &LocalSigner<SigningKey>,
    ) -> Result<Vec<CallParams>, HTLCError>;

    /// Returns the EIP-712 domain used for signing and verifying messages.
    ///
    /// # Returns
    ///
    /// The `Eip712Domain` or an `HTLCError`.
    async fn domain(&self) -> Result<Eip712Domain, HTLCError>;

    /// Retrieves the on-chain order information for a given swap.
    ///
    /// # Arguments
    ///
    /// * `swap` - Reference to the swap details.
    ///
    /// # Returns
    ///
    /// The `SwapInfo` or an `HTLCError`.
    async fn get_order(&self, swap: &EVMSwap) -> Result<SwapInfo, HTLCError>;

    /// Computes the unique order ID for a given chain and swap.
    ///
    /// # Arguments
    ///
    /// * `chain_id` - The chain ID.
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Address of the htlc contract.
    ///
    /// # Returns
    ///
    /// The order ID as a `FixedBytes<32>`, or an `HTLCError`.
    fn get_order_id(
        &self,
        chain_id: u64,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<FixedBytes<32>, HTLCError>;

    async fn get_typed_data(
        &self,
        swap: &EVMSwap,
        domain: Eip712Domain,
    ) -> Result<InitiateTypedData, HTLCError>;

    /// Returns the token address for the HTLC contract.
    ///
    /// # Returns
    ///
    /// The token address as an `Address`, or an `HTLCError`.
    async fn token(&self) -> Result<Address, HTLCError>;

    /// Returns a boxed clone of the current HTLC interface.
    ///
    /// # Returns
    ///
    /// A boxed trait object implementing `HTLCInterface`.
    fn clone_box(&self) -> Box<dyn HTLCInterface + Send + Sync>;
}
