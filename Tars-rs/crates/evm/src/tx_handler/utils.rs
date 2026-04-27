use crate::tx_handler::primitives::TransactionStatus;
use alloy::{
    consensus::Transaction, eips::eip1559::Eip1559Estimation, primitives::FixedBytes,
    providers::Provider,
};
use eyre::{eyre, Result};
use std::time::Duration;

/// Time to sleep between checking transaction status
const TRANSACTION_STATUS_CHECK_INTERVAL: Duration = Duration::from_millis(3000);

pub async fn parse_transaction_status(
    tx_hash: FixedBytes<32>,
    provider: &impl Provider,
) -> Result<TransactionStatus> {
    match provider.get_transaction_by_hash(tx_hash).await {
        Ok(Some(tx)) => {
            if tx.block_number.is_some() {
                let receipt = provider
                    .get_transaction_receipt(tx_hash)
                    .await
                    .map_err(|e| eyre!(e.to_string()))?;

                match receipt {
                    Some(receipt) => match receipt.inner.status() {
                        true => Ok(TransactionStatus::Confirmed),
                        false => Ok(TransactionStatus::Reverted),
                    },
                    None => Ok(TransactionStatus::Pending),
                }
            } else {
                Ok(TransactionStatus::Pending)
            }
        }
        Ok(None) => Ok(TransactionStatus::NotFound),
        Err(e) => return Err(eyre!(e.to_string())),
    }
}

/// Calculates EIP-1559 compliant replacement transaction fees
pub async fn calculate_replacement_fees(
    provider: &impl Provider,
    original_tx: &alloy::rpc::types::Transaction,
) -> Result<Eip1559Estimation> {
    let current_fees = provider.estimate_eip1559_fees().await?;

    let previous_priority_fee = original_tx
        .max_priority_fee_per_gas()
        .unwrap_or(1_000_000_000);
    let base_fee_component = current_fees.max_fee_per_gas - current_fees.max_priority_fee_per_gas;

    let new_max_priority_fee = std::cmp::max(
        previous_priority_fee * 130 / 100, // 30% bump
        current_fees.max_priority_fee_per_gas,
    );

    let new_max_fee_per_gas = base_fee_component + new_max_priority_fee;

    Ok(Eip1559Estimation {
        max_fee_per_gas: new_max_fee_per_gas,
        max_priority_fee_per_gas: new_max_priority_fee,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum WaitForConfirmationError {
    #[error("timeout")]
    Timeout,
    #[error("transaction fetch failed")]
    FetchFailed(String),
}

/// Waits for the transaction to get mined or timeouts
pub async fn wait_for_confirmation(
    provider: &impl Provider,
    tx_hash: FixedBytes<32>,
    timeout: Duration,
) -> Result<TransactionStatus, WaitForConfirmationError> {
    let confirmation_future = async {
        loop {
            // we can only get confirmed, reverted or not found statuses
            let result = parse_transaction_status(tx_hash, provider)
                .await
                .map_err(|e| WaitForConfirmationError::FetchFailed(e.to_string()));
            let r = match result {
                Ok(TransactionStatus::Pending) | Ok(TransactionStatus::NotFound) => {
                    tokio::time::sleep(TRANSACTION_STATUS_CHECK_INTERVAL).await;
                    continue;
                }
                Ok(status) => Ok(status),
                Err(e) => Err(e),
            };
            return r;
        }
    };

    match tokio::time::timeout(timeout, confirmation_future).await {
        Ok(result) => result,
        Err(_) => Err(WaitForConfirmationError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use crate::tx_handler::primitives::TransactionStatus;
    use crate::tx_handler::utils::parse_transaction_status;
    use alloy::primitives::FixedBytes;
    use alloy::providers::ProviderBuilder;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_parse_transaction_status() {
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .with_gas_estimation()
            .with_simple_nonce_management()
            .fetch_chain_id()
            .connect_http("https://sepolia-rollup.arbitrum.io/rpc".parse().unwrap());
        let tx_hash = FixedBytes::from_str(
            "0x43892ba14577bc52083425e20034c369fb06dbbefe3402f67de2d8decfcd5f58",
        )
        .unwrap();
        let result = parse_transaction_status(tx_hash, &provider).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TransactionStatus::Reverted);

        let tx_hash = FixedBytes::from_str(
            "0x29f8973ceb025908fde54a38e5c88d4a5dea76c2a3ff1fe8f7cde4b2187fdb4e",
        )
        .unwrap();
        let result = parse_transaction_status(tx_hash, &provider).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), TransactionStatus::Confirmed);
    }
}
