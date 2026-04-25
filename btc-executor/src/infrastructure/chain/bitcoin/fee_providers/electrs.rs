//! Electrs (Esplora) fee rate estimator.
//!
//! Wraps the existing ElectrsClient's fee-estimates endpoint.

use super::primitives::FeeEstimate;
use super::traits::{FeeEstimatorError, FeeRateEstimator};
use async_trait::async_trait;
use std::sync::Arc;

use crate::infrastructure::chain::bitcoin::clients::ElectrsClient;

/// Fee rate estimator using Electrs (Esplora) `/fee-estimates` endpoint.
///
/// Maps block targets to FeeEstimate:
/// - 1 → fastest_fee
/// - 3 → half_hour_fee
/// - 6 → hour_fee
/// - 144 → minimum_fee
/// - 504 → economy_fee
pub struct ElectrsFeeRateEstimator {
    electrs: Arc<ElectrsClient>,
}

impl ElectrsFeeRateEstimator {
    pub fn new(electrs: Arc<ElectrsClient>) -> Self {
        Self { electrs }
    }
}

#[async_trait]
impl FeeRateEstimator for ElectrsFeeRateEstimator {
    async fn get_fee_estimates(&self) -> Result<FeeEstimate, FeeEstimatorError> {
        let estimates = self
            .electrs
            .get_fee_estimates()
            .await
            .map_err(|e| FeeEstimatorError::Provider(e.to_string()))?;

        if estimates.is_empty() {
            return Ok(FeeEstimate::default());
        }

        let fee = FeeEstimate {
            fastest_fee: estimates.get("1").copied().unwrap_or(2.0),
            half_hour_fee: estimates.get("3").copied().unwrap_or(2.0),
            hour_fee: estimates.get("6").copied().unwrap_or(2.0),
            minimum_fee: estimates.get("144").copied().unwrap_or(2.0),
            economy_fee: estimates.get("504").copied().unwrap_or(2.0),
        };

        Ok(fee)
    }

    fn name(&self) -> &str {
        "Electrs"
    }
}
