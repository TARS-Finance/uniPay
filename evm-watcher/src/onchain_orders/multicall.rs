use alloy::{primitives::Bytes, providers::Provider};
use eyre::Result;
use tars::evm::Multicall3::{Call3Value, Multicall3Instance};
use std::sync::Arc;

/// Represents a result from a multicall operation
#[derive(Debug, Clone)]
pub struct MulticallResult {
    /// Whether the call was successful
    pub success: bool,
    /// The return data from the call
    pub return_data: Bytes,
}

/// Simple multicall executor
pub struct Multicall<T: Provider> {
    pub multicall_contract: Arc<Multicall3Instance<Arc<T>>>,
}

impl<T: Provider> Multicall<T> {
    /// Create a new Multicall instance
    pub fn new(multicall_contract: Arc<Multicall3Instance<Arc<T>>>) -> Self {
        Self { multicall_contract }
    }

    /// Execute a read-only multicall and return results
    ///
    /// # Arguments
    /// * `calls` - Slice of Call3Value structs containing the calls to execute
    ///
    /// # Returns
    /// * `Ok(Vec<MulticallResult>)` - The results of each call in the batch
    /// * `Err(Error)` - If the execution fails
    pub async fn call(&self, calls: &[Call3Value]) -> Result<Vec<MulticallResult>> {
        if calls.is_empty() {
            return Err(eyre::eyre!("No calls to execute"));
        }

        let results = self
            .multicall_contract
            .aggregate3Value(calls.to_vec())
            .call()
            .await
            .map_err(|e| eyre::eyre!("Failed to execute multicall: {}", e))?;

        Ok(results
            .into_iter()
            .map(|result| MulticallResult {
                success: result.success,
                return_data: result.returnData,
            })
            .collect())
    }
}
