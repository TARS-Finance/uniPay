use crate::core::RPCClient;
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// JSON-RPC request payload
#[derive(Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'a str,
    id: &'a str,
    method: &'a str,
    params: Value,
}

/// JSON-RPC response wrapper
#[derive(Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

/// JSON-RPC error details
#[derive(Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

pub struct BitcoinRPCClient {
    url: String,
    client: Client,
    auth: Option<(String, String)>,
}

impl BitcoinRPCClient {
    pub fn new(url: String, username: Option<String>, password: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .expect("Failed to build reqwest client");

        let auth = match (username, password) {
            (Some(user), Some(pass)) => Some((user, pass)),
            _ => None,
        };

        Self { url, client, auth }
    }

    async fn call<R: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: Value,
    ) -> eyre::Result<R> {
        let request = JsonRpcRequest {
            jsonrpc: "1.0",
            id: "1",
            method,
            params,
        };

        let mut req = self
            .client
            .post(&self.url)
            .header("content-type", "text/plain;")
            .json(&request);

        if let Some((user, pass)) = &self.auth {
            req = req.basic_auth(user, Some(pass));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| eyre::eyre!("RPC request failed: {e}"))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(eyre::eyre!("RPC HTTP error {status}: {body}"));
        }

        let rpc_response: JsonRpcResponse<R> = resp
            .json()
            .await
            .map_err(|e| eyre::eyre!("Failed to parse RPC response: {e}"))?;

        if let Some(err) = rpc_response.error {
            return Err(eyre::eyre!(
                "RPC error (code {}): {}",
                err.code,
                err.message
            ));
        }

        rpc_response
            .result
            .ok_or_else(|| eyre::eyre!("Missing result in RPC response"))
    }
}

#[async_trait]
impl RPCClient for BitcoinRPCClient {
    async fn get_mempool_entry(&self, tx_id: &str) -> eyre::Result<Option<serde_json::Value>> {
        const TX_NOT_IN_MEMPOOL_ERROR: &str = "Transaction not in mempool";

        match self
            .call("getmempoolentry", serde_json::json!([tx_id]))
            .await
        {
            Ok(entry) => Ok(entry),
            Err(e) => {
                if e.to_string().contains(TX_NOT_IN_MEMPOOL_ERROR) {
                    return Ok(None);
                }
                return Err(e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const BTC_REGNET_USERNAME: &str = "admin1";
    const BTC_REGNET_PASSWORD: &str = "123";
    const BTC_REGNET_URL: &str = "http://localhost:18443";
    #[tokio::test]
    async fn test_get_mempool_entry() {
        let client = BitcoinRPCClient::new(
            BTC_REGNET_URL.to_string(),
            Some(BTC_REGNET_USERNAME.to_string()),
            Some(BTC_REGNET_PASSWORD.to_string()),
        );
        let tx_id = "8c753bbd795196b5e4a8505944cb2e7843f4342bedc7279823161f0f3625bca6";
        let result = client.get_mempool_entry(tx_id).await;
        dbg!(&result);
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
