use crate::{
    batcher::{batch_tx::build_batch_tx, sign::sign_batch_tx, traits::TxBatcher},
    ArcFeeRateEstimator, ArcIndexer, FeeLevel, FeeRate, ValidSpendRequests,
};
use async_trait::async_trait;
use bon::Builder;
use eyre::Result;

/// A batch processor for Bitcoin transactions that handles transaction building, signing, and submission.
#[derive(Builder)]
pub struct BitcoinTxBatcher {
    /// The Bitcoin indexer used to submit and interact with the network.
    indexer: ArcIndexer,

    /// The fee level used to calculate and adjust transaction fees.
    fee_level: FeeLevel,

    /// The fee rate estimator used to obtain the current Bitcoin network fee rate.
    fee_rate_estimator: ArcFeeRateEstimator,
}

impl BitcoinTxBatcher {
    /// Creates a new instance of `BitcoinTxBatcher`.
    ///
    /// # Arguments
    /// * `indexer` - The Bitcoin indexer used for transaction submission.
    /// * `fee_level` - The fee level used for transaction fee adjustments.
    /// * `fee_rate_estimator` - The fee rate estimator for calculating fees based on network conditions.
    ///
    /// # Returns
    /// * A new instance of `BitcoinTxBatcher`.
    pub fn new(
        indexer: ArcIndexer,
        fee_level: FeeLevel,
        fee_rate_estimator: ArcFeeRateEstimator,
    ) -> Self {
        Self {
            indexer,
            fee_level,
            fee_rate_estimator,
        }
    }

    /// Retrieves the current fee rate based on the fee level and fee rate estimator.
    ///
    /// # Returns
    /// * `Ok(FeeRate)` - The fee rate calculated using the current fee estimates.
    /// * `Err` - If the fee estimation fails.
    async fn get_fee_rate(&self) -> Result<FeeRate> {
        let fee_estimate = self.fee_rate_estimator.get_fee_estimates().await?;
        let fee_rate = FeeRate::new(self.fee_level.from(&fee_estimate))?;
        Ok(fee_rate)
    }

    /// Handles the process of building, signing, and submitting a batch transaction.
    ///
    /// # Arguments
    /// * `spend_requests` - A slice of spend requests for which the transaction is to be created.
    ///
    /// # Returns
    /// * `Ok(String)` - The transaction ID after successful submission.
    /// * `Err` - If any error occurs during the process.
    async fn handle_spend_requests(&self, spend_requests: &ValidSpendRequests) -> Result<String> {
        let fee_rate = self.get_fee_rate().await?;

        // Build the batch transaction.
        let mut tx = build_batch_tx(spend_requests, fee_rate)?;

        // Sign the batch transaction.
        sign_batch_tx(&mut tx, spend_requests)?;

        // Submit the batch transaction.
        self.indexer.submit_tx(&tx).await?;

        Ok(tx.compute_txid().to_string())
    }
}

#[async_trait]
impl TxBatcher for BitcoinTxBatcher {
    /// Executes the batch transaction process, validating requests and handling submission.
    ///
    /// # Arguments
    /// * `spend_requests` - A vector of spend requests that need to be processed.
    ///
    /// # Returns
    /// * `Ok(String)` - The transaction ID of the successfully submitted batch transaction.
    /// * `Err` - If validation or transaction handling fails.
    async fn execute(&self, spend_requests: ValidSpendRequests) -> Result<String> {
        self.handle_spend_requests(&spend_requests).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        get_test_bitcoin_indexer, get_test_bitcoin_tx_batcher, get_test_spend_request,
    };
    use std::sync::Arc;

    #[tokio::test]
    async fn test_batcher_execute_flow_spend_request() -> Result<()> {
        let _ = tracing_subscriber::fmt().try_init();

        let n = 4;
        let spend_requests = {
            let requests =
                futures::future::join_all((0..n).map(|_| async { get_test_spend_request().await }))
                    .await;
            let mut spend_requests = Vec::new();
            for request in requests {
                spend_requests.push(request?);
            }
            spend_requests
        };

        let spend_requests =
            ValidSpendRequests::validate(spend_requests, &Arc::new(get_test_bitcoin_indexer()?))
                .await?;

        let batcher = Arc::new(get_test_bitcoin_tx_batcher().await?);

        let response = batcher.execute(spend_requests).await;

        assert!(response.is_ok());

        Ok(())
    }
}
