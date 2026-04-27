use crate::primitives::TxOptions;
use alloy::primitives::FixedBytes;
use eyre::Result;

/// Trait for submitting replacement transactions
#[async_trait::async_trait]
pub trait TransactionSubmitter<T>: Send + Sync {
    /// Submit a transaction with the given requests and options
    async fn submit_transaction(
        &mut self,
        requests: &[T],
        tx_options: TxOptions,
    ) -> Result<FixedBytes<32>>;
}
