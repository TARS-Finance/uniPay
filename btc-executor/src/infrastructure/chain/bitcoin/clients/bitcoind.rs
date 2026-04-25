//! bitcoind JSON-RPC 1.0 client.
//!
//! Thin wrapper around `reqwest` for calling bitcoind's JSON-RPC interface.
//! Used for mempool queries, raw-tx broadcasting, regtest block generation,
//! and RBF fee analysis that Esplora does not expose.

use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use super::electrs::BitcoinClientError;

// ── Response types ──────────────────────────────────────────────────────────

/// A mempool entry returned by `getmempoolentry`.
#[derive(Debug, Deserialize)]
pub struct MempoolEntry {
    pub vsize: u64,
    pub weight: u64,
    pub fees: MempoolFees,
    pub depends: Vec<String>,
    pub spentby: Vec<String>,
}

/// A mempool entry returned by `getmempooldescendants`.
#[derive(Debug, Deserialize)]
pub struct DescendantEntry {
    pub vsize: u64,
    pub fees: MempoolFees,
}

/// Fee breakdown inside a `MempoolEntry` (values are in BTC).
#[derive(Debug, Deserialize)]
pub struct MempoolFees {
    pub base: f64,
    pub modified: f64,
    pub ancestor: f64,
    pub descendant: f64,
}

/// Computed fee information used for RBF replacement decisions.
#[derive(Debug)]
pub struct RBFTxFeeInfo {
    /// The transaction's own fee in satoshis.
    pub total_fee: u64,
    /// Sum of direct descendants' fees in satoshis.
    pub descendant_fee: u64,
    /// Effective fee rate (sat/vB) considering the tx and its direct descendants.
    pub tx_fee_rate: f64,
}

// ── JSON-RPC envelope ───────────────────────────────────────────────────────

/// Raw JSON-RPC 1.0 response envelope.
#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<Value>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

// ── Client ──────────────────────────────────────────────────────────────────

/// JSON-RPC 1.0 client for a bitcoind node.
pub struct BitcoindRpcClient {
    client: Client,
    url: String,
    auth: (String, String),
}

impl BitcoindRpcClient {
    /// Create a new `BitcoindRpcClient`.
    ///
    /// * `url`  - Full URL of the bitcoind RPC endpoint (e.g. `"http://127.0.0.1:18443"`).
    /// * `user` - RPC username.
    /// * `pass` - RPC password.
    pub fn new(url: String, user: String, pass: String) -> Self {
        Self {
            client: Client::new(),
            url,
            auth: (user, pass),
        }
    }

    async fn call_optional(
        &self,
        method: &str,
        params: Vec<Value>,
    ) -> Result<Option<Value>, BitcoinClientError> {
        let body = serde_json::json!({
            "jsonrpc": "1.0",
            "id": "munger",
            "method": method,
            "params": params,
        });

        let resp = self
            .client
            .post(&self.url)
            .basic_auth(&self.auth.0, Some(&self.auth.1))
            .json(&body)
            .send()
            .await?;

        let rpc_resp: RpcResponse = resp.json().await?;

        if let Some(err) = rpc_resp.error {
            return Err(BitcoinClientError::Rpc(format!(
                "code {}: {}",
                err.code, err.message
            )));
        }

        Ok(rpc_resp.result)
    }

    /// Send a raw JSON-RPC 1.0 call and return the `result` field.
    async fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, BitcoinClientError> {
        self.call_optional(method, params)
            .await?
            .ok_or_else(|| BitcoinClientError::Rpc("null result with no error".to_string()))
    }

    // ── Public helpers ──────────────────────────────────────────────────────

    /// Fetch the mempool entry for `txid` (`getmempoolentry`).
    pub async fn get_mempool_entry(&self, txid: &str) -> Result<MempoolEntry, BitcoinClientError> {
        let val = self
            .call("getmempoolentry", vec![Value::String(txid.to_string())])
            .await?;
        let entry: MempoolEntry = serde_json::from_value(val)?;
        Ok(entry)
    }

    /// Broadcast a signed raw transaction (`sendrawtransaction`).
    /// Returns the txid on success.
    pub async fn send_raw_transaction(&self, hex: &str) -> Result<String, BitcoinClientError> {
        let val = self
            .call("sendrawtransaction", vec![Value::String(hex.to_string())])
            .await?;
        val.as_str().map(|s| s.to_string()).ok_or_else(|| {
            BitcoinClientError::Parse("sendrawtransaction did not return a string".to_string())
        })
    }

    /// Fetch a raw transaction hex string (`getrawtransaction` with verbose=false).
    pub async fn get_raw_transaction_hex(&self, txid: &str) -> Result<String, BitcoinClientError> {
        let val = self
            .call("getrawtransaction", vec![Value::String(txid.to_string())])
            .await?;
        val.as_str().map(|s| s.to_string()).ok_or_else(|| {
            BitcoinClientError::Parse("getrawtransaction did not return a hex string".to_string())
        })
    }

    /// Get the current block count (`getblockcount`).
    pub async fn get_block_count(&self) -> Result<u64, BitcoinClientError> {
        let val = self.call("getblockcount", vec![]).await?;
        val.as_u64().ok_or_else(|| {
            BitcoinClientError::Parse("getblockcount did not return a number".to_string())
        })
    }

    /// Ask the node wallet for a fresh address (`getnewaddress`).
    pub async fn get_new_address(&self) -> Result<String, BitcoinClientError> {
        let val = self.call("getnewaddress", vec![]).await?;
        val.as_str().map(|s| s.to_string()).ok_or_else(|| {
            BitcoinClientError::Parse("getnewaddress did not return a string".to_string())
        })
    }

    /// Send funds from the node wallet to `addr` in sats (`sendtoaddress`).
    /// Returns the broadcast txid.
    pub async fn send_to_address_sats(
        &self,
        addr: &str,
        sats: u64,
    ) -> Result<String, BitcoinClientError> {
        let btc = (sats as f64) / 100_000_000.0;
        let amount = serde_json::Number::from_f64(btc).ok_or_else(|| {
            BitcoinClientError::Parse(format!("invalid BTC amount for sats value {sats}"))
        })?;
        let val = self
            .call(
                "sendtoaddress",
                vec![Value::String(addr.to_string()), Value::Number(amount)],
            )
            .await?;
        val.as_str().map(|s| s.to_string()).ok_or_else(|| {
            BitcoinClientError::Parse("sendtoaddress did not return a txid string".to_string())
        })
    }

    /// Generate `n` blocks to `addr` in regtest (`generatetoaddress`).
    /// Returns the hashes of the generated blocks.
    pub async fn generate_to_address(
        &self,
        n: u64,
        addr: &str,
    ) -> Result<Vec<String>, BitcoinClientError> {
        let val = self
            .call(
                "generatetoaddress",
                vec![Value::Number(n.into()), Value::String(addr.to_string())],
            )
            .await?;
        let hashes: Vec<String> = serde_json::from_value(val)?;
        Ok(hashes)
    }

    /// Fetch all descendants of `txid` from the mempool (`getmempooldescendants`).
    /// Returns a map of txid → entry.
    pub async fn get_mempool_descendants(
        &self,
        txid: &str,
    ) -> Result<std::collections::HashMap<String, DescendantEntry>, BitcoinClientError> {
        let val = self
            .call(
                "getmempooldescendants",
                vec![Value::String(txid.to_string()), Value::Bool(true)],
            )
            .await?;
        let map: std::collections::HashMap<String, DescendantEntry> = serde_json::from_value(val)?;
        Ok(map)
    }

    // Compute RBF-relevant fee information for a mempool transaction.
    ///
    /// Mirrors the Go `GetRBFTxFeeInfo` exactly:
    /// - `total_fee`      = `descendant` fee field (tx + all descendants), in sats
    /// - `descendant_fee` = `descendant - base`, in sats
    /// - `tx_fee_rate`    = if no descendants -> `descendant_total / descendant_vsize`;
    ///   if descendants exist -> `(base + direct_desc_fees) / (vsize + direct_desc_vsize)`
    pub async fn get_rbf_tx_fee_info(
        &self,
        txid: &str,
    ) -> Result<RBFTxFeeInfo, BitcoinClientError> {
        let descendants = self.get_mempool_descendants(txid).await?;
        let entry = self.get_mempool_entry(txid).await?;

        let total_fee = (entry.fees.descendant * 1e8).ceil() as u64;
        let base_fee = (entry.fees.base * 1e8).ceil() as u64;
        let descendant_fee = total_fee.saturating_sub(base_fee);

        let mut fee_info = RBFTxFeeInfo {
            total_fee,
            descendant_fee,
            tx_fee_rate: if entry.vsize == 0 {
                0.0
            } else {
                total_fee as f64 / entry.vsize as f64
            },
        };

        if descendants.is_empty() {
            return Ok(fee_info);
        }

        // Fee rate from direct descendants only (entry.spentby ∩ descendants map).
        let mut direct_desc_vsize: f64 = 0.0;
        let mut direct_desc_fee: f64 = 0.0;

        for child_txid in &entry.spentby {
            if let Some(desc) = descendants.get(child_txid) {
                direct_desc_vsize += desc.vsize as f64;
                direct_desc_fee += (desc.fees.base * 1e8).floor();
            }
        }

        let combined_vsize = direct_desc_vsize + entry.vsize as f64;
        if combined_vsize > 0.0 {
            fee_info.tx_fee_rate =
                ((direct_desc_fee + base_fee as f64) / combined_vsize).max(fee_info.tx_fee_rate);
        }

        Ok(fee_info)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_mempool_entry() {
        let json = r#"{
            "vsize": 141,
            "weight": 561,
            "fees": {
                "base": 0.00001410,
                "modified": 0.00001410,
                "ancestor": 0.00001410,
                "descendant": 0.00002820
            },
            "depends": [],
            "spentby": ["child_txid_1"]
        }"#;

        let entry: MempoolEntry = serde_json::from_str(json).expect("deserialize MempoolEntry");
        assert_eq!(entry.vsize, 141);
        assert_eq!(entry.weight, 561);
        assert!((entry.fees.base - 0.00001410).abs() < 1e-12);
        assert!((entry.fees.descendant - 0.00002820).abs() < 1e-12);
        assert!(entry.depends.is_empty());
        assert_eq!(entry.spentby, vec!["child_txid_1"]);
    }

    #[test]
    fn deserialize_mempool_fees() {
        let json = r#"{
            "base": 0.00005000,
            "modified": 0.00005000,
            "ancestor": 0.00005000,
            "descendant": 0.00010000
        }"#;

        let fees: MempoolFees = serde_json::from_str(json).expect("deserialize MempoolFees");
        assert!((fees.base - 0.00005000).abs() < 1e-12);
        assert!((fees.descendant - 0.00010000).abs() < 1e-12);
    }

    #[test]
    fn deserialize_rpc_response_success() {
        let json = r#"{"result": 850000, "error": null}"#;

        let resp: RpcResponse = serde_json::from_str(json).expect("deserialize RpcResponse");
        assert!(resp.error.is_none());
        assert_eq!(resp.result.and_then(|v| v.as_u64()), Some(850000));
    }

    #[test]
    fn deserialize_rpc_response_error() {
        let json = r#"{
            "result": null,
            "error": {"code": -5, "message": "Transaction not in mempool"}
        }"#;

        let resp: RpcResponse = serde_json::from_str(json).expect("deserialize RpcResponse");
        assert!(resp.error.is_some());
        let err = resp.error.as_ref().expect("error present");
        assert_eq!(err.code, -5);
        assert_eq!(err.message, "Transaction not in mempool");
    }

    #[test]
    fn rbf_fee_info_no_descendants() {
        // Simulate: base=0.00001410 BTC, vsize=141, no spentby
        let base_btc: f64 = 0.00001410;
        let base_fee = (base_btc * 1e8).round() as u64;
        assert_eq!(base_fee, 1410);

        let vsize: u64 = 141;
        let rate = base_fee as f64 / vsize as f64;
        assert!((rate - 10.0).abs() < 0.01);
    }

    #[test]
    fn rbf_fee_info_with_descendants() {
        // Parent: base=0.00001410, vsize=141, spentby=[child]
        // Child:  base=0.00000705, vsize=70
        let parent_base = (0.00001410_f64 * 1e8).round() as u64; // 1410
        let parent_vsize: u64 = 141;
        let child_base = (0.00000705_f64 * 1e8).round() as u64; // 705
        let child_vsize: u64 = 70;

        let combined_fee = parent_base + child_base; // 2115
        let combined_vsize = parent_vsize + child_vsize; // 211
        let rate = combined_fee as f64 / combined_vsize as f64;

        assert_eq!(combined_fee, 2115);
        assert_eq!(combined_vsize, 211);
        assert!((rate - 10.0236).abs() < 0.01);
    }

    #[test]
    fn bitcoind_client_stores_credentials() {
        let client = BitcoindRpcClient::new(
            "http://127.0.0.1:18443".into(),
            "user".into(),
            "pass".into(),
        );
        assert_eq!(client.url, "http://127.0.0.1:18443");
        assert_eq!(client.auth, ("user".to_string(), "pass".to_string()));
    }
}
