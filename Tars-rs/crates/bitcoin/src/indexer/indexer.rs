use super::{primitives::Utxo, traits::Indexer};
use crate::{
    indexer::primitives::{OutSpend, TransactionMetadata},
    UtxoStatus,
};
use alloy::hex;
use async_trait::async_trait;
use bitcoin::{Address, Transaction};
use eyre::{eyre, Result};
use serde::Deserialize;
use std::time::Duration;
use tracing::debug;

/// Default timeout in seconds for indexer client requests
const INDEXER_CLIENT_TIMEOUT_SECS: u64 = 5;

/// Endpoint for retrieving unspent transaction outputs (UTXOs) for an address
const UTXO_ENDPOINT: &str = "/address/{}/utxo";

/// Endpoint for retrieving the current block height
const BLOCK_HEIGHT_ENDPOINT: &str = "/blocks/tip/height";

/// Endpoint for retrieving a transaction's raw hex data
const TX_HEX_ENDPOINT: &str = "/tx/{}/hex";

/// Endpoint for retrieving detailed transaction metadata
const TX_ENDPOINT: &str = "/tx/{}";

/// Endpoint for retrieving spending information for a transaction's outputs
const TX_OUTSPENDS_ENDPOINT: &str = "/tx/{}/outspends";

/// Endpoint for submitting a new transaction to the network
const SUBMIT_TX_ENDPOINT: &str = "/tx";

/// A helper struct used for deserializing UTXO data from JSON responses.
///
/// Unlike [`Utxo`], this struct represents the transaction ID (`txid`) as a
/// `String` instead of a [`Txid`] type, since [`Txid`] does not implement
/// `Deserialize`. This makes it suitable for use with `serde` when parsing
/// JSON data, after which it can be converted into a [`Utxo`].
#[derive(Debug, Deserialize)]
pub struct UtxoJson {
    /// The transaction ID that contains this output (as a string).
    pub txid: String,

    /// The output index (vout) within the transaction.
    pub vout: u32,

    /// The value of this UTXO in satoshis (1 BTC = 100,000,000 satoshis).
    pub value: u64,

    /// The status of the UTXO.
    pub status: UtxoStatus,
}

/// A client for interacting with Bitcoin blockchain indexer services.
#[derive(Debug, Clone)]
pub struct BitcoinIndexerClient {
    /// Base URL of the indexer service
    url: String,

    /// HTTP client used for making requests to the indexer
    client: reqwest::Client,
}

impl BitcoinIndexerClient {
    /// Creates a new Bitcoin indexer client.
    ///
    /// # Arguments
    /// * `url` - The base URL of the indexer service
    /// * `timeout_secs` - Optional timeout in seconds (defaults to 5 seconds)
    ///
    /// # Returns
    /// A new `BitcoinIndexerClient` instance or an error if the HTTP client
    /// could not be created
    pub fn new(url: String, timeout_secs: Option<u64>) -> Result<Self> {
        let timeout = Duration::from_secs(timeout_secs.unwrap_or(INDEXER_CLIENT_TIMEOUT_SECS));

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(|e| eyre!("Failed to build client: {e}"))?;

        Ok(Self { url, client })
    }

    /// Helper method to handle HTTP response errors
    async fn handle_response_error(
        &self,
        endpoint: &str,
        response: reqwest::Response,
    ) -> Result<reqwest::Response> {
        if response.status().is_success() {
            return Ok(response);
        }

        let status = response.status();
        let err_msg = response
            .text()
            .await
            .unwrap_or_else(|_| "Failed to read error message".to_string());

        Err(eyre!(
            "Request to {endpoint} failed with status {status}: {err_msg}"
        ))
    }
}

#[async_trait]
impl Indexer for BitcoinIndexerClient {
    /// Retrieves a transaction by its transaction ID and returns it as a Transaction object.
    async fn get_tx_hex(&self, txid: &str) -> Result<Transaction> {
        let endpoint = format!("{}{}", self.url, TX_HEX_ENDPOINT.replace("{}", txid));
        debug!(target: "indexer", "Fetching transaction {txid}");

        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| eyre!("Failed to send GET request to fetch tx {txid}: {e}"))?;

        let resp = self
            .handle_response_error(TX_HEX_ENDPOINT.replace("{}", txid).as_str(), resp)
            .await?;

        let hex = resp
            .text()
            .await
            .map_err(|e| eyre!("Failed to read transaction hex from response: {e}"))?;

        let tx_bytes =
            hex::decode(&hex).map_err(|e| eyre!("Failed to decode transaction hex: {e}"))?;

        bitcoin::consensus::deserialize(&tx_bytes)
            .map_err(|e| eyre!("Failed to deserialize transaction: {e}"))
    }

    /// Retrieves detailed transaction information from the indexer.
    ///
    /// # Arguments
    /// * `txid` - The transaction ID to fetch details for
    ///
    /// # Returns
    /// A `TransactionMetadata` containing the transaction details, or an error if the request failed
    async fn get_tx(&self, txid: &str) -> Result<TransactionMetadata> {
        let endpoint = format!("{}{}", self.url, TX_ENDPOINT.replace("{}", txid));
        debug!(target: "indexer", "Fetching transaction details for {txid}");

        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| eyre!("Failed to send GET request to fetch tx details {txid}: {e}"))?;

        let resp = self
            .handle_response_error(TX_ENDPOINT.replace("{}", txid).as_str(), resp)
            .await?;

        resp.json::<TransactionMetadata>()
            .await
            .map_err(|e| eyre!("Failed to parse transaction response: {e}"))
    }

    /// Submits a transaction to the Bitcoin network.
    async fn submit_tx(&self, tx: &Transaction) -> Result<()> {
        let endpoint = format!("{}{}", self.url, SUBMIT_TX_ENDPOINT);
        debug!(target: "indexer", "Submitting transaction");

        let tx_hex = hex::encode(bitcoin::consensus::serialize(tx));

        let resp = self
            .client
            .post(&endpoint)
            .header("Content-Type", "application/text")
            .body(tx_hex)
            .send()
            .await
            .map_err(|e| eyre!("Failed to submit transaction: {e}"))?;

        self.handle_response_error(SUBMIT_TX_ENDPOINT, resp)
            .await
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Retrieves the current block height of the Bitcoin blockchain.
    async fn get_block_height(&self) -> Result<u64> {
        let url = format!("{}{}", self.url, BLOCK_HEIGHT_ENDPOINT);
        debug!(target: "indexer", "Fetching block height");

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| eyre!("Failed to send request to fetch block height: {e}"))?;

        let resp = self
            .handle_response_error(BLOCK_HEIGHT_ENDPOINT, resp)
            .await?;

        let height_str = resp
            .text()
            .await
            .map_err(|e| eyre!("Failed to read block height response text: {e}"))?
            .trim()
            .to_string();

        height_str
            .parse::<u64>()
            .map_err(|e| eyre!("Invalid block height format: {e}"))
    }

    /// Retrieves the unspent transaction outputs (UTXOs) for a given address.
    async fn get_utxos(&self, address: &Address) -> Result<Vec<Utxo>> {
        let endpoint = format!(
            "{}{}",
            self.url,
            UTXO_ENDPOINT.replace("{}", &address.to_string())
        );
        debug!(target: "indexer", "Fetching UTXOs for address {address}");

        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| eyre!("Failed to fetch UTXOs for address {address}: {e}"))?;

        let resp = self
            .handle_response_error(
                UTXO_ENDPOINT.replace("{}", &address.to_string()).as_str(),
                resp,
            )
            .await?;

        let utxos_json: Vec<UtxoJson> = resp
            .json()
            .await
            .map_err(|e| eyre!("Failed to parse response : {e}"))?;

        utxos_json
            .iter()
            .map(|utxo_json| Utxo::try_from(utxo_json))
            .collect()
    }

    /// Retrieves the spending status of all outputs for a given transaction.
    async fn get_tx_outspends(&self, txid: &str) -> Result<Vec<OutSpend>> {
        let endpoint = format!("{}{}", self.url, TX_OUTSPENDS_ENDPOINT.replace("{}", txid));
        debug!(target: "indexer", "Fetching outspends for transaction {txid}");

        // Send request to get outspends for the given transaction
        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .map_err(|e| eyre!("Failed to send GET request for outspends: {e}"))?;

        let outspends: Vec<OutSpend> = resp
            .json()
            .await
            .map_err(|e| eyre!("Failed to parse outspends response: {e}"))?;

        Ok(outspends)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::get_test_bitcoin_indexer;

    use super::*;
    use alloy::hex;
    use bitcoin::{Address, Transaction};
    use std::str::FromStr;

    #[tokio::test]
    async fn test_post_tx() {
        let client = get_test_bitcoin_indexer().unwrap();

        // Test transaction hex (large hex string from original test)
        const TX_HEX: &str = "02000000000101b0c69f30508c8c9c8f3c30bb7290e7d8088cd06265c1951b6c701b8a5a1e245a0000000000ffffffff01f0ca052a0100000017a914f0c1ed22d8aef66040b9970e26d0970b72d92a72870247304402201758dfd2c8e1d5cd9c899bc07d85b34b8b1ff6f7098dc2ccdf5aa32e2516ec3f0220672bcedb857e17e3d1eb5a005f5c0d9ea4516a4ffb5d7fd7e47a32a67acbdfb5012102c6e3e9b1c0d3947aeac624042d7ccbe3816d0fa9d90f6f9725eb50e87b3b0be300000000";

        let tx_bytes = hex::decode(TX_HEX).expect("Should be valid hex");
        let tx: Transaction =
            bitcoin::consensus::deserialize(&tx_bytes).expect("Should be valid transaction");

        // Submit tx (this test will be skipped if the indexer is not running)
        let res = client.submit_tx(&tx).await;
        println!("Transaction submission result: {:?}", res);
    }

    #[tokio::test]
    async fn test_get_utxos() {
        let client = get_test_bitcoin_indexer().unwrap();

        // Create a test address
        let address = Address::from_str("bcrt1q556lc447reahwdq24ur5q4xsqs099fja78lq6p")
            .expect("Should be valid address")
            .assume_checked();

        let result = client.get_utxos(&address).await;
        assert!(result.is_ok(), "Should successfully get UTXOs");
    }

    #[tokio::test]
    async fn test_get_block_height() {
        let client = get_test_bitcoin_indexer().unwrap();

        // Get the current block height
        let height = client
            .get_block_height()
            .await
            .expect("Should get block height");

        // Assert that the result contains a non-zero height
        assert!(height > 0, "Block height should be greater than 0");
    }

    #[tokio::test]
    async fn test_get_tx_outspends() {
        let indexer_url = "https://mempool.space/testnet4/api".to_string();
        let client = BitcoinIndexerClient::new(indexer_url, None).expect("Should create client");

        // Test transaction ID that should have at least one outspent
        let txid = "b6ab6b9eb55e4e43d68e40f9acbd36079fd30688e292235d3d9801ef227f9e5c";

        // Get the outspends for this transaction
        let outspends = client
            .get_tx_outspends(txid)
            .await
            .expect("Should successfully get transaction outspends");

        // Verify that there is at least one outspent
        assert!(
            !outspends.is_empty(),
            "Transaction {} should have at least one outspent",
            txid
        );
    }

    #[tokio::test]
    async fn test_get_tx_hex() {
        let indexer_url = "https://mempool.space/testnet4/api".to_string();
        let client = BitcoinIndexerClient::new(indexer_url, None).expect("Should create client");

        // Test transaction ID that should exist on testnet
        let txid = "b6ab6b9eb55e4e43d68e40f9acbd36079fd30688e292235d3d9801ef227f9e5c";

        // Get the transaction hex
        let tx = client
            .get_tx_hex(txid)
            .await
            .expect("Should successfully get transaction hex");

        // Verify the transaction metadata has the expected structure
        assert_eq!(
            tx.compute_txid().to_string(),
            txid,
            "Transaction ID should match"
        );
        assert!(!tx.input.is_empty(), "Transaction should have inputs");
        assert!(!tx.output.is_empty(), "Transaction should have outputs");
    }

    #[tokio::test]
    async fn test_get_tx() {
        let indexer_url = "https://mempool.space/testnet4/api".to_string();
        let client = BitcoinIndexerClient::new(indexer_url, None).expect("Should create client");

        // Test transaction ID that should exist on testnet
        let txid = "b6ab6b9eb55e4e43d68e40f9acbd36079fd30688e292235d3d9801ef227f9e5c";

        // Get the transaction metadata
        let tx_metadata = client
            .get_tx(txid)
            .await
            .expect("Should successfully get transaction metadata");

        assert!(!tx_metadata.txid.is_empty(), "Transaction should have Txid");
    }
}
