use crate::ValidSpendRequests;
use async_trait::async_trait;
use eyre::Result;
use mockall::automock;

/// A trait for handling the execution of batch Bitcoin transactions.
#[automock]
#[async_trait]
pub trait TxBatcher {
    /// Executes a batch transaction by processing the provided spend requests.
    ///
    /// This method will handle the creation, signing, and submission of a batch transaction.
    /// It returns the transaction ID of the successfully submitted batch transaction.
    ///
    /// # Arguments
    /// * `spend_requests` - A vector of valid `SpendRequest` objects
    async fn execute(&self, spend_requests: ValidSpendRequests) -> Result<String>;
}
