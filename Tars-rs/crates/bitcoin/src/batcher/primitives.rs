use crate::{
    batcher::{traits::TxBatcher, validate::validate_spend_requests},
    ArcIndexer, Utxo,
};
use bitcoin::{key::Keypair, Address, ScriptBuf, Witness};
use eyre::{eyre, Result};
use std::sync::Arc;

/// Represents a request to spend Bitcoin from specific UTXOs (Unspent Transaction Outputs).
///
/// This struct contains the necessary information to create a spend transaction, including the
/// UTXOs to be spent, the recipient address, and the keypair for signing the transaction.
///
/// # Fields
/// * `id` - A unique identifier for the spend request.
/// * `utxos` - A list of UTXOs to be spent in the transaction.
/// * `witness` - The witness data used for signing the transaction.
/// * `keypair` - The keypair used to sign the transaction.
/// * `recipient` - The address of the recipient for the transaction.
/// * `script` - The script associated with the transaction outputs.
/// * `htlc_address` - The address associated with the HTLC (Hashed TimeLock Contract) for the transaction.
#[derive(Debug, Clone)]
pub struct SpendRequest {
    /// A unique identifier for the spend request.
    pub id: String,

    /// A list of UTXOs to be spent in the transaction.
    pub utxos: Vec<Utxo>,

    /// The witness data used for signing the transaction.
    pub witness: Witness,

    /// The keypair used to sign the transaction.
    pub keypair: Keypair,

    /// The address of the recipient for the transaction.
    pub recipient: Address,

    /// The script associated with the transaction outputs.
    pub script: ScriptBuf,

    /// The address associated with the HTLC (Hashed TimeLock Contract) for the transaction.
    pub htlc_address: Address,
}

/// A validated fee rate for Bitcoin transactions.
#[derive(Debug, Clone, Copy)]
pub struct FeeRate(f64);

impl FeeRate {
    /// Creates a new fee rate, validating that it's positive.
    ///
    /// # Arguments
    /// * `rate` - The fee rate in satoshis per vbyte.
    ///
    /// # Returns
    /// * `Ok(FeeRate)` if the rate is positive.
    /// * `Err` if the rate is not positive.
    pub fn new(rate: f64) -> Result<Self> {
        if rate <= 0.0 {
            return Err(eyre!("Fee rate must be positive, got: {}", rate));
        }
        Ok(FeeRate(rate))
    }

    /// Returns the fee rate value.
    pub fn value(&self) -> f64 {
        self.0
    }
}
/// A collection of validated [`SpendRequest`]s.
///
/// This wrapper ensures that only spend requests that have been successfully
/// validated against the indexer are included.
#[derive(Debug, Clone)]
pub struct ValidSpendRequests(Vec<SpendRequest>);

impl ValidSpendRequests {
    /// Validates a list of spend requests and returns a [`ValidSpendRequests`] instance.
    ///
    /// # Arguments
    /// * `spend_requests` - A list of spend requests to validate.
    /// * `indexer` - The [`ArcIndexer`] instance used to fetch and verify UTXO data.
    ///
    /// # Returns
    /// * `Ok(ValidSpendRequests)` if all valid spend requests are collected.
    /// * `Err(eyre::Report)` if validation fails (e.g., due to invalid or missing UTXOs).
    pub async fn validate(spend_requests: Vec<SpendRequest>, indexer: &ArcIndexer) -> Result<Self> {
        let (valid_spend_requests, _) = Self::validate_and_split(spend_requests, indexer).await;

        Ok(valid_spend_requests)
    }

    /// Validates a list of spend requests and separates them into valid and invalid groups.
    ///
    /// # Arguments
    /// * `spend_requests` - A list of spend requests to validate.
    /// * `indexer` - The [`ArcIndexer`] instance used to fetch and verify UTXO data.
    ///
    /// # Returns
    /// A tuple containing:
    /// * `ValidSpendRequests` - The subset of spend requests that passed validation.
    /// * `Vec<SpendRequest>` - The subset of spend requests that failed validation.
    pub async fn validate_and_split(
        spend_requests: Vec<SpendRequest>,
        indexer: &ArcIndexer,
    ) -> (Self, Vec<SpendRequest>) {
        // Validate the spend requests.
        let (valid_spend_requests, invalid_spend_requests) =
            validate_spend_requests(&spend_requests, indexer).await;

        (Self(valid_spend_requests), invalid_spend_requests)
    }

    /// Returns a reference to the underlying vector of validated [`SpendRequest`]s.
    ///
    /// # Returns
    /// A slice of validated spend requests.
    pub fn as_ref(&self) -> &[SpendRequest] {
        &self.0
    }
}

/// A type alias for a thread-safe, reference-counted `TxBatcher` trait object.
pub type ArcTxBatcher = Arc<dyn TxBatcher + Send + Sync>;
