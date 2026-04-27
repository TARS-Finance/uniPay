use crate::{
    fee_providers::{primitives::FeeEstimate, traits::FeeRateEstimator},
    ArcFeeRateEstimator,
};
use async_trait::async_trait;
use eyre::Result;
use std::collections::HashMap;
use thiserror::Error;

/// Errors that can occur when using multiple fee estimators
#[derive(Debug, Error)]
pub enum MultiFeeEstimatorError {
    /// Returns the name of this fee rate estimato
    #[error("No fee estimators are configured")]
    EmptyFeeRateEstimators,

    /// All configured fee estimators failed
    #[error("All fee estimators failed: {errors:?}")]
    AllEstimatorsFailed {
        /// Map of provider names to their error messages
        errors: HashMap<String, String>,
    },
}

/// Result type for multi-fee estimator operations
pub type MultiFeeEstimatorResult<T> = Result<T, MultiFeeEstimatorError>;

/// Fee rate estimator that aggregates multiple fee sources
///
/// Queries multiple fee rate estimators in sequence until a valid fee rate is obtained.
/// Useful for maintaining fee rate availability through provider redundancy.
pub struct MultiFeeRateEstimator {
    /// List of fee rate estimators to query
    estimators: Vec<ArcFeeRateEstimator>,
}

/// Builder for constructing MultiFeeProvider instances
pub struct MultiFeeRateEstimatorBuilder {
    estimators: Vec<ArcFeeRateEstimator>,
}

impl MultiFeeRateEstimatorBuilder {
    /// Creates a new empty builder instance
    pub fn new() -> Self {
        Self {
            estimators: Vec::new(),
        }
    }

    /// Adds a fee estimator to the builder
    ///
    /// # Arguments
    /// * `estimator` - Fee rate estimator implementation to add
    pub fn with_fee_rate_estimator(mut self, estimator: ArcFeeRateEstimator) -> Self {
        self.estimators.push(estimator);
        self
    }

    /// Constructs a MultiFeeProvider from configured providers
    ///
    /// # Errors
    /// * Returns EmptyFeeProvider if no providers were added
    pub fn build(self) -> MultiFeeEstimatorResult<MultiFeeRateEstimator> {
        if self.estimators.is_empty() {
            return Err(MultiFeeEstimatorError::EmptyFeeRateEstimators);
        }

        Ok(MultiFeeRateEstimator {
            estimators: self.estimators,
        })
    }
}

impl MultiFeeRateEstimator {
    /// Creates a new builder for constructing MultiFeeProvider instances
    pub fn builder() -> MultiFeeRateEstimatorBuilder {
        MultiFeeRateEstimatorBuilder::new()
    }
}

#[async_trait]
impl FeeRateEstimator for MultiFeeRateEstimator {
    /// Queries configured providers for fee rate estimates
    ///
    /// Attempts to get fee estimate from each estimator in sequence
    /// until one succeeds. Returns the first successful result.
    ///
    /// # Returns
    /// * Fee rate estimate from first successful provider
    ///
    /// # Errors
    /// * FeeError if all providers fail
    /// Tries all configured providers for a fee estimate
    async fn get_fee_estimates(&self) -> Result<FeeEstimate> {
        let mut errors = HashMap::new();

        for estimator in self.estimators.iter() {
            let name = estimator.name();
            match estimator.get_fee_estimates().await {
                Ok(fee) => return Ok(fee),
                Err(e) => {
                    errors.insert(name.to_string(), e.to_string());
                }
            }
        }

        Err(MultiFeeEstimatorError::AllEstimatorsFailed { errors }.into())
    }

    fn name(&self) -> &str {
        "MultiFeeRateEstimator"
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use eyre::{eyre, Result};
    use std::sync::Arc;

    #[derive(Debug)]
    struct MockFeeProvider {
        name: &'static str,
        should_fail: bool,
        response: Option<FeeEstimate>,
    }

    #[async_trait]
    impl FeeRateEstimator for MockFeeProvider {
        async fn get_fee_estimates(&self) -> Result<FeeEstimate> {
            if self.should_fail {
                Err(eyre!(format!("{} failed", self.name)))
            } else {
                Ok(self.response.clone().unwrap_or_default())
            }
        }

        fn name(&self) -> &str {
            "Mock"
        }
    }

    fn mock_provider(
        name: &'static str,
        should_fail: bool,
        suggestion: Option<FeeEstimate>,
    ) -> ArcFeeRateEstimator {
        Arc::new(MockFeeProvider {
            name,
            should_fail,
            response: suggestion,
        })
    }

    #[tokio::test]
    async fn test_first_successful_provider_returns_result() {
        let provider1 = mock_provider("fail1", true, None);
        let provider2 = mock_provider(
            "success2",
            false,
            Some(FeeEstimate {
                fastest_fee: 120.0,
                ..Default::default()
            }),
        );

        let multi = MultiFeeRateEstimator::builder()
            .with_fee_rate_estimator(provider1)
            .with_fee_rate_estimator(provider2)
            .build()
            .unwrap();

        let result = multi.get_fee_estimates().await.unwrap();
        assert_eq!(result.fastest_fee, 120.0);
    }

    #[tokio::test]
    async fn test_all_providers_fail() {
        let provider1 = mock_provider("fail1", true, None);
        let provider2 = mock_provider("fail2", true, None);

        let multi = MultiFeeRateEstimator::builder()
            .with_fee_rate_estimator(provider1)
            .with_fee_rate_estimator(provider2)
            .build()
            .unwrap();

        let result = multi.get_fee_estimates().await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_only_first_provider_succeeds() {
        let provider1 = mock_provider(
            "success1",
            false,
            Some(FeeEstimate {
                minimum_fee: 10.0,
                ..Default::default()
            }),
        );

        let provider2 = mock_provider(
            "success2",
            false,
            Some(FeeEstimate {
                minimum_fee: 50.0,
                ..Default::default()
            }),
        );

        let multi = MultiFeeRateEstimator::builder()
            .with_fee_rate_estimator(provider1)
            .with_fee_rate_estimator(provider2)
            .build()
            .unwrap();

        let result = multi.get_fee_estimates().await.unwrap();
        assert_eq!(result.minimum_fee, 10.0); // Should NOT return 50.0 from provider2
    }

    #[tokio::test]
    async fn test_builder_with_no_providers_should_fail() {
        let result = MultiFeeRateEstimator::builder().build();
        assert!(matches!(
            result,
            Err(MultiFeeEstimatorError::EmptyFeeRateEstimators)
        ));
    }
}
