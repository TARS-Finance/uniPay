use crate::{
    executor::block_numbers::{BlockNumbers, get_block_numbers, is_arbitrum_chain},
    orders::PendingOrdersProvider,
    settings::ChainSettings,
};
use alloy::{
    primitives::{Address, FixedBytes},
    providers::Provider,
};
use bon::Builder;
use eyre::{Result, bail};
use moka::future::Cache;
use std::{str::FromStr, sync::Arc};
use tars::{
    evm::{
        executor::UnipayActionExecutor,
        primitives::{
            AlloyProvider, UnipayActionRequest, UnipayActionType, SimulationResult, TxOptions,
        },
        tx_handler::PendingTxHandler,
    },
    orderbook::{OrderMapper, primitives::MatchedOrderVerbose},
    primitives::HTLCAction,
};
use tracing::info;
pub mod block_numbers;

// Time to sleep when there are no pending orders
const IDLE_SLEEP_TIME: u64 = 5000;

// Maximum number of requests in single multicall
const BATCH_LIMIT: usize = 64;

#[derive(Builder)]
// Executor for a specific chain
pub struct Executor {
    polling_interval: u64,
    orders_provider: PendingOrdersProvider,
    order_mapper: OrderMapper,
    actions_executor: UnipayActionExecutor,
    provider: AlloyProvider,
    settings: ChainSettings,
    cache: Arc<Cache<String, bool>>,
    pending_tx_handler: PendingTxHandler<UnipayActionRequest, UnipayActionExecutor>,
    signer_addr: String,
}

impl Executor {
    /// Runs the executor
    ///
    /// PIPELINE
    /// 1. Get pending orders
    /// 2. Map orders to actions
    /// 3. Filter cached requests
    /// 4. Execute dry run
    /// 5. Execute multicall
    /// 6. Wait for transaction
    /// 7. Update cache
    /// 8. Sleep
    ///
    /// # Arguments
    /// * `self` - The executor to run
    ///
    /// # Returns
    /// * `()` - The executor runs indefinitely
    pub async fn run(&mut self) {
        tracing::info!(
            chain = %self.settings.chain_identifier,
            "starting executor"
        );
        loop {
            let pending_orders = match self
                .orders_provider
                .get_pending_orders(&self.settings.chain_identifier)
                .await
            {
                Ok(orders) => orders,
                Err(e) => {
                    tracing::error!(
                        chain = %self.settings.chain_identifier,
                        error = %e,
                        "failed to get pending orders"
                    );
                    sleep_idle().await;
                    continue;
                }
            };

            if pending_orders.is_empty() {
                sleep_idle().await;
                continue;
            }

            info!(
                chain = %self.settings.chain_identifier,
                order_count = pending_orders.len(),
                "received pending orders"
            );

            // Check if there are any refund eligible swaps
            let has_refundable_swaps = self.order_mapper.has_refundable_swaps(&pending_orders);

            let block_numbers = if has_refundable_swaps {
                // Get block numbers for refundable swaps
                get_block_numbers(&self.provider, &self.settings.chain_identifier).await
            } else {
                // No refundable swaps, so no block numbers are needed
                None
            };

            let requests = self
                .prepare_requests(pending_orders, block_numbers.as_ref())
                .await;

            if requests.is_empty() {
                sleep_idle().await;
                continue;
            }

            for request_chuck in requests.chunks(BATCH_LIMIT) {
                let requests = request_chuck.to_vec();
                let valid_requests = match self.dry_run_and_filter(requests).await {
                    Some(requests) => requests,
                    None => {
                        sleep_idle().await;
                        continue;
                    }
                };

                match self.submit_requests(&valid_requests).await {
                    Ok(()) => { /* Success, cache already updated */ }
                    Err(e) => {
                        tracing::error!(
                            chain = %self.settings.chain_identifier,
                            request_count = valid_requests.len(),
                            error = %e,
                            "failed to submit batch transaction"
                        );
                        sleep_idle().await;
                        continue;
                    }
                }
            }

            tokio::time::sleep(tokio::time::Duration::from_millis(self.polling_interval)).await;
        }
    }

    /// Prepares HTLC requests to be executed from the given pending orders.
    ///
    /// This takes a list of pending orders and maps each one to an HTLC action using the
    /// `OrderMapper`. It then filters out any requests that have already been cached, and returns
    /// the remaining requests as a vector of `HTLCRequest`.
    ///
    /// # Arguments
    ///
    /// * `pending_orders` - The list of pending orders to prepare requests for.
    /// * `latest_block` - The latest block number.
    ///
    /// # Returns
    ///
    /// A vector of `HTLCRequest`.
    ///
    /// # Errors
    ///
    /// Returns an error if the order mapping fails.
    async fn prepare_requests(
        &self,
        pending_orders: Vec<MatchedOrderVerbose>,
        block_numbers: Option<&BlockNumbers>,
    ) -> Vec<UnipayActionRequest> {
        let mut requests = Vec::with_capacity(pending_orders.len());

        for order in pending_orders {
            let order_id = &order.create_order.create_id;

            let htlc_version = &order.create_order.additional_data.version;

            let block_number = block_numbers.and_then(|blocks| {
                blocks.get_block_for_version(
                    htlc_version,
                    is_arbitrum_chain(&self.settings.chain_identifier),
                )
            });

            let action_info = match self.order_mapper.map(&order, block_number).await {
                Ok(action_info) => {
                    info!(order_id = %order_id,action = %action_info.action,chain = %self.settings.chain_identifier, "Mapped order to action");
                    action_info
                }
                Err(e) => {
                    tracing::error!(
                        order_id = %order_id,
                        error = %e,
                        "failed to map order to action"
                    );
                    continue;
                }
            };

            match (&action_info.action, &action_info.swap) {
                (HTLCAction::NoOp, _) => {
                    // Skip NoOp actions
                    continue;
                }
                (_, None) => {
                    tracing::error!(
                        order_id = %order_id,
                        action = ?action_info.action,
                        "Non-NoOp action missing swap data"
                    );
                    continue;
                }
                (_, Some(swap)) => {
                    // Valid action with swap, continue processing
                    let evm_swap = match swap.get_evm_swap() {
                        Ok(evm_swap) => evm_swap,
                        Err(e) => {
                            tracing::error!(
                                order_id = %order_id,
                                error = %e,
                                "failed to parse swap data"
                            );
                            continue;
                        }
                    };

                    requests.push(UnipayActionRequest {
                        id: order_id.to_string(),
                        action: UnipayActionType::HTLC(action_info.action),
                        swap: evm_swap,
                        asset: swap.asset.clone(),
                    });
                }
            }
        }

        self.filter_cached_requests(requests).await
    }

    /// Executes a dry run of the given requests and filters out any that fail with errors other than "Empty error data"
    ///
    /// # Arguments
    /// * `requests` - The HTLC requests to dry run
    ///
    /// # Returns
    /// * `Option<Vec<UnipayActionRequest>>` - A vector containing the valid requests.
    ///   Returns `None` if there are no valid requests or if the dry run fails.
    async fn dry_run_and_filter(
        &self,
        requests: Vec<UnipayActionRequest>,
    ) -> Option<Vec<UnipayActionRequest>> {
        let chain_identifier = self.settings.chain_identifier.clone();
        let results = match self.actions_executor.actions_dry_run(&requests).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(chain = %chain_identifier, request_count = requests.len(), error = %e, "failed to execute dry run");
                return None;
            }
        };

        let mut valid_requests = Vec::with_capacity(requests.len());

        for (req, res) in requests.into_iter().zip(results) {
            match res {
                SimulationResult::Success => {
                    valid_requests.push(req);
                }
                SimulationResult::Error(e) => {
                    tracing::warn!(
                        order_id = req.id,
                        action = ?req.action,
                        error = %e,
                        chain = %chain_identifier,
                        "dry run failed"
                    );
                }
            }
        }

        if valid_requests.is_empty() {
            tracing::info!(chain = %chain_identifier, "no valid requests found");
            return None;
        }

        Some(valid_requests)
    }

    /// Returns a unique key for a given order id and action
    ///
    /// # Arguments
    /// * `chain_identifier` - The chain identifier
    /// * `order_id` - The order id
    /// * `action` - The action to be performed on the HTLC
    ///
    #[inline]
    fn get_cache_key(chain_identifier: &str, order_id: &str, action: &UnipayActionType) -> String {
        format!("{}-{}-{}", chain_identifier, order_id, action)
    }

    /// Submits the given requests and updates the cache on success
    ///
    /// # Arguments
    /// * `requests` - The requests to submit
    ///
    /// # Returns
    /// * `Result<()>` - Ok(()) if submission was successful, Err otherwise
    async fn submit_requests(&mut self, requests: &[UnipayActionRequest]) -> Result<()> {
        let tx_hash = self.submi_transaction(requests).await?;
        match self.pending_tx_handler.handle_tx(tx_hash, requests).await {
            Ok(tx_hash) => {
                self.update_cache(requests).await;
                info!("Submitted HTLC requests with tx hash: {}", tx_hash);
                return Ok(());
            }
            Err(e) => {
                bail!("transaction dropped or reverted: {}", e);
            }
        }
    }

    /// Submits the initial multicall transaction
    async fn submi_transaction(
        &mut self,
        requests: &[UnipayActionRequest],
    ) -> Result<FixedBytes<32>> {
        let addr =
            Address::from_str(&self.signer_addr).map_err(|_| eyre::eyre!("invalid signer addr"))?;

        let account_nonce = self.provider.get_transaction_count(addr).await?;
        let fees = self.provider.estimate_eip1559_fees().await?;

        let mut tx_opts = TxOptions {
            nonce: Some(account_nonce),
            gas_limit: None,
            max_fee_per_gas: Some(fees.max_fee_per_gas),
            max_priority_fee_per_gas: Some(fees.max_priority_fee_per_gas),
        };

        let transaction_hash = match self
            .actions_executor
            .multicall(requests, Some(tx_opts.clone()))
            .await
        {
            Ok(hash) => hash,
            Err(err) => {
                if err
                    .to_string()
                    .contains("replacement transaction underpriced")
                {
                    tx_opts.max_fee_per_gas = Some(fees.max_fee_per_gas * 3);
                    tx_opts.max_priority_fee_per_gas = Some(fees.max_priority_fee_per_gas * 3);
                    self.actions_executor
                        .multicall(requests, Some(tx_opts))
                        .await?
                } else {
                    return Err(err.into());
                }
            }
        };

        info!(
            chain = %self.settings.chain_identifier,
            request_count = requests.len(),
            transaction_hash = %transaction_hash,
            "initial multicall transaction submitted"
        );

        Ok(transaction_hash)
    }

    /// Updates the cache as true for the given requests
    ///
    /// # Arguments
    /// * `valid_requests` - The requests to update the cache with
    ///
    #[inline]
    async fn update_cache(&self, valid_requests: &[UnipayActionRequest]) {
        for request in valid_requests {
            let key = Self::get_cache_key(
                &self.settings.chain_identifier,
                &request.id,
                &request.action,
            );
            self.cache.insert(key, true).await;
        }
    }

    /// Filters out requests that have already been executed
    ///
    /// # Arguments
    /// * `requests` - The requests to filter
    ///
    /// # Returns
    /// * `Vec<HTLCRequest>` - The filtered requests
    async fn filter_cached_requests(
        &self,
        requests: Vec<UnipayActionRequest>,
    ) -> Vec<UnipayActionRequest> {
        let input_count = requests.len();
        let mut filtered = Vec::with_capacity(input_count);

        for request in requests.into_iter() {
            let key = Self::get_cache_key(
                &self.settings.chain_identifier,
                &request.id,
                &request.action,
            );
            if self.cache.get(&key).await.is_none() {
                filtered.push(request);
            }
        }

        info!(
            chain = %self.settings.chain_identifier,
            input_count,
            output_count = filtered.len(),
            "filtered out cached requests"
        );

        filtered
    }
}

/// Sleep for a particular amount of time
///
/// This function is used to sleep for a particular amount of time between actions.
#[inline]
async fn sleep_idle() {
    tokio::time::sleep(tokio::time::Duration::from_millis(IDLE_SLEEP_TIME)).await;
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, sync::Arc, time::Duration};

    use alloy::{hex::FromHex, primitives::Address};
    use bigdecimal::BigDecimal;
    use moka::future::Cache;
    use reqwest::{StatusCode, Url};
    use tars::{
        api::primitives::{Response, Status},
        evm::{
            Multicall3::Multicall3Instance, executor::UnipayActionExecutor,
            primitives::UnipayActionType, test_utils as evm_test_utils,
            tx_handler::PendingTxHandler,
        },
        fiat::{FiatPriceResult, FiatProvider},
        orderbook::{
            OrderMapper,
            primitives::{MatchedOrderVerbose, MaybeString},
        },
        primitives::HTLCAction,
    };
    use wiremock::MockServer;

    use crate::{executor::Executor, orders::PendingOrdersProvider, settings::ChainSettings};
    use tars::orderbook::test_utils as orderbook_test_utils;
    use wiremock::{
        Mock, ResponseTemplate,
        matchers::{method, path},
    };

    const POLLING_INTERVAL: u64 = 1000;
    const MULTICALL_ADDRESS: &str = "0x2279B7A0a67DB372996a5FaB50D91eAA73d2eBe6";
    const TRANSACTION_TIMEOUT: u64 = 60000;
    const ORDER_PROVIDER_URL: &str = "http://127.0.0.1:4596";
    const DESTINATION_CHAIN: &str = "ethereum_localnet";
    const SOURCE_CHAIN: &str = "bitcoin_regtest";
    const ASSET: &str = "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0";

    fn repeated_hex(byte: u8, len_bytes: usize) -> String {
        format!("0x{}", format!("{byte:02x}").repeat(len_bytes))
    }

    async fn start_mock_fiat_server() -> MockServer {
        let mock_server = MockServer::start().await;

        let res = Response {
            status: Status::Ok,
            result: Some(FiatPriceResult {
                input_token_price: 1.0,
                output_token_price: 1.0,
            }),
            error: None,
            status_code: StatusCode::OK,
        };

        Mock::given(method("GET"))
            .and(path("/fiat"))
            .respond_with(ResponseTemplate::new(200).set_body_json(res.clone()))
            .mount(&mock_server)
            .await;

        mock_server
    }

    async fn build_test_executor() -> (Executor, MockServer) {
        let fiat_server = start_mock_fiat_server().await;
        let fiat_provider = FiatProvider::new(&fiat_server.uri(), None).unwrap();
        let order_mapper = OrderMapper::builder(fiat_provider)
            .add_supported_chain(DESTINATION_CHAIN.to_string())
            .add_supported_chain(SOURCE_CHAIN.to_string())
            .build();

        let provider = evm_test_utils::ethereum_provider(None);
        let multicall_address = Address::from_hex(MULTICALL_ADDRESS).unwrap();
        let multicall_contract =
            Arc::new(Multicall3Instance::new(multicall_address, provider.clone()));
        let actions_executor = UnipayActionExecutor::new(multicall_contract, HashMap::new());

        let orders_provider = PendingOrdersProvider::new(Url::parse(ORDER_PROVIDER_URL).unwrap());

        let cache = Arc::new(Cache::builder().build());

        let pending_tx_handler = PendingTxHandler::new(
            Duration::from_millis(TRANSACTION_TIMEOUT),
            provider.clone(),
            DESTINATION_CHAIN.to_string(),
            actions_executor.clone(),
        );

        let executor = Executor::builder()
            .polling_interval(POLLING_INTERVAL)
            .order_mapper(order_mapper)
            .actions_executor(actions_executor)
            .provider(provider)
            .orders_provider(orders_provider)
            .cache(cache)
            .settings(ChainSettings {
                chain_identifier: DESTINATION_CHAIN.to_string(),
                rpc_url: "http://localhost:8545".to_string(),
                multicall_address: MULTICALL_ADDRESS.to_string(),
                polling_interval: POLLING_INTERVAL,
                transaction_timeout: TRANSACTION_TIMEOUT,
            })
            .signer_addr(repeated_hex(0x11, 20))
            .pending_tx_handler(pending_tx_handler)
            .build();

        (executor, fiat_server)
    }

    fn initiate_ready_order(id: &str, seed: u8) -> MatchedOrderVerbose {
        let mut order = orderbook_test_utils::default_matched_order();
        let source_amount = BigDecimal::from(1_000u32);
        let destination_amount = BigDecimal::from(1_000u32);
        let secret_hash = repeated_hex(seed.wrapping_add(1), 32);

        order.create_order.create_id = id.to_string();
        order.create_order.source_chain = SOURCE_CHAIN.to_string();
        order.create_order.destination_chain = DESTINATION_CHAIN.to_string();
        order.create_order.source_asset = "primary".to_string();
        order.create_order.destination_asset = ASSET.to_string();
        order.create_order.source_amount = source_amount.clone();
        order.create_order.destination_amount = destination_amount.clone();
        order.create_order.secret_hash = secret_hash.clone();
        order.create_order.additional_data.deadline = chrono::Utc::now().timestamp() + 3600;
        order.create_order.additional_data.input_token_price = 1.0;
        order.create_order.additional_data.output_token_price = 1.0;
        order.create_order.additional_data.is_blacklisted = false;

        order.source_swap.swap_id = repeated_hex(seed.wrapping_add(2), 32);
        order.source_swap.chain = SOURCE_CHAIN.to_string();
        order.source_swap.asset = "primary".to_string();
        order.source_swap.amount = source_amount.clone();
        order.source_swap.filled_amount = source_amount;
        order.source_swap.secret_hash = secret_hash.clone();
        order.source_swap.initiate_tx_hash =
            MaybeString::new(repeated_hex(seed.wrapping_add(3), 32));
        order.source_swap.initiate_block_number = Some(BigDecimal::from(1u32));
        order.source_swap.required_confirmations = 1;
        order.source_swap.current_confirmations = 1;

        order.destination_swap.swap_id = repeated_hex(seed.wrapping_add(4), 32);
        order.destination_swap.chain = DESTINATION_CHAIN.to_string();
        order.destination_swap.asset = ASSET.to_string();
        order.destination_swap.amount = destination_amount;
        order.destination_swap.secret_hash = secret_hash;
        order.destination_swap.initiator = repeated_hex(seed.wrapping_add(5), 20);
        order.destination_swap.redeemer = repeated_hex(seed.wrapping_add(6), 20);
        order.destination_swap.initiate_tx_hash = MaybeString::new(String::new());
        order.destination_swap.redeem_tx_hash = MaybeString::new(String::new());
        order.destination_swap.refund_tx_hash = MaybeString::new(String::new());
        order.destination_swap.initiate_block_number = None;
        order.destination_swap.token_address = Some(ASSET.to_string());

        order
    }

    #[tokio::test]
    async fn test_executor_initiates() {
        let (executor, _fiat_server) = build_test_executor().await;
        let order = initiate_ready_order("order-1", 0x01);

        let requests = executor.prepare_requests(vec![order], None).await;

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].id, "order-1");
        assert_eq!(requests[0].asset, ASSET);
        assert_eq!(
            requests[0].action,
            UnipayActionType::HTLC(HTLCAction::Initiate)
        );
    }

    #[tokio::test]
    async fn test_executor_batch() {
        let (executor, _fiat_server) = build_test_executor().await;
        let mut orders = Vec::with_capacity(10);
        for seed in 0..10u8 {
            orders.push(initiate_ready_order(&format!("order-{seed}"), seed));
        }

        let requests = executor.prepare_requests(orders, None).await;
        executor.update_cache(&requests[..3]).await;

        let filtered = executor.filter_cached_requests(requests).await;

        assert_eq!(filtered.len(), 7);
        assert!(filtered.iter().all(|request| {
            request.id != "order-0" && request.id != "order-1" && request.id != "order-2"
        }));
    }
}
