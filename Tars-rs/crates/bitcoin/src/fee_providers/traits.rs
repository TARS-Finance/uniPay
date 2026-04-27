use crate::fee_providers::primitives::FeeEstimate;
use async_trait::async_trait;
use eyre::Result;
use mockall::automock;

/// Trait for fee rate estimation providers
#[automock]
#[async_trait]
pub trait FeeRateEstimator: Send + Sync {
    /// Fetches current fee rate estimates
    async fn get_fee_estimates(&self) -> Result<FeeEstimate>;

    /// Returns the name of this fee rate estimator
    fn name(&self) -> &str;
}
