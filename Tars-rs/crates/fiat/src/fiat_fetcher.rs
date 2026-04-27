use crate::errors::FiatError;
use api::primitives::Response;
use moka::future::Cache;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Represents Price information for a token pair
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FiatPriceResult {
    /// Current price of the input token in fiat currency
    pub input_token_price: f64,
    /// Current price of the output token in fiat currency
    pub output_token_price: f64,
}

/// Default cache time-to-live in seconds
const DEFAULT_CACHE_TTL_SECS: u64 = 5;
/// Request timeout for the fiat API in seconds
const REQUEST_TIMEOUT_SECS: u64 = 5;

/// The FiatProvider fetches and caches fiat prices for token pairs
///
/// Uses an in-memory cache to reduce API calls and improve performance.
#[derive(Debug, Clone)]
pub struct FiatProvider {
    /// Base URL for the price API endpoint
    api_url: String,
    /// Cache for the fiat prices
    cache: Cache<String, FiatPriceResult>,
    /// HTTP client for making requests
    client: Client,
}

impl FiatProvider {
    /// Creates a new FiatProvider instance
    ///
    /// # Arguments
    ///
    /// * `base_url` - Base URL for the price API
    /// * `cache_ttl` - Time-to-live for cache entries in seconds (default: 5)
    ///
    /// # Returns
    ///
    /// A new FiatProvider instance or an error
    ///
    /// # Errors
    ///
    /// Returns `FiatError::FiatProviderCreationFailed` if the HTTP client
    /// cannot be created
    pub fn new(base_url: &str, cache_ttl: Option<u64>) -> Result<Self, FiatError> {
        let api_url = format!("{}/fiat", base_url);

        // Create cache with configurable TTL
        let ttl = Duration::from_secs(cache_ttl.unwrap_or(DEFAULT_CACHE_TTL_SECS));
        let cache = Cache::builder().time_to_live(ttl).build();

        // Set up HTTP client with timeout
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECS))
            .build()
            .map_err(|err| {
                let msg = format!("Failed to create HTTP client: {}", err);
                FiatError::FiatProviderCreationFailed(msg)
            })?;

        Ok(Self {
            api_url,
            cache,
            client,
        })
    }

    /// Fetches the fiat price for a given order pair
    ///
    /// Returns cached values if available and not expired.
    ///
    /// # Arguments
    ///
    /// * `order_pair` - Order pair in format "source_chain:source_asset::destination_chain:destination_asset"
    ///
    /// # Returns
    ///
    /// Tuple of (input_token_price, output_token_price) or an error
    ///
    /// # Errors
    ///
    /// Returns `FiatError` variants for network errors, API errors, or parsing failures
    pub async fn get_price(&self, order_pair: &str) -> Result<(f64, f64), FiatError> {
        // Try to get from cache first
        if let Some(result) = self.cache.get(order_pair).await {
            return Ok((result.input_token_price, result.output_token_price));
        }

        self.fetch_price_from_api(order_pair).await
    }

    /// Fetches price from the API and updates cache
    ///
    /// Internal helper method to handle the actual API request
    async fn fetch_price_from_api(&self, order_pair: &str) -> Result<(f64, f64), FiatError> {
        // Build URL with query parameters
        let url = format!("{}?order_pair={}", self.api_url, order_pair);

        // Execute request
        let response = self.client.get(&url).send().await.map_err(|err| {
            let msg = format!("API request failed: {}", err);
            FiatError::FiatApiRequestFailed(msg)
        })?;

        // Check HTTP status
        let status = response.status();
        if !status.is_success() {
            let msg = format!("API returned error status: {}", status);

            // For specific status codes, return more specific errors
            return match status {
                StatusCode::NOT_FOUND => Err(FiatError::FiatApiError(format!(
                    "Order pair not found: {}",
                    order_pair
                ))),
                StatusCode::BAD_REQUEST => Err(FiatError::FiatApiError(format!(
                    "Invalid order pair format: {}",
                    order_pair
                ))),
                _ => Err(FiatError::FiatApiError(msg)),
            };
        }

        // Parse JSON response
        let price_result: Response<FiatPriceResult> = response.json().await.map_err(|err| {
            let msg = format!("Failed to parse API response: {}", err);
            FiatError::FiatApiError(msg)
        })?;

        // Handle response based on content
        match price_result {
            Response {
                error: Some(error), ..
            } => Err(FiatError::FiatApiError(error)),
            Response {
                result: Some(result),
                ..
            } => {
                // Update cache
                self.cache
                    .insert(order_pair.to_string(), result.clone())
                    .await;
                Ok((result.input_token_price, result.output_token_price))
            }
            _ => {
                let msg = "API returned empty response with no error";
                Err(FiatError::FiatApiError(msg.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{start_mock_fiat_server, MOCK_FIAT_SERVER_URL};

    const API_URL: &str = "http://localhost:6969";
    const CACHE_TTL: u64 = 5;
    const ORDER_PAIR: &str =
        "bitcoin_regtest:primary::ethereum_localnet:0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0";

    #[tokio::test]
    async fn test_fiat_provider_get_price() {
        let fiat_provider = FiatProvider::new(API_URL, Some(CACHE_TTL)).unwrap();
        let result = fiat_provider.get_price(ORDER_PAIR).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_fiat_provider_get_price_with_invalid_order_pair() {
        let fiat_provider = FiatProvider::new(API_URL, Some(CACHE_TTL)).unwrap();
        let result = fiat_provider.get_price("invalid_order_pair").await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid order pair format"));
    }

    #[tokio::test]
    async fn test_fiat_provider_cache_ttl() {
        // Start mock server
        start_mock_fiat_server().await;

        // Create provider with short cache TTL
        let fiat_provider = FiatProvider::new(MOCK_FIAT_SERVER_URL, Some(CACHE_TTL)).unwrap();

        // First call - should hit API
        let cached_result = fiat_provider.get_price(ORDER_PAIR).await.unwrap();

        // Second call within TTL - should use cache
        tokio::time::sleep(Duration::from_secs(2)).await;
        let result = fiat_provider.get_price(ORDER_PAIR).await.unwrap();
        assert_eq!(
            cached_result, result,
            "Cache should return the same values within TTL"
        );

        // Wait for cache expiration
        tokio::time::sleep(Duration::from_secs(6)).await;

        // Call after expiration - should hit API again with new values
        let new_result = fiat_provider.get_price(ORDER_PAIR).await.unwrap();
        assert_ne!(
            new_result, cached_result,
            "Should get different values after cache expires"
        );
    }

    #[tokio::test]
    async fn test_cache_prioritized_over_api() {
        // Create a provider with long cache TTL
        let fiat_provider = FiatProvider::new(API_URL, Some(60)).unwrap();

        // Mock a cache entry directly
        let mock_result = FiatPriceResult {
            input_token_price: 42.0,
            output_token_price: 24.0,
        };

        fiat_provider
            .cache
            .insert(ORDER_PAIR.to_string(), mock_result.clone())
            .await;

        // Get price should return the cached value without calling the API
        let result = fiat_provider.get_price(ORDER_PAIR).await.unwrap();
        assert_eq!(
            result,
            (42.0, 24.0),
            "Should return exactly the cached values"
        );
    }
}
