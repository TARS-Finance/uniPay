//! Fee rate estimator trait used by the Bitcoin wallet runner.

use super::primitives::FeeEstimate;
use async_trait::async_trait;

/// Trait for fee rate estimation providers.
///
/// The standalone executor currently implements this with Electrs-backed
/// estimates, but the runner depends only on this small abstraction.
#[async_trait]
pub trait FeeRateEstimator: Send + Sync {
    /// Fetches current fee rate estimates.
    async fn get_fee_estimates(&self) -> Result<FeeEstimate, FeeEstimatorError>;

    /// Returns the name of this fee rate estimator for logs.
    fn name(&self) -> &str;
}

/// Errors from fee rate estimators.
#[derive(Debug, thiserror::Error)]
pub enum FeeEstimatorError {
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("provider error: {0}")]
    Provider(String),
}
