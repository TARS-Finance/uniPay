use crate::fee_providers::{
    constants::DEFAULT_TIMEOUT_SECS, primitives::FeeEstimate, traits::FeeRateEstimator,
};
use async_trait::async_trait;
use eyre::{bail, eyre, Result};
use reqwest::{Client, Url};
use std::{collections::HashMap, time::Duration};

/// A fee rate estimator that fetches fee estimates from the Blockstream API.
#[derive(Debug, Clone)]
pub struct BlockstreamFeeRateEstimator {
    /// Base URL for the blockstream API
    url: Url,

    /// HTTP client with configured timeout
    client: Client,

    /// Request timeout duration
    timeout: Duration,
}

impl BlockstreamFeeRateEstimator {
    /// Creates a new instance of [`BlockstreamFeeRateEstimator`].
    ///
    /// # Arguments
    /// * `url` - The base URL of the Blockstream fee estimation API.
    /// * `timeout` - Optional timeout in seconds for HTTP requests. If `None`, defaults to `DEFAULT_TIMEOUT_SECS`.
    ///
    /// # Returns
    /// A configured `BlockstreamFeeRateEstimator`.
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
impl FeeRateEstimator for BlockstreamFeeRateEstimator {
    /// Fetches fee estimates from the Blockstream API.
    ///
    /// # Returns
    /// A [`FeeEstimate`] populated with fee rates for different confirmation targets.
    ///
    /// # Errors
    /// Returns an error if the request fails, the response is invalid, or required keys are missing.
    async fn get_fee_estimates(&self) -> Result<FeeEstimate> {
        let request = self.client.get(self.url.clone()).timeout(self.timeout);

        let response = request
            .send()
            .await
            .map_err(|err| eyre!(format!("Failed to send request : {:#?}", err.to_string())))?;

        if !response.status().is_success() {
            let status = response.status();
            let error = response.text().await.map_err(|err| {
                eyre!(format!(
                    "Failed to parse Blockstream API response: {:#?}",
                    err
                ))
            })?;

            bail!(format!(
                "Blockstream API returned status: {} with error: {}",
                status, error
            ))
        }

        let result: HashMap<String, f64> = response.json().await.map_err(|err| {
            eyre!(format!(
                "Failed to parse Blockstream API response: {:#?}",
                err
            ))
        })?;

        // Return Default fee estimate if result is empty.
        if result.is_empty() {
            return Ok(FeeEstimate::default());
        }

        let fee_estimate = {
            let fastest_fee = result
                .get("1")
                .ok_or_else(|| eyre!("Missing fee rate for target '1'".to_string()))?
                .clone();
            let half_hour_fee = result
                .get("3")
                .ok_or_else(|| eyre!("Missing fee rate for target '3'".to_string()))?
                .clone();
            let hour_fee = result
                .get("6")
                .ok_or_else(|| eyre!("Missing fee rate for target '6'".to_string()))?
                .clone();
            let minimum_fee = result
                .get("144")
                .ok_or_else(|| eyre!("Missing fee rate for target '144'".to_string()))?
                .clone();
            let economy_fee = result
                .get("504")
                .ok_or_else(|| eyre!("Missing fee rate for target '504'".to_string()))?
                .clone();

            FeeEstimate {
                fastest_fee,
                half_hour_fee,
                hour_fee,
                minimum_fee,
                economy_fee,
            }
        };

        Ok(fee_estimate)
    }

    /// Returns the name of this fee rate estimator.
    ///
    /// This can be used to distinguish between multiple providers.
    fn name(&self) -> &str {
        "Blockstream"
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;
    const BLOCKSTREAM_API_URL: &str = "https://blockstream.info/testnet/api/fee-estimates";

    fn get_blockstream_provider(url: Url) -> BlockstreamFeeRateEstimator {
        BlockstreamFeeRateEstimator::new(url, None)
    }

    #[tokio::test]
    async fn test_blockstream_fee_rate_success() {
        let url = Url::from_str(BLOCKSTREAM_API_URL).unwrap();
        let provider = get_blockstream_provider(url);
        let result = provider.get_fee_estimates().await.unwrap();

        assert!(
            result.fastest_fee > 0.0,
            "Fee rate should be greater than 0 for block target '1'"
        );
    }
}
