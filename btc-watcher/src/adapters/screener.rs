use crate::core::AddressScreener as CoreAddressScreener;
use async_trait::async_trait;
use moka::future::Cache;
use screener::client::{AddressScreener, GardenAddressScreener, ScreenerRequest, ScreenerResponse};
use std::time::Duration;

const DEFAULT_CACHE_TTL: Duration = Duration::from_secs(2 * 60 * 60);

/// Address screening client
#[derive(Clone)]
pub struct AddressScreenerClient {
    screener: GardenAddressScreener,
    cache: Cache<String, bool>,
}

impl AddressScreenerClient {
    pub fn new(url: String) -> Self {
        let cache = Cache::builder().time_to_live(DEFAULT_CACHE_TTL).build();
        Self {
            screener: GardenAddressScreener::new(url),
            cache,
        }
    }
}

#[async_trait]
impl CoreAddressScreener for AddressScreenerClient {
    async fn is_blacklisted(
        &self,
        addresses: Vec<ScreenerRequest>,
    ) -> eyre::Result<Vec<ScreenerResponse>> {
        // Partition addresses into cached and uncached
        let mut cached_responses = Vec::new();
        let mut uncached_requests = Vec::new();

        for address in addresses {
            match self.cache.get(&address.address).await {
                Some(is_blacklisted) => {
                    cached_responses.push(ScreenerResponse {
                        address: address.address,
                        chain: address.chain,
                        is_blacklisted,
                    });
                }
                None => uncached_requests.push(address),
            }
        }

        // Screen uncached addresses
        let screener_responses = match uncached_requests.is_empty() {
            true => Vec::new(),
            false => self
                .screener
                .is_blacklisted(uncached_requests)
                .await
                .map_err(|e| eyre::eyre!("Failed to screen addresses: {}", e))?,
        };

        // Cache all responses (both blacklisted and non-blacklisted)
        for response in &screener_responses {
            self.cache
                .insert(response.address.clone(), response.is_blacklisted)
                .await;
        }

        // Combine cached and fresh responses
        let mut all_responses = cached_responses;
        all_responses.extend(screener_responses);
        Ok(all_responses)
    }
}

pub struct FixedStatusScreener {
    is_blacklisted: bool,
}

impl FixedStatusScreener {
    pub fn new(is_blacklisted: bool) -> Self {
        Self { is_blacklisted }
    }
}

#[async_trait]
impl CoreAddressScreener for FixedStatusScreener {
    async fn is_blacklisted(
        &self,
        addresses: Vec<ScreenerRequest>,
    ) -> eyre::Result<Vec<ScreenerResponse>> {
        let responses = addresses
            .into_iter()
            .map(|address| ScreenerResponse {
                address: address.address,
                chain: address.chain,
                is_blacklisted: self.is_blacklisted,
            })
            .collect();

        Ok(responses)
    }
}
