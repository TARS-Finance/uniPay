use crate::fee_providers::{
    constants::DEFAULT_TIMEOUT_SECS, primitives::FeeEstimate, traits::FeeRateEstimator,
};
use async_trait::async_trait;
use eyre::{bail, eyre, Result};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::time::Duration;
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MempoolFeeEstimate {
    /// Fastest possible fee rate in sat/vB
    pub fastest_fee: u64,

    /// Fee rate for confirmation within ~30 minutes (half an hour) in sat/vB
    pub half_hour_fee: u64,

    /// Fee rate for confirmation within ~60 minutes (one hour) in sat/vB
    pub hour_fee: u64,

    /// Minimum acceptable fee rate in sat/vB
    pub minimum_fee: u64,

    /// Economical fee rate suitable for low-priority transactions in sat/vB
    pub economy_fee: u64,
}

/// Fee rate provider using mempool.space API
#[derive(Debug, Clone)]
pub struct MempoolFeeRateEstimator {
    /// Base URL for the mempool.space API
    url: Url,

    /// HTTP client with configured timeout
    client: Client,

    /// Request timeout duration
    timeout: Duration,
}

impl MempoolFeeRateEstimator {
    /// Creates a new MempoolFeeRateEstimator instance
    ///
    /// # Arguments
    /// * `url` - Base URL for mempool.space API
    /// * `timeout` - Optional request timeout in seconds
    ///
    /// Uses DEFAULT_TIMEOUT_SECS if timeout is not specified
    pub fn new(url: Url, timeout: Option<u64>) -> Self {
        let timeout = Duration::from_secs(timeout.unwrap_or(DEFAULT_TIMEOUT_SECS));
        let client = Client::new();

        Self {
            url,
            client,
            timeout,
        }
    }
}
#[async_trait]
impl FeeRateEstimator for MempoolFeeRateEstimator {
    /// Fetches current recommended fee rates
    ///
    /// Queries mempool.space API for current fee rate suggestions
    /// Returns fee rates in satoshis per virtual byte
    ///
    /// # Returns
    /// * `Result<FeeEstimate>` - Fee rate suggestions for different priorities
    async fn get_fee_estimates(&self) -> Result<FeeEstimate> {
        let request = self.client.get(self.url.clone()).timeout(self.timeout);

        let response = request
            .send()
            .await
            .map_err(|err| eyre!(format!("Failed to send request : {:#?}", err.to_string())))?;

        if !response.status().is_success() {
            let status = response.status();
            let error = response.text().await.map_err(|err| {
                eyre!(format!("Failed to parse Mempool API response: {:#?}", err))
            })?;

            bail!(format!(
                "Mempool API returned status: {} with error: {}",
                status, error
            ))
        }

        let fee_estimate: MempoolFeeEstimate = response
            .json()
            .await
            .map_err(|err| eyre!(format!("Failed to parse Mempool API response: {:#?}", err)))?;

        let fee_estimate = FeeEstimate {
            fastest_fee: fee_estimate.fastest_fee as f64,
            half_hour_fee: fee_estimate.half_hour_fee as f64,
            hour_fee: fee_estimate.hour_fee as f64,
            minimum_fee: fee_estimate.minimum_fee as f64,
            economy_fee: fee_estimate.economy_fee as f64,
        };

        Ok(fee_estimate)
    }

    /// Returns the name of this fee rate estimator.
    ///
    /// This can be used to distinguish between multiple providers.
    fn name(&self) -> &str {
        "Mempool"
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const MEMPOOL_API_URL: &str = "https://mempool.space/testnet4/api/v1/fees/recommended";

    fn get_mempool_provider(url: Url) -> MempoolFeeRateEstimator {
        MempoolFeeRateEstimator::new(url, None)
    }

    #[tokio::test]
    async fn test_mempool_fee_rate_success() {
        let url = Url::from_str(MEMPOOL_API_URL).unwrap();
        let provider = get_mempool_provider(url);

        let result = provider.get_fee_estimates().await;

        assert!(
            result.is_ok(),
            "Fee rate should be greater than 0 from fastestFee"
        );
    }

    #[tokio::test]
    async fn test_mempool_fee_rate_error() {
        let url = Url::from_str("http://localhost:9999/invalid-url").unwrap(); // Non-existent server
        let provider = get_mempool_provider(url);

        let result = provider.get_fee_estimates().await;
        assert!(result.is_err(), "Should return error on invalid URL");
    }
}
