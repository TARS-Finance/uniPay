use crate::{fee_providers::primitives::FeeEstimate, fee_providers::traits::FeeRateEstimator};
use async_trait::async_trait;
use eyre::Result;

/// A simple fee rate estimator that returns a fixed fee rate
/// This is useful for testing or when you want to use a predetermined fee rate
#[derive(Debug, Clone)]
pub struct FixedFeeRateEstimator {
    fee_rate: f64,
}

impl FixedFeeRateEstimator {
    /// Creates a new FixedFeeRateEstimator with the specified fee rate
    ///
    /// # Arguments
    /// * `fee_rate` - The fixed fee rate in satoshis per vbyte
    pub fn new(fee_rate: f64) -> Self {
        Self { fee_rate }
    }
}

#[async_trait]
impl FeeRateEstimator for FixedFeeRateEstimator {
    async fn get_fee_estimates(&self) -> Result<FeeEstimate> {
        Ok(FeeEstimate {
            fastest_fee: self.fee_rate,
            half_hour_fee: self.fee_rate,
            hour_fee: self.fee_rate,
            economy_fee: self.fee_rate,
            minimum_fee: self.fee_rate,
        })
    }

    fn name(&self) -> &str {
        "Fixed"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fixed_fee_rate_estimator() {
        let fee_rate = 10.0;
        let estimator = FixedFeeRateEstimator::new(fee_rate);

        let fee_estimate = estimator.get_fee_estimates().await;
        assert!(fee_estimate.is_ok());
        assert_eq!(fee_estimate.unwrap().fastest_fee, fee_rate);
    }

    #[tokio::test]
    async fn test_fixed_fee_rate_estimator_zero() {
        let fee_rate = 0.0;
        let estimator = FixedFeeRateEstimator::new(fee_rate);

        let fee_estimate = estimator.get_fee_estimates().await;
        assert!(fee_estimate.is_ok());
        assert_eq!(fee_estimate.unwrap().fastest_fee, fee_rate);
    }
}
