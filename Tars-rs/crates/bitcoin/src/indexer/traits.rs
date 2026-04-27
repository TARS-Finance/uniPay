use crate::indexer::primitives::{OutSpend, TransactionMetadata};

use super::primitives::Utxo;
use async_trait::async_trait;
use bitcoin::{Address, Transaction};
use eyre::Result;
use mockall::automock;

#[automock]
#[async_trait]
pub trait Indexer: Send + Sync {
    /// Retrieves the transaction hex by its transaction ID and converts it into a transaction object.
    ///
    /// # Arguments
    ///
    /// * `txid` - The transaction ID as a string
    ///
    /// # Returns
    ///
    /// The transaction if found, or an error if the transaction could not be retrieved
    /// or parsed.
    async fn get_tx_hex(&self, txid: &str) -> Result<Transaction>;

    /// Retrieves detailed metadata about a transaction by its transaction ID.
    ///
    /// # Arguments
    ///
    /// * `txid` - The transaction ID as a string
    ///
    /// # Returns
    ///
    /// The transaction metadata if found, or an error if the transaction could not be retrieved
    /// or parsed.
    async fn get_tx(&self, txid: &str) -> Result<TransactionMetadata>;

    /// Submits a transaction to the Bitcoin network.
    ///
    /// # Arguments
    ///
    /// * `tx` - The transaction to submit
    ///
    /// # Returns
    ///
    /// `Ok(())` if the transaction was successfully submitted, or an error if
    /// the submission failed.
    async fn submit_tx(&self, tx: &Transaction) -> Result<()>;

    /// Retrieves the current block height of the Bitcoin blockchain.
    ///
    /// # Returns
    ///
    /// The current block height as a u64, or an error if the request failed.
    async fn get_block_height(&self) -> Result<u64>;

    /// Retrieves the unspent transaction outputs (UTXOs) for a given address.
    ///
    /// # Arguments
    ///
    /// * `address` - The Bitcoin address to query
    ///
    /// # Returns
    ///
    /// A vector of UTXOs associated with the address, or an error if the request failed.
    async fn get_utxos(&self, address: &Address) -> Result<Vec<Utxo>>;

    /// Retrieves the spending status of all outputs for a given transaction.
    ///
    /// # Arguments
    ///
    /// * `txid` - The transaction ID as a string
    ///
    /// # Returns
    ///
    /// A vector of OutSpends containing the spending status of each output in the transaction,
    /// or an error if the request failed.
    async fn get_tx_outspends(&self, txid: &str) -> Result<Vec<OutSpend>>;
}
