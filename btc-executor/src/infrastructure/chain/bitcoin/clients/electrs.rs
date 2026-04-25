//! Electrs (Esplora) REST client for querying the Bitcoin blockchain.
//!
//! Wraps the Esplora/Electrs HTTP API to provide typed access to UTXOs,
//! transactions, fee estimates, and block height. Used by the Bitcoin
//! `ChainPort` for read-side chain queries and transaction broadcasting.
//!
//! In the wallet/batcher design this client is read-heavy:
//!
//! - the runner uses tx/status endpoints to decide whether a lineage head is
//!   still live, confirmed, or missing;
//! - query/watcher code uses address history endpoints to reconstruct HTLC
//!   funding and spend observations;
//! - the builder uses address UTXOs as the base set for wallet fee funding;
//! - broadcasting is still exposed here for the Esplora-compatible path, even
//!   though the main runner currently broadcasts through bitcoind.

use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;

// ── Error type shared across Bitcoin clients ────────────────────────────────

/// Errors that can occur when communicating with Bitcoin infrastructure
/// (Electrs REST API or bitcoind JSON-RPC).
#[derive(Debug, thiserror::Error)]
pub enum BitcoinClientError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JSON-RPC error: {0}")]
    Rpc(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("JSON deserialization error: {0}")]
    Json(#[from] serde_json::Error),
}

impl BitcoinClientError {
    /// True if the error indicates "tx not found" (404, not in mempool) rather than transient RPC/network failure.
    pub fn is_tx_not_found(&self) -> bool {
        match self {
            Self::Http(e) => e.status().is_some_and(|s| s.as_u16() == 404),
            Self::Rpc(msg) => {
                msg.contains("not in mempool")
                    || msg.contains("-5")
                    || msg.contains("Transaction not found")
                    || msg.contains("not found")
                    || msg.contains("not in memory pool")
            },
            _ => false,
        }
    }
}

// ── Esplora response types ──────────────────────────────────────────────────

/// A UTXO returned by the Esplora `/address/:addr/utxo` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct Utxo {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub status: TxStatus,
}

/// Confirmation status attached to transactions and UTXOs.
#[derive(Debug, Clone, Deserialize)]
pub struct TxStatus {
    pub confirmed: bool,
    pub block_height: Option<u64>,
    pub block_hash: Option<String>,
    pub block_time: Option<u64>,
}

/// Full transaction as returned by the Esplora `/tx/:txid` endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct EsploraTx {
    pub txid: String,
    pub version: u32,
    pub locktime: u32,
    pub vin: Vec<TxVin>,
    pub vout: Vec<TxVout>,
    pub size: u64,
    pub weight: u64,
    pub fee: u64,
    pub status: TxStatus,
}

/// A transaction input.
#[derive(Debug, Clone, Deserialize)]
pub struct TxVin {
    pub txid: String,
    pub vout: u32,
    pub prevout: Option<TxVout>,
    pub scriptsig: String,
    pub witness: Option<Vec<String>>,
    pub sequence: u32,
}

/// A transaction output.
#[derive(Debug, Clone, Deserialize)]
pub struct TxVout {
    pub scriptpubkey: String,
    pub scriptpubkey_address: Option<String>,
    pub value: u64,
}

// ── Client ──────────────────────────────────────────────────────────────────

/// HTTP client for the Esplora / Electrs REST API.
pub struct ElectrsClient {
    client: Client,
    base_url: String,
}

impl ElectrsClient {
    /// Create a new `ElectrsClient` targeting the given Esplora base URL
    /// (e.g. `"https://blockstream.info/api"`).
    pub fn new(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Fetch all UTXOs for `address`.
    pub async fn get_address_utxos(&self, address: &str) -> Result<Vec<Utxo>, BitcoinClientError> {
        let url = format!("{}/address/{}/utxo", self.base_url, address);
        // The builder uses this as the externally visible wallet UTXO set.
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let utxos: Vec<Utxo> = resp.json().await?;
        Ok(utxos)
    }

    /// Fetch confirmed and unconfirmed transactions for `address`.
    pub async fn get_address_txs(
        &self,
        address: &str,
    ) -> Result<Vec<EsploraTx>, BitcoinClientError> {
        let url = format!("{}/address/{}/txs", self.base_url, address);
        // Address history lets watcher/query code infer both HTLC funding and
        // subsequent redeem/refund spends without requiring an indexer of our own.
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let txs: Vec<EsploraTx> = resp.json().await?;
        Ok(txs)
    }

    /// Fetch confirmed transaction history for `address`, newest first, with optional pagination.
    pub async fn get_confirmed_address_txs_chain(
        &self,
        address: &str,
        last_seen_txid: Option<&str>,
    ) -> Result<Vec<EsploraTx>, BitcoinClientError> {
        let url = match last_seen_txid {
            Some(last_seen_txid) => format!(
                "{}/address/{}/txs/chain/{}",
                self.base_url, address, last_seen_txid
            ),
            None => format!("{}/address/{}/txs/chain", self.base_url, address),
        };
        // Confirmed-chain pagination is used when callers need deterministic
        // history traversal beyond the first page of newest-first results.
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let txs: Vec<EsploraTx> = resp.json().await?;
        Ok(txs)
    }

    /// Get the current best-block height.
    pub async fn get_block_height(&self) -> Result<u64, BitcoinClientError> {
        let url = format!("{}/blocks/tip/height", self.base_url);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let text = resp.text().await?;
        text.trim()
            .parse::<u64>()
            .map_err(|e| BitcoinClientError::Parse(e.to_string()))
    }

    /// Fetch a full transaction by its txid.
    pub async fn get_tx(&self, txid: &str) -> Result<EsploraTx, BitcoinClientError> {
        let url = format!("{}/tx/{}", self.base_url, txid);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let tx: EsploraTx = resp.json().await?;
        Ok(tx)
    }

    /// Fetch the raw hex-encoded transaction by its txid.
    pub async fn get_tx_hex(&self, txid: &str) -> Result<String, BitcoinClientError> {
        let url = format!("{}/tx/{}/hex", self.base_url, txid);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let hex = resp.text().await?;
        Ok(hex.trim().to_string())
    }

    /// Fetch only the confirmation status for a transaction.
    pub async fn get_tx_status(&self, txid: &str) -> Result<TxStatus, BitcoinClientError> {
        let url = format!("{}/tx/{}/status", self.base_url, txid);
        // Runner observation prefers the lightest endpoint that answers the
        // "confirmed, mempool, or missing?" question.
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let status: TxStatus = resp.json().await?;
        Ok(status)
    }

    /// Broadcast a signed transaction (raw hex) and return the resulting txid.
    pub async fn broadcast_tx(&self, raw_hex: &str) -> Result<String, BitcoinClientError> {
        let url = format!("{}/tx", self.base_url);
        // This is kept for Esplora-compatible submission paths even though the
        // batch runner's primary submission path currently goes through bitcoind.
        let resp = self
            .client
            .post(&url)
            .header("Content-Type", "text/plain")
            .body(raw_hex.to_string())
            .send()
            .await?
            .error_for_status()?;
        let txid = resp.text().await?;
        Ok(txid.trim().to_string())
    }

    /// Fetch fee estimates (target confirmations -> sat/vB) from the Esplora
    /// `/fee-estimates` endpoint.
    pub async fn get_fee_estimates(&self) -> Result<HashMap<String, f64>, BitcoinClientError> {
        let url = format!("{}/fee-estimates", self.base_url);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let estimates: HashMap<String, f64> = resp.json().await?;
        Ok(estimates)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn deserialize_utxo() {
        let json = r#"{
            "txid": "abc123def456",
            "vout": 0,
            "value": 50000,
            "status": {
                "confirmed": true,
                "block_height": 800000,
                "block_hash": "000000000000000000023b3c1b2d8e7a",
                "block_time": 1700000000
            }
        }"#;

        let utxo: Utxo = serde_json::from_str(json).expect("deserialize Utxo");
        assert_eq!(utxo.txid, "abc123def456");
        assert_eq!(utxo.vout, 0);
        assert_eq!(utxo.value, 50000);
        assert!(utxo.status.confirmed);
        assert_eq!(utxo.status.block_height, Some(800000));
    }

    #[test]
    fn deserialize_tx_status_unconfirmed() {
        let json = r#"{
            "confirmed": false,
            "block_height": null,
            "block_hash": null,
            "block_time": null
        }"#;

        let status: TxStatus = serde_json::from_str(json).expect("deserialize TxStatus");
        assert!(!status.confirmed);
        assert!(status.block_height.is_none());
    }

    #[test]
    fn deserialize_esplora_tx() {
        let json = r#"{
            "txid": "deadbeef01234567",
            "version": 2,
            "locktime": 0,
            "vin": [
                {
                    "txid": "prevtxid0000",
                    "vout": 1,
                    "prevout": {
                        "scriptpubkey": "0014aabbccdd",
                        "scriptpubkey_address": "bc1qexample",
                        "value": 100000
                    },
                    "scriptsig": "",
                    "witness": ["304402...", "02aabb..."],
                    "sequence": 4294967293
                }
            ],
            "vout": [
                {
                    "scriptpubkey": "0014eeff0011",
                    "scriptpubkey_address": "bc1qrecipient",
                    "value": 90000
                },
                {
                    "scriptpubkey": "0014aabbccdd",
                    "scriptpubkey_address": "bc1qchange",
                    "value": 9500
                }
            ],
            "size": 222,
            "weight": 561,
            "fee": 500,
            "status": {
                "confirmed": true,
                "block_height": 800001,
                "block_hash": "0000000000000000000abc123",
                "block_time": 1700001000
            }
        }"#;

        let tx: EsploraTx = serde_json::from_str(json).expect("deserialize EsploraTx");
        assert_eq!(tx.txid, "deadbeef01234567");
        assert_eq!(tx.version, 2);
        assert_eq!(tx.vin.len(), 1);
        assert_eq!(tx.vout.len(), 2);
        assert_eq!(tx.fee, 500);
        assert!(tx.status.confirmed);

        let vin = &tx.vin[0];
        assert_eq!(vin.txid, "prevtxid0000");
        assert_eq!(vin.vout, 1);
        assert_eq!(vin.sequence, 4294967293);
        assert!(vin.prevout.is_some());
        assert_eq!(vin.witness.as_ref().map(|w| w.len()), Some(2));

        let vout = &tx.vout[0];
        assert_eq!(vout.value, 90000);
        assert_eq!(vout.scriptpubkey_address.as_deref(), Some("bc1qrecipient"));
    }

    #[test]
    fn deserialize_fee_estimates() {
        let json = r#"{"1": 25.5, "3": 12.0, "6": 8.5, "25": 3.2}"#;

        let estimates: HashMap<String, f64> =
            serde_json::from_str(json).expect("deserialize fee estimates");
        assert_eq!(estimates.len(), 4);
        assert!((estimates["1"] - 25.5).abs() < f64::EPSILON);
        assert!((estimates["25"] - 3.2).abs() < f64::EPSILON);
    }

    #[test]
    fn deserialize_txvout_without_address() {
        let json = r#"{
            "scriptpubkey": "6a0b68656c6c6f20776f726c64",
            "scriptpubkey_address": null,
            "value": 0
        }"#;

        let vout: TxVout = serde_json::from_str(json).expect("deserialize TxVout");
        assert!(vout.scriptpubkey_address.is_none());
        assert_eq!(vout.value, 0);
    }

    #[test]
    fn electrs_client_stores_base_url() {
        let client = ElectrsClient::new("https://mempool.space/api".to_string());
        assert_eq!(client.base_url, "https://mempool.space/api");
    }

    #[tokio::test]
    async fn get_confirmed_address_txs_chain_uses_last_seen_txid_pagination() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/address/bc1qexample/txs/chain/last-seen-tx"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(vec![serde_json::json!({
                    "txid": "chain-tx-1",
                    "version": 2,
                    "locktime": 0,
                    "vin": [],
                    "vout": [],
                    "size": 180,
                    "weight": 720,
                    "fee": 500,
                    "status": {
                        "confirmed": true,
                        "block_height": 900001,
                        "block_hash": "hash",
                        "block_time": 1700000100
                    }
                })]),
            )
            .mount(&server)
            .await;

        let client = ElectrsClient::new(server.uri());

        let txs = client
            .get_confirmed_address_txs_chain("bc1qexample", Some("last-seen-tx"))
            .await
            .expect("confirmed tx chain page");

        assert_eq!(txs.len(), 1);
        assert_eq!(txs[0].txid, "chain-tx-1");
        assert!(txs[0].status.confirmed);
    }
}
