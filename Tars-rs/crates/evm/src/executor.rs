use std::{collections::HashMap, sync::Arc};

use alloy::{
    hex::FromHex,
    primitives::{Address, FixedBytes},
    providers::Provider,
};

use crate::{
    errors::{sanitize_error, EvmError, HTLCError, MulticallError},
    multicall::Multicall,
    primitives::{
        UnipayActionRequest, UnipayHandlerType, RequestCallBatch, SimulationResult, TxOptions,
    },
    traits::UnipayActionHandler,
    tx_handler::traits::TransactionSubmitter,
    Multicall3Contract,
};

#[derive(Clone)]
pub struct UnipayActionExecutor {
    multicall_contract: Arc<Multicall3Contract>,
    handlers: HashMap<UnipayHandlerType, Arc<dyn UnipayActionHandler>>,
}

impl UnipayActionExecutor {
    pub fn new(
        multicall_contract: Arc<Multicall3Contract>,
        handlers: HashMap<UnipayHandlerType, Arc<dyn UnipayActionHandler>>,
    ) -> Self {
        Self {
            handlers,
            multicall_contract,
        }
    }

    /// Prepares a multicall batch for a set of ActionRequests.
    ///
    /// This function validates each request, generates calldata, and builds a multicall batch.
    /// For each request, it records errors immediately if validation or calldata generation fails.
    /// Valid requests are added to the multicall batch and tracked with their call indices.
    ///
    /// # Arguments
    ///
    /// * `requests` - A slice of `ActionRequest` structs to be simulated.
    ///
    /// # Returns
    ///
    /// A tuple containing:
    /// - A vector of `SimulationResult` (with errors for invalid requests)
    /// - The constructed `Multicall` object for valid requests
    /// - A vector of `RequestCallBatch` describing the mapping from requests to multicall calls
    ///
    /// # Errors
    ///
    /// Returns `ContractError` if multicall construction fails.
    async fn prepare_multicall(
        &self,
        requests: &[UnipayActionRequest],
    ) -> Result<(Vec<SimulationResult>, Multicall, Vec<RequestCallBatch>), MulticallError> {
        // Initialize all results - will be populated with errors or marked as successful
        let mut results = vec![SimulationResult::default(); requests.len()];
        let mut multicall_builder = Multicall::builder(self.multicall_contract.clone());
        let mut valid_request_batches: Vec<RequestCallBatch> = Vec::with_capacity(requests.len());
        let mut total_calls = 0;

        // Validate all requests and populate errors immediately
        for (request_index, request) in requests.iter().enumerate() {
            // Validate swap parameters
            if let Err(e) = request.swap.validate() {
                results[request_index] =
                    SimulationResult::error(format!("Invalid swap parameters: {}", e));
                continue;
            }

            // Validate and parse asset address
            let asset = match Address::from_hex(&request.asset) {
                Ok(addr) => addr,
                Err(e) => {
                    results[request_index] =
                        SimulationResult::error(format!("Invalid asset address: {}", e));
                    continue;
                }
            };

            let handler = self
                .handlers
                .get(&UnipayHandlerType::from(&request.action))
                .ok_or(MulticallError::Error(
                    "No handler found for action".to_string(),
                ))?;

            let calls_res = handler
                .get_calldata(&request.action, &request.swap, &asset)
                .await;

            let calls = match calls_res {
                Ok(calls) => calls,
                Err(e) => {
                    results[request_index] =
                        SimulationResult::error(format!("Failed to generate calldata: {}", e));
                    continue;
                }
            };

            if calls.is_empty() {
                results[request_index] =
                    SimulationResult::error("No calls generated for request".to_string());
                continue;
            }

            // This request is valid - add to multicall batch
            let call_batch = RequestCallBatch {
                request_index,
                call_start_index: total_calls,
                call_count: calls.len(),
            };

            // Add all calls to multicall
            for call in calls {
                multicall_builder.add_call(call, true);
                total_calls += 1;
            }

            valid_request_batches.push(call_batch);
        }

        let multicall = multicall_builder.build();
        Ok((results, multicall, valid_request_batches))
    }

    /// Executes a multicall dry run and processes the results for each request.
    ///
    /// This function takes the prepared multicall and request batches, executes the multicall simulation,
    /// and updates the results vector with success or error for each request.
    ///
    /// # Arguments
    ///
    /// * `results` - The initial results vector (with errors for invalid requests).
    /// * `multicall` - The prepared `Multicall` object for valid requests.
    /// * `valid_request_batches` - The mapping from requests to multicall call indices.
    ///
    /// # Returns
    ///
    /// A vector of `SimulationResult` with updated success/error for each request.
    ///
    /// # Errors
    ///
    /// Returns `ContractError` if the multicall simulation fails.
    async fn process_multicall_dry_run(
        &self,
        mut results: Vec<SimulationResult>,
        multicall: Multicall,
        valid_request_batches: Vec<RequestCallBatch>,
    ) -> Result<Vec<SimulationResult>, MulticallError> {
        if !valid_request_batches.is_empty() {
            let multicall_results = multicall
                .call()
                .await
                .map_err(|e| MulticallError::Error(sanitize_error(e.to_string())))?;

            // Process multicall results for each valid request
            for batch in valid_request_batches {
                let request_results = &multicall_results
                    [batch.call_start_index..batch.call_start_index + batch.call_count];

                let mut success = true;

                // Check each call result in sequence - stop at first failure
                for (_call_index, call_result) in request_results.iter().enumerate() {
                    if !call_result.success {
                        success = false;
                        results[batch.request_index] =
                            SimulationResult::error_bytes(&call_result.return_data);
                        break;
                    }
                }

                // If we haven't set an error, all calls succeeded
                if success {
                    results[batch.request_index] = SimulationResult::success();
                }
            }
        }

        Ok(results)
    }

    /// Simulates multiple actions without executing actual transactions
    ///
    /// This function performs a dry run of multiple ActionRequests by validating and simulating
    /// each request in the sequence. It processes the requests in the following order:
    ///
    /// 1. Validates all swap parameters for each request
    /// 2. Decodes and validates asset addresses  
    /// 3. Generates appropriate calldata for valid requests
    /// 4. Performs a multicall simulation for all valid requests
    ///
    /// The function maintains the original order of results. Failed validations will result in a
    /// `SimulationResult` with `success = false` and appropriate error message.
    ///
    /// # Arguments
    ///
    /// * `requests` - A slice of `ActionRequest` structs containing the HTLC actions to simulate.
    ///
    /// # Returns
    ///
    /// * `Result<Vec<SimulationResult>, ContractError>` - A vector of `SimulationResult` structs containing
    ///   the simulation results for each request. Each result includes:
    ///   - `success`: boolean indicating if the simulation was successful
    ///   - `error`: optional error message with decoded return data if simulation failed
    ///
    /// # Errors
    ///
    /// Returns `ContractError` if:
    /// - Multicall simulation fails catastrophically
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let requests = vec![
    ///     ActionRequest { action: ActionType::HTLC(HTLCAction::Initiate { signature }), swap: swap1, asset: "0x..." },
    ///     ActionRequest { action: ActionType::HTLC(HTLCAction::Redeem { secret }), swap: swap2, asset: "0x..." },
    /// ];
    ///
    /// let simulation_results = htlc.actions_dry_run(&requests).await?;
    /// for result in simulation_results {
    ///     if result.success {
    ///         println!("Request succeeded");
    ///     } else {
    ///         println!("Request failed: {}", result.error.unwrap_or_default());
    ///     }
    /// }
    /// ```
    pub async fn actions_dry_run(
        &self,
        requests: &[UnipayActionRequest],
    ) -> Result<Vec<SimulationResult>, MulticallError> {
        if requests.is_empty() {
            return Ok(Vec::new());
        }

        let (results, multicall, valid_request_batches) = self.prepare_multicall(requests).await?;

        self.process_multicall_dry_run(results, multicall, valid_request_batches)
            .await
    }

    /// Executes multiple actions in a single transaction using Multicall3.
    ///
    /// # Arguments
    ///
    /// * `requests` - Vector of ActionRequests to execute.
    /// * `options` - Optional transaction options.
    ///
    /// # Returns
    ///
    /// Transaction hash as a `FixedBytes<32>` if successful.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let requests = vec![
    ///     ActionRequest { action: ActionType::HTLC(HTLCAction::Initiate { signature }), swap: swap1, asset1 },
    ///     ActionRequest { action: ActionType::HTLC(HTLCAction::Redeem { secret }), swap: swap2, asset2 },
    /// ];
    /// let tx_hash = htlc.multicall(&requests).await?;
    /// ```
    pub async fn multicall(
        &self,
        requests: &[UnipayActionRequest],
        options: Option<TxOptions>,
    ) -> Result<FixedBytes<32>, MulticallError> {
        if requests.is_empty() {
            return Err(MulticallError::Error("No requests provided".to_string()));
        }

        let mut multicall_builder = Multicall::builder(self.multicall_contract.clone());

        for request in requests {
            let asset = Address::from_hex(&request.asset)
                .map_err(|e| HTLCError::EvmError(EvmError::DecodeAddressError(e)))?;

            request
                .swap
                .validate()
                .map_err(|e| HTLCError::SimulationFailed {
                    action: "validate".to_string(),
                    reason: e.to_string(),
                })?;

            let handler = self
                .handlers
                .get(&UnipayHandlerType::from(&request.action))
                .ok_or(MulticallError::Error(
                    "No handler found for action".to_string(),
                ))?;

            let calls = handler
                .get_calldata(&request.action, &request.swap, &asset)
                .await?;

            for call in calls {
                multicall_builder.add_call(call, false);
            }
        }

        // Manual fee calculation
        let (max_fee_per_gas, max_priority_fee_per_gas) = self
            .resolve_fee_options(
                options.as_ref().and_then(|o| o.max_fee_per_gas),
                options.as_ref().and_then(|o| o.max_priority_fee_per_gas),
            )
            .await?;

        multicall_builder = multicall_builder.with_max_fee_per_gas(max_fee_per_gas);
        multicall_builder =
            multicall_builder.with_max_priority_fee_per_gas(max_priority_fee_per_gas);

        // Apply other options if provided
        if let Some(options) = options {
            if let Some(gas_limit) = options.gas_limit {
                multicall_builder = multicall_builder.with_gas_limit(gas_limit);
            }

            if let Some(nonce) = options.nonce {
                multicall_builder = multicall_builder.with_nonce(nonce);
            }
        }

        let tx_hash = multicall_builder
            .build()
            .execute()
            .await
            .map_err(|e| MulticallError::Error(sanitize_error(e.to_string())))?;

        Ok(tx_hash)
    }

    /// Resolve fee options and return both max_fee_per_gas and max_priority_fee_per_gas
    async fn resolve_fee_options(
        &self,
        max_fee_per_gas: Option<u128>,
        max_priority_fee_per_gas: Option<u128>,
    ) -> Result<(u128, u128), MulticallError> {
        match (max_fee_per_gas, max_priority_fee_per_gas) {
            // Both provided - use as is
            (Some(max_fee), Some(priority_fee)) => Ok((max_fee, priority_fee)),

            // Neither provided - calculate both
            (None, None) => {
                let provider = self.multicall_contract.provider().clone();
                let fees = provider
                    .estimate_eip1559_fees()
                    .await
                    .map_err(|e| MulticallError::Error(sanitize_error(e.to_string())))?;
                Ok((fees.max_fee_per_gas, fees.max_priority_fee_per_gas))
            }

            // Only either max fee or priority fee provided - return error as we cannot get the intended fee
            _ => {
                return Err(MulticallError::Error(
                    "Expected both max fee per gas and priority fee per gas or neither".to_string(),
                ));
            }
        }
    }
}

#[async_trait::async_trait]
impl TransactionSubmitter<UnipayActionRequest> for UnipayActionExecutor {
    async fn submit_transaction(
        &mut self,
        requests: &[UnipayActionRequest],
        tx_options: TxOptions,
    ) -> eyre::Result<FixedBytes<32>> {
        self.multicall(requests, Some(tx_options))
            .await
            .map_err(|e| eyre::eyre!(e))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use alloy::{
        primitives::{Bytes, U256},
        signers::Signer,
    };
    use primitives::{HTLCAction, HTLCVersion};
    use tokio::time::sleep;

    use crate::{
        htlc::v1::Initiate,
        primitives::UnipayActionType,
        test_utils::{self, get_contracts},
    };

    use super::*;

    #[tokio::test]
    async fn test_prepare_multicall_and_execute_multicall_and_process_results() {
        use crate::primitives::UnipayActionRequest;
        use alloy::primitives::Bytes;

        let (chain_htlc, htlc_contract, _, initiator, chain_id, contract_wrapper, _) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();
        // Valid swap and request
        let (swap, _secret) =
            test_utils::new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);
        let domain = chain_htlc.domain(htlc_contract.address()).await.unwrap();
        let initiate = Initiate {
            redeemer: swap.redeemer,
            timelock: swap.timelock,
            amount: swap.amount,
            secretHash: swap.secret_hash,
        };
        let sig = initiator
            .sign_typed_data(&initiate, &domain)
            .await
            .expect("Failed to sign");

        let valid_request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::InitiateWithUserSignature {
                signature: Bytes::from(sig.as_bytes()),
            }),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        // Invalid request: bad asset address
        let invalid_request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: swap.clone(),
            asset: "not_an_address".to_string(),
            id: "bad_id".to_string(),
        };

        // Invalid request: zero amount
        let mut bad_swap = swap.clone();
        bad_swap.amount = U256::ZERO;
        let zero_amount_request = UnipayActionRequest {
            action: UnipayActionType::HTLC(HTLCAction::Initiate),
            swap: bad_swap,
            asset: htlc_contract.address().to_string(),
            id: "zero_amount".to_string(),
        };

        let requests = vec![valid_request, invalid_request, zero_amount_request];

        // Test prepare_multicall
        let (results, multicall, valid_batches) =
            contract_wrapper.prepare_multicall(&requests).await.unwrap();

        // Only the first request should be valid
        assert_eq!(valid_batches.len(), 1);
        assert_eq!(valid_batches[0].request_index, 0);

        // The invalid requests should have error results
        assert!(!results[1].is_success());
        assert!(!results[2].is_success());

        // Test execute_multicall_and_process_results
        let processed_results = contract_wrapper
            .process_multicall_dry_run(results, multicall, valid_batches)
            .await
            .unwrap();

        assert!(processed_results[0].is_success());
        assert!(!processed_results[1].is_success());
        assert!(!processed_results[2].is_success());
    }

    #[tokio::test]
    async fn test_multicall() {
        let (chain_htlc, htlc_contract, _, initiator, chain_id, contract_wrapper, _) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();
        let (swap, secret) =
            test_utils::new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);
        let domain = chain_htlc.domain(htlc_contract.address()).await.unwrap();

        let initiate = Initiate {
            redeemer: swap.redeemer,
            timelock: swap.timelock,
            amount: swap.amount,
            secretHash: swap.secret_hash,
        };

        let sig = initiator
            .sign_typed_data(&initiate, &domain)
            .await
            .expect("Failed to sign");

        let request1 = super::UnipayActionRequest {
            action: UnipayActionType::HTLC(primitives::HTLCAction::InitiateWithUserSignature {
                signature: Bytes::from(sig.as_bytes()),
            }),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let request2 = super::UnipayActionRequest {
            action: UnipayActionType::HTLC(primitives::HTLCAction::Redeem { secret }),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let txid = contract_wrapper
            .multicall(&[request1, request2], None)
            .await
            .unwrap();
        tracing::info!("Multicall transaction ID: {}", txid);
    }

    #[tokio::test]
    async fn test_intiate_with_sig_multicall() {
        let _ = tracing_subscriber::fmt::try_init();
        let (chain_htlc, htlc_contract, _, initiator, chain_id, contract_wrapper, _) =
            get_contracts(HTLCVersion::V3).await;
        let asset = htlc_contract.address();

        let (swap, _) =
            test_utils::new_swap(initiator.address(), chain_id, *asset, HTLCVersion::V3);

        let domain = chain_htlc.domain(htlc_contract.address()).await.unwrap();
        let initiate = Initiate {
            redeemer: swap.redeemer,
            timelock: swap.timelock,
            amount: swap.amount,
            secretHash: swap.secret_hash,
        };

        tracing::info!("Initiate struct: {:#?}", swap);

        let sig = initiator
            .sign_typed_data(&initiate, &domain)
            .await
            .expect("Failed to sign");

        let request = UnipayActionRequest {
            action: UnipayActionType::HTLC(primitives::HTLCAction::InitiateWithUserSignature {
                signature: Bytes::from(sig.as_bytes()),
            }),
            swap: swap.clone(),
            asset: htlc_contract.address().to_string(),
            id: swap.secret_hash.to_string(),
        };

        let txid = contract_wrapper.multicall(&[request], None).await.unwrap();
        tracing::info!("Multicall transaction ID: {}", txid);

        sleep(Duration::from_secs(10)).await;
        let order = chain_htlc
            .get_order(&swap, htlc_contract.address())
            .await
            .unwrap();
        assert!(!order.is_fulfilled);
        assert_eq!(order.initiator, initiator.address());
    }
}
