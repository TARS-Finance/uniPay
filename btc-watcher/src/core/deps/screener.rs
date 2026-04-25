use async_trait::async_trait;
use screener::client::{ScreenerRequest, ScreenerResponse};

/// Trait for address screening implementations
#[async_trait]
pub trait AddressScreener: Send + Sync {
    async fn is_blacklisted(
        &self,
        addresses: Vec<ScreenerRequest>,
    ) -> eyre::Result<Vec<ScreenerResponse>>;
}
