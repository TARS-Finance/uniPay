use crate::{Indexer, UtxoJson};
use bitcoin::{OutPoint, Txid};
use eyre::{eyre, Result};
use serde::Deserialize;
use std::{str::FromStr, sync::Arc};

/// Represents an unspent transaction output (UTXO) in the Bitcoin network.
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct Utxo {
    /// The transaction ID that contains this output
    pub txid: Txid,

    /// The output index (vout) within the transaction
    pub vout: u32,

    /// The value of this UTXO in satoshis (1 BTC = 100,000,000 satoshis)
    pub value: u64,

    /// The status of the UTXO
    pub status: UtxoStatus,
}

impl TryFrom<&UtxoJson> for Utxo {
    type Error = eyre::Report;

    /// Attempts to convert a [`UtxoJson`] object into a [`Utxo`].
    ///
    /// # Arguments
    /// * `utxo_json` - A reference to the [`UtxoJson`] instance that will be converted.
    ///
    /// # Returns
    /// * `Ok(Utxo)` if the conversion succeeds.
    /// * `Err(eyre::Report)` if the `txid` field cannot be parsed into a valid [`Txid`].
    ///
    /// # Errors
    /// Returns an error if:
    /// * The `txid` string in the input cannot be parsed into a [`Txid`].
    fn try_from(utxo_json: &UtxoJson) -> Result<Self, Self::Error> {
        let txid = Txid::from_str(&utxo_json.txid)
            .map_err(|e| eyre!("Failed to parse utxo json : {:#?}", e))?;

        Ok(Utxo {
            status: utxo_json.status.clone(),
            txid,
            value: utxo_json.value,
            vout: utxo_json.vout,
        })
    }
}
/// Represents the status of a UTXO in the Bitcoin network.
#[derive(Debug, Clone, Deserialize, Default, Hash, Eq, PartialEq)]
pub struct UtxoStatus {
    /// Whether the UTXO is confirmed
    pub confirmed: bool,

    /// The block height of the UTXO
    pub block_height: Option<u64>,
}

impl Utxo {
    /// Converts the `Utxo` into a string representation
    pub fn to_string(&self) -> String {
        format!("{}:{}", self.txid, self.vout)
    }

    /// Converts the `Utxo` into an `OutPoint` structure.
    ///
    /// This method maps the UTXO to an `OutPoint`, which is commonly used in the context of
    /// transaction inputs, linking back to the source of the UTXO.
    pub fn to_outpoint(&self) -> OutPoint {
        OutPoint {
            txid: self.txid,
            vout: self.vout,
        }
    }
}
/// Detailed metadata about a Bitcoin transaction from the indexer
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TransactionMetadata {
    /// The transaction ID
    pub txid: String,

    /// The transaction version
    pub version: i32,

    /// The transaction locktime
    pub locktime: u32,

    /// The transaction fee in satoshis
    pub fee: u64,

    /// The transaction weight in weight units (WU) as defined by BIP-141
    pub weight: u64,

    /// Transaction inputs
    pub vin: Vec<TxInput>,

    /// Transaction outputs
    pub vout: Vec<TxOutput>,

    /// The transaction status including confirmation details
    pub status: TxStatus,
}
/// A transaction input that references a previous output
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TxInput {
    /// The transaction ID containing the output being spent
    pub txid: String,

    /// The output index (vout) being spent
    pub vout: usize,

    /// The details of the previous output being spent
    pub prevout: TxOutput,

    /// The sequence number of the input
    pub sequence: u32,

    /// The witness data for the input
    pub witness: Vec<String>,
}

/// A transaction output that specifies a value to be spent
#[derive(Debug, Clone, Deserialize, Default)]
pub struct TxOutput {
    /// The value of this output in satoshis
    pub value: u64,

    /// The script pubkey of the output
    #[serde(rename = "scriptpubkey")]
    pub script_pubkey: String,

    /// The address derived by the indexer for this script pubkey when available.
    #[serde(rename = "scriptpubkey_address", default)]
    pub script_pubkey_address: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TxStatus {
    /// Whether the transaction is confirmed
    pub confirmed: bool,

    /// Block height (available for confirmed transactions, null otherwise)
    pub block_height: Option<u64>,

    /// Block hash (available for confirmed transactions, null otherwise)
    pub block_hash: Option<String>,

    /// Block time (available for confirmed transactions, null otherwise)
    pub block_time: Option<u64>,
}

/// Represents the spending status of a transaction output.
///
/// This struct contains information about whether a specific transaction output
/// has been spent and, if so, which transaction spent it. This is commonly used
/// when querying blockchain indexers to determine the current state of transaction
/// outputs and to find descendant transactions.
///
/// # Fields
/// * `spent` - Whether the output has been spent (true) or is still unspent (false)
/// * `txid` - The transaction ID that spent this output, if it has been spent
#[derive(Deserialize, Debug, Clone)]
pub struct OutSpend {
    /// Whether the output has been spent (true) or is still unspent (false)
    pub spent: bool,

    /// The transaction ID that spent this output, if it has been spent.
    /// This field is `None` when `spent` is `false`.
    pub txid: Option<String>,
}

/// A thread-safe, reference-counted pointer to an `Indexer` trait object.
pub type ArcIndexer = Arc<dyn Indexer + Send + Sync>;
