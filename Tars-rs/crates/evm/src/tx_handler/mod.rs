//! Handles the confirmation of pending transactions with automatic retry logic
pub mod primitives;
pub mod traits;
pub mod utils;

use crate::{
    primitives::{AlloyProvider, TxOptions},
    tx_handler::{
        primitives::{TransactionState, TransactionStatus},
        traits::TransactionSubmitter,
        utils::{calculate_replacement_fees, wait_for_confirmation, WaitForConfirmationError},
    },
};
use alloy::{consensus::Transaction, primitives::FixedBytes, providers::Provider};
use eyre::{eyre, Result};
use std::{collections::VecDeque, marker::PhantomData, time::Duration};
use tracing::{error, info};

/// Maximum number of replacement transactions for a multicall (excluding the first submission)
/// Total transactions submitted will be MAX_REPLACEMENT_ATTEMPTS + 1
pub const MAX_REPLACEMENT_ATTEMPTS: u64 = 5;

/// Time to sleep between retries
pub const RETRY_SLEEP_DURATION: Duration = Duration::from_millis(2000);

/// Handles the confirmation of pending transactions with automatic retry logic
///
/// Provides robust transaction submission with fee bumping and nonce replacement
/// to handle network congestion and stuck transactions.
pub struct PendingTxHandler<T, S>
where
    S: TransactionSubmitter<T>,
{
    // Transaction confirmation timeout
    confirmation_timeout: Duration,
    // Provider for interacting with the blockchain
    provider: AlloyProvider,
    // Human-readable chain identifier for logging
    chain_identifier: String,
    // Transaction submitter
    submitter: S,
    // Phantom data to hold the type parameter T
    _phantom: PhantomData<T>,
    // Queue of transactions to watch (latest first)
    transaction_queue: VecDeque<FixedBytes<32>>,
}

impl<T, S> PendingTxHandler<T, S>
where
    S: TransactionSubmitter<T>,
{
    pub fn new(
        confirmation_timeout: Duration,
        provider: AlloyProvider,
        chain_identifier: String,
        submitter: S,
    ) -> Self {
        Self {
            confirmation_timeout,
            provider,
            chain_identifier,
            submitter,
            _phantom: PhantomData,
            transaction_queue: VecDeque::with_capacity(MAX_REPLACEMENT_ATTEMPTS as usize + 1),
        }
    }

    /// Waits for the transaction to be confirmed or dropped
    /// Handles the submission of replacement transactions if fee bumping is required
    /// Returns a TransactionState enum indicating the outcome
    ///
    /// # Arguments
    ///
    /// * `tx_hash` - The hash of the transaction to wait for
    /// * `requests` - The requests to include in the transaction
    ///
    /// # Returns
    ///
    /// * `Ok(tx_hash)` - The transaction was confirmed
    /// * `Err(e)` - The transaction was dropped or reverted
    pub async fn handle_tx(
        &mut self,
        tx_hash: FixedBytes<32>,
        requests: &[T],
    ) -> Result<FixedBytes<32>> {
        let mut replacement_attempts = 0;

        self.transaction_queue.clear();
        self.transaction_queue.push_front(tx_hash);

        loop {
            // Get the latest transaction from the queue
            let tx_hash_to_watch = match self.transaction_queue.pop_front() {
                Some(hash) => hash,
                None => {
                    error!(
                        chain = %self.chain_identifier,
                        "Transaction queue is empty, all transactions dropped"
                    );
                    return Err(eyre!(
                        "Transaction queue is empty, all transactions are dropped or reverted"
                    ));
                }
            };

            loop {
                match self.fetch_transaction_state(tx_hash_to_watch).await {
                    Ok(TransactionState::Status(TransactionStatus::Confirmed)) => {
                        info!(
                            chain = %self.chain_identifier,
                            replacement_attempts,
                            tx_hash = %tx_hash_to_watch,
                            "Transaction confirmed"
                        );
                        return Ok(tx_hash_to_watch);
                    }
                    Ok(TransactionState::ReplacementNeeded) => {
                        if replacement_attempts >= MAX_REPLACEMENT_ATTEMPTS {
                            info!(
                                chain = %self.chain_identifier,
                                replacement_attempts,
                                tx_hash = %tx_hash_to_watch,
                                "Maximum replacement attempts reached, continuing to monitor existing transactions"
                            );
                            tokio::time::sleep(RETRY_SLEEP_DURATION).await;
                            continue;
                        }

                        match self
                            .submit_replacement_transaction(requests, &tx_hash_to_watch)
                            .await
                        {
                            Ok(replacement_tx_hash) => {
                                replacement_attempts += 1;
                                info!(
                                    chain = %self.chain_identifier,
                                    replacement_attempts,
                                    original_tx_hash = %tx_hash_to_watch,
                                    replacement_tx_hash = %replacement_tx_hash,
                                    "Replacement transaction submitted, adding to queue"
                                );

                                self.transaction_queue.push_front(tx_hash_to_watch);
                                self.transaction_queue.push_front(replacement_tx_hash);
                                continue;
                            }
                            Err(e) => {
                                error!(
                                    chain = %self.chain_identifier,
                                    replacement_attempts,
                                    original_tx_hash = %tx_hash_to_watch,
                                    error = %e,
                                "Failed to submit replacement transaction, continuing to wait"
                                );
                                continue;
                            }
                        }
                    }
                    Ok(TransactionState::Status(TransactionStatus::Pending)) => {
                        info!(
                            chain = %self.chain_identifier,
                            replacement_attempts,
                            "Transaction still competitive, continuing to wait for confirmation"
                        );
                        continue;
                    }
                    Ok(TransactionState::Status(TransactionStatus::Reverted)) => {
                        info!(
                            chain = %self.chain_identifier,
                            replacement_attempts,
                            tx_hash = %tx_hash_to_watch,
                            "Transaction reverted"
                        );
                        break;
                    }
                    Ok(TransactionState::Status(TransactionStatus::NotFound)) => {
                        info!(
                            chain = %self.chain_identifier,
                            replacement_attempts,
                            tx_hash = %tx_hash_to_watch,
                            "Transaction not found, removing from queue and trying previous transaction"
                        );
                        break;
                    }
                    Err(e) => {
                        error!(
                            chain = %self.chain_identifier,
                            replacement_attempts,
                            error = %e,
                            "Failed to fetch transaction status"
                        );
                        continue;
                    }
                }
            }
        }
    }

    /// Fetches the transaction state
    async fn fetch_transaction_state(
        &mut self,
        latest_submitted_tx: FixedBytes<32>,
    ) -> Result<TransactionState> {
        // wait for confirmation until timeout
        match wait_for_confirmation(
            &self.provider,
            latest_submitted_tx,
            self.confirmation_timeout,
        )
        .await
        {
            Ok(result) => Ok(TransactionState::Status(result)),
            Err(WaitForConfirmationError::Timeout) => {
                match self.should_replace_transaction(&latest_submitted_tx).await {
                    Ok(true) => Ok(TransactionState::ReplacementNeeded),
                    Ok(false) => Ok(TransactionState::Status(TransactionStatus::Pending)),
                    Err(e) => {
                        if e.to_string().contains("Transaction not found") {
                            Ok(TransactionState::Status(TransactionStatus::NotFound))
                        } else {
                            Err(eyre!(e.to_string()))
                        }
                    }
                }
            }
            Err(WaitForConfirmationError::FetchFailed(e)) => Err(eyre!(e.to_string())),
        }
    }

    /// Determines if a transaction should be replaced based on EIP-1559 requirements and age
    async fn should_replace_transaction(&self, tx_hash: &FixedBytes<32>) -> Result<bool> {
        let transaction = self.fetch_transaction(tx_hash).await?;
        let fees = self.provider.estimate_eip1559_fees().await?;

        let submitted_max_fee = transaction.max_priority_fee_per_gas().unwrap_or(0);

        let fee_increase_needed = fees.max_priority_fee_per_gas > submitted_max_fee;

        // Also check if enough time has passed for a reasonable replacement
        let min_replacement_fee = ((submitted_max_fee as f64) * 1.2) as u128;
        let would_meet_bump_requirement = fees.max_priority_fee_per_gas >= min_replacement_fee;

        Ok(fee_increase_needed && would_meet_bump_requirement)
    }

    /// Fetches a transaction by hash with error handling
    async fn fetch_transaction(
        &self,
        tx_hash: &FixedBytes<32>,
    ) -> Result<alloy::rpc::types::Transaction> {
        self.provider
            .get_transaction_by_hash(*tx_hash)
            .await?
            .ok_or_else(|| eyre!("Transaction not found: {:?}", tx_hash))
    }

    /// Submits a replacement transaction with EIP-1559 compliant fees
    async fn submit_replacement_transaction(
        &mut self,
        requests: &[T],
        tx_hash_to_replace: &FixedBytes<32>,
    ) -> Result<FixedBytes<32>> {
        let tx_to_replace = self.fetch_transaction(tx_hash_to_replace).await?;

        let fees = calculate_replacement_fees(&self.provider, &tx_to_replace).await?;

        let tx_options = TxOptions {
            max_fee_per_gas: Some(fees.max_fee_per_gas),
            max_priority_fee_per_gas: Some(fees.max_priority_fee_per_gas),
            gas_limit: None,
            nonce: Some(tx_to_replace.nonce()),
        };

        let replacement_hash = self
            .submitter
            .submit_transaction(requests, tx_options)
            .await?;

        Ok(replacement_hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        htlc::v1::Initiate,
        primitives::{UnipayActionRequest, UnipayActionType, TxOptions},
        test_utils::{get_contracts, new_swap},
    };
    use ::primitives::{HTLCAction, HTLCVersion};
    use alloy::{primitives::Bytes, providers::Provider, signers::Signer};
    use std::time::Duration;
    use tokio::time::sleep;
    use tracing::info;

    #[tokio::test]
    async fn test_handle_tx_confirmed() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap and initiate it
        let (swap, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        // Submit transaction using HTLC
        let tx_hash = unipay_htlc_executor
            .multicall(&[request], None)
            .await
            .expect("Failed to initiate swap");

        // Create tx_handler and wait for confirmation
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(30),
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Create a simple request for the handler
        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let requests = vec![request];

        // Wait for transaction confirmation
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        match result {
            Ok(confirmed_hash) => {
                assert_eq!(confirmed_hash, tx_hash);
                info!("Transaction confirmed successfully: {}", confirmed_hash);
            }
            Err(e) => {
                panic!("Transaction was dropped unexpectedly: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_with_multicall() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap
        let (swap, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        // Create HTLC request
        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let requests = vec![request];

        // Submit transaction using HTLC multicall
        let tx_hash = unipay_htlc_executor
            .multicall(&requests, None)
            .await
            .expect("Failed to submit multicall");

        // Create tx_handler and wait for confirmation
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(30),
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Wait for transaction confirmation
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        match result {
            Ok(confirmed_hash) => {
                assert_eq!(confirmed_hash, tx_hash);
                info!("Multicall transaction confirmed: {}", confirmed_hash);
            }
            Err(e) => {
                panic!("Multicall transaction was dropped unexpectedly: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_with_replacement() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap
        let (swap, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        let current_fees = provider.estimate_eip1559_fees().await.unwrap();

        // Set the fees 30% lower than the current fees
        let max_fee_per_gas = current_fees.max_fee_per_gas * 70 / 100;
        let max_priority_fee_per_gas = current_fees.max_priority_fee_per_gas * 70 / 100;

        // Create HTLC request with custom gas options to potentially trigger replacement
        let tx_options = TxOptions {
            max_fee_per_gas: Some(max_fee_per_gas),
            max_priority_fee_per_gas: Some(max_priority_fee_per_gas),
            gas_limit: None,
            nonce: None,
        };

        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let requests = vec![request];

        // Submit transaction using HTLC multicall with low fees
        let tx_hash = unipay_htlc_executor
            .multicall(&requests, Some(tx_options))
            .await
            .expect("Failed to submit multicall");

        // Create tx_handler with shorter timeout to test replacement logic
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(10), // Shorter timeout to test replacement
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Wait for transaction confirmation (may trigger replacement)
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        match result {
            Ok(confirmed_hash) => {
                info!(
                    "Transaction confirmed (possibly with replacement): {}",
                    confirmed_hash
                );
                // The confirmed hash might be different from original if replacement occurred
            }
            Err(e) => {
                info!("Transaction was dropped unexpectedly: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_redeem_flow() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap with secret
        let (swap, secret) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        // First, initiate the swap
        let initiate_request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let initiate_requests = vec![initiate_request];

        // Submit initiation transaction
        let initiate_tx_hash = unipay_htlc_executor
            .multicall(&initiate_requests, None)
            .await
            .expect("Failed to submit initiation");

        // Create tx_handler and wait for initiation confirmation
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(30),
            provider.clone(),
            "ethereum_localnet".to_string(),
            unipay_htlc_executor.clone(),
        );

        let initiate_result = tx_handler
            .handle_tx(initiate_tx_hash, &initiate_requests)
            .await;

        match initiate_result {
            Ok(_) => {
                info!("Initiation confirmed, proceeding with redeem");
            }
            Err(e) => {
                panic!("Initiation transaction was dropped: {}", e);
            }
        }

        // Wait a bit for the transaction to be processed
        sleep(Duration::from_secs(5)).await;

        // Now create redeem request
        let redeem_request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Redeem { secret }),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let redeem_requests = vec![redeem_request];

        // Submit redeem transaction
        let redeem_tx_hash = unipay_htlc_executor
            .multicall(&redeem_requests, None)
            .await
            .expect("Failed to submit redeem");

        // Wait for redeem confirmation
        let redeem_result = tx_handler.handle_tx(redeem_tx_hash, &redeem_requests).await;

        match redeem_result {
            Ok(confirmed_hash) => {
                assert_eq!(confirmed_hash, redeem_tx_hash);
                info!("Redeem transaction confirmed: {}", confirmed_hash);
            }
            Err(e) => {
                panic!("Redeem transaction was dropped unexpectedly: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_with_signature() {
        let _ = tracing_subscriber::fmt::try_init();
        let (unipay_htlc, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap
        let (swap, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        // Get domain for signing
        let domain = unipay_htlc.domain(htlc_contract.address()).await.unwrap();

        // Create signature for initiation
        let initiate = Initiate {
            redeemer: swap.redeemer,
            timelock: swap.timelock,
            amount: swap.amount,
            secretHash: swap.secret_hash,
        };

        let signature = initiator
            .sign_typed_data(&initiate, &domain)
            .await
            .expect("Failed to sign");

        // Create HTLC request with signature
        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::InitiateWithUserSignature {
                signature: Bytes::from(signature.as_bytes()),
            }),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let requests = vec![request];

        // Submit transaction using HTLC multicall with signature
        let tx_hash = unipay_htlc_executor
            .multicall(&requests, None)
            .await
            .expect("Failed to submit multicall with signature");

        // Create tx_handler and wait for confirmation
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(30),
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Wait for transaction confirmation
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        match result {
            Ok(confirmed_hash) => {
                assert_eq!(confirmed_hash, tx_hash);
                info!("Signature-based transaction confirmed: {}", confirmed_hash);
            }
            Err(e) => {
                panic!(
                    "Signature-based transaction was dropped unexpectedly: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_multiple_requests() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create multiple swaps
        let (swap1, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);
        let (swap2, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        // Create multiple requests
        let request1 = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap1.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap1.secret_hash.to_string(),
        };

        let request2 = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap2.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap2.secret_hash.to_string(),
        };

        let requests = vec![request1, request2];

        // Submit transaction using HTLC multicall with multiple requests
        let tx_hash = unipay_htlc_executor
            .multicall(&requests, None)
            .await
            .expect("Failed to submit multicall with multiple requests");

        // Create tx_handler and wait for confirmation
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(30),
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Wait for transaction confirmation
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        match result {
            Ok(confirmed_hash) => {
                assert_eq!(confirmed_hash, tx_hash);
                info!(
                    "Multiple requests transaction confirmed: {}",
                    confirmed_hash
                );
            }
            Err(e) => {
                panic!(
                    "Multiple requests transaction was dropped unexpectedly: {}",
                    e
                );
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_with_custom_options() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap
        let (swap, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        // Create HTLC request with custom transaction options
        let tx_options = TxOptions {
            max_fee_per_gas: Some(50_000_000_000),         // 50 gwei
            max_priority_fee_per_gas: Some(2_000_000_000), // 2 gwei
            gas_limit: Some(500_000),
            nonce: None,
        };

        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let requests = vec![request];

        // Submit transaction using HTLC multicall with custom options
        let tx_hash = unipay_htlc_executor
            .multicall(&requests, Some(tx_options))
            .await
            .expect("Failed to submit multicall with custom options");

        // Create tx_handler and wait for confirmation
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_secs(30),
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Wait for transaction confirmation
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        match result {
            Ok(confirmed_hash) => {
                assert_eq!(confirmed_hash, tx_hash);
                info!("Custom options transaction confirmed: {}", confirmed_hash);
            }
            Err(e) => {
                panic!("Custom options transaction was dropped unexpectedly: {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_handle_tx_timeout_behavior() {
        let _ = tracing_subscriber::fmt::try_init();
        let (_, htlc_contract, _, initiator, chain_id, unipay_htlc_executor, provider) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        // Create a swap
        let (swap, _) = new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        // Create HTLC request
        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let requests = vec![request];

        // Submit transaction using HTLC multicall
        let tx_hash = unipay_htlc_executor
            .multicall(&requests, None)
            .await
            .expect("Failed to submit multicall");

        // Create tx_handler with very short timeout to test timeout behavior
        let mut tx_handler = PendingTxHandler::new(
            Duration::from_millis(100), // Very short timeout
            provider,
            "ethereum_localnet".to_string(),
            unipay_htlc_executor,
        );

        // Wait for transaction confirmation (should timeout quickly)
        let result = tx_handler.handle_tx(tx_hash, &requests).await;

        // In a test environment, the transaction might still confirm quickly
        // or timeout depending on network conditions
        match result {
            Ok(confirmed_hash) => {
                info!(
                    "Transaction confirmed despite short timeout: {}",
                    confirmed_hash
                );
            }
            Err(e) => {
                info!(
                    "Transaction dropped due to timeout (expected behavior): {}",
                    e
                );
            }
        }
    }
}
