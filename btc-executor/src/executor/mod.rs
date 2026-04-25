mod execution_request;

use std::sync::Arc;
use std::time::Duration;

use bigdecimal::{BigDecimal, Zero};
use moka::future::Cache;
use tars::orderbook::primitives::{ActionWithInfo, MatchedOrderVerbose, SingleSwap};
use tars::orderbook::OrderMapper;
use tars::primitives::HTLCAction;

use crate::errors::ExecutorError;
use crate::infrastructure::chain::bitcoin::BitcoinActionExecutor;
use crate::orders::PendingOrdersProvider;

use self::execution_request::OrderExecutionRequest;

pub struct Executor {
    polling_interval_ms: u64,
    chain_identifier: String,
    orders_provider: PendingOrdersProvider,
    order_mapper: OrderMapper,
    action_executor: Arc<BitcoinActionExecutor>,
    cache: Arc<Cache<String, bool>>,
    signer_id: String,
}

impl Executor {
    pub fn new(
        polling_interval_ms: u64,
        chain_identifier: String,
        orders_provider: PendingOrdersProvider,
        order_mapper: OrderMapper,
        action_executor: Arc<BitcoinActionExecutor>,
        cache: Arc<Cache<String, bool>>,
    ) -> Self {
        let signer_id = action_executor.solver_id();
        Self {
            polling_interval_ms,
            chain_identifier,
            orders_provider,
            order_mapper,
            action_executor,
            cache,
            signer_id,
        }
    }

    pub async fn run(&mut self) {
        tracing::info!(
            chain = %self.chain_identifier,
            solver = %self.signer_id,
            address = %self.action_executor.solver_address(),
            "starting bitcoin executor",
        );

        loop {
            let pending_orders = match self
                .orders_provider
                .get_pending_orders(&self.chain_identifier, &self.signer_id)
                .await
            {
                Ok(orders) => orders,
                Err(error) => {
                    tracing::error!(
                        chain = %self.chain_identifier,
                        solver = %self.signer_id,
                        error = %error,
                        "failed to fetch pending orders",
                    );
                    sleep_for_poll_interval(self.polling_interval_ms).await;
                    continue;
                },
            };

            tracing::info!(
                chain = %self.chain_identifier,
                solver = %self.signer_id,
                pending = pending_orders.len(),
                "poll tick: fetched pending orders",
            );

            if pending_orders.is_empty() {
                sleep_for_poll_interval(self.polling_interval_ms).await;
                continue;
            }

            let requests = self.prepare_requests(pending_orders).await;
            tracing::info!(
                chain = %self.chain_identifier,
                requests = requests.len(),
                "poll tick: prepared requests",
            );
            if requests.is_empty() {
                sleep_for_poll_interval(self.polling_interval_ms).await;
                continue;
            }

            for request in requests {
                match self
                    .action_executor
                    .execute_action(&request.order, &request.action, &request.swap)
                    .await
                {
                    Ok(submitted_requests) => {
                        tracing::info!(
                            chain = %self.chain_identifier,
                            order_id = %request.order_id,
                            action = %request.action,
                            submitted_requests,
                            "submitted bitcoin wallet request(s)",
                        );
                        self.cache.insert(request.cache_key, true).await;
                    },
                    Err(error) => {
                        tracing::error!(
                            chain = %self.chain_identifier,
                            order_id = %request.order_id,
                            action = %request.action,
                            error = %error,
                            "failed to execute bitcoin action",
                        );
                    },
                }
            }

            tokio::time::sleep(Duration::from_millis(self.polling_interval_ms)).await;
        }
    }

    async fn prepare_requests(
        &self,
        pending_orders: Vec<MatchedOrderVerbose>,
    ) -> Vec<OrderExecutionRequest> {
        let latest_block = match self.latest_block_number(&pending_orders).await {
            Ok(block) => block,
            Err(error) => {
                tracing::error!(
                    chain = %self.chain_identifier,
                    error = %error,
                    "failed to fetch bitcoin block height for refund mapping",
                );
                return Vec::new();
            },
        };

        let mut requests = Vec::with_capacity(pending_orders.len());
        for order in pending_orders {
            let order_id = order.create_order.create_id.clone();
            let action_info = match self.order_mapper.map(&order, latest_block.as_ref()).await {
                Ok(action_info) => {
                    tracing::info!(
                        chain = %self.chain_identifier,
                        order_id = %order_id,
                        action = %action_info.action,
                        has_swap = action_info.swap.is_some(),
                        "order_mapper produced action",
                    );
                    action_info
                },
                Err(error) => {
                    tracing::error!(
                        chain = %self.chain_identifier,
                        order_id = %order_id,
                        error = %error,
                        "failed to map order to bitcoin action",
                    );
                    continue;
                },
            };

            let action_info = if matches!(action_info.action, HTLCAction::NoOp) {
                match self.try_force_source_redeem(&order) {
                    Some(forced) => {
                        tracing::warn!(
                            chain = %self.chain_identifier,
                            order_id = %order_id,
                            "forcing source redeem despite missing init block / confirmations",
                        );
                        forced
                    },
                    None => {
                        self.log_noop_reason(&order, latest_block.as_ref());
                        continue;
                    },
                }
            } else {
                action_info
            };

            let Some(swap) = action_info.swap else {
                tracing::error!(
                    chain = %self.chain_identifier,
                    order_id = %order_id,
                    action = %action_info.action,
                    "mapped non-noop action without swap payload",
                );
                continue;
            };

            requests.push(OrderExecutionRequest::new(
                order,
                swap,
                action_info.action,
            ));
        }

        self.filter_cached_requests(requests).await
    }

    async fn latest_block_number(
        &self,
        pending_orders: &[MatchedOrderVerbose],
    ) -> Result<Option<BigDecimal>, ExecutorError> {
        if !self
            .order_mapper
            .has_refundable_swaps(&pending_orders.to_vec())
        {
            return Ok(None);
        }

        let height = self.action_executor.current_block_height().await?;
        Ok(Some(BigDecimal::from(height)))
    }

    async fn filter_cached_requests(
        &self,
        requests: Vec<OrderExecutionRequest>,
    ) -> Vec<OrderExecutionRequest> {
        let mut filtered = Vec::with_capacity(requests.len());

        for request in requests {
            if self.cache.get(&request.cache_key).await.is_some() {
                tracing::debug!(
                    chain = %self.chain_identifier,
                    order_id = %request.order_id,
                    action = %request.action,
                    "skipping cached bitcoin action",
                );
                continue;
            }

            filtered.push(request);
        }

        filtered
    }
}

impl Executor {
    fn try_force_source_redeem(&self, order: &MatchedOrderVerbose) -> Option<ActionWithInfo> {
        let src = &order.source_swap;
        let dst = &order.destination_swap;

        if !src.chain.contains(&self.chain_identifier) {
            return None;
        }
        if src.redeem_tx_hash.is_some() || src.refund_tx_hash.is_some() {
            return None;
        }
        if !src.initiate_tx_hash.is_some() {
            return None;
        }
        if !dst.secret.is_some() {
            return None;
        }

        let raw = dst.secret.to_string();
        let trimmed = raw.strip_prefix("0x").unwrap_or(&raw);
        let secret = match hex::decode(trimmed) {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::error!(
                    order_id = %order.create_order.create_id,
                    error = %err,
                    "force-redeem: failed to decode destination secret",
                );
                return None;
            },
        };

        Some(ActionWithInfo {
            action: HTLCAction::Redeem { secret: secret.into() },
            swap: Some(src.clone()),
        })
    }

    fn log_noop_reason(
        &self,
        order: &MatchedOrderVerbose,
        latest_block: Option<&BigDecimal>,
    ) {
        let order_id = &order.create_order.create_id;
        let src = &order.source_swap;
        let dst = &order.destination_swap;
        let chain = &self.chain_identifier;

        let src_supported = src.chain.contains(chain);
        let dst_supported = dst.chain.contains(chain);

        tracing::info!(
            order_id = %order_id,
            src_chain = %src.chain,
            dst_chain = %dst.chain,
            src_supported,
            dst_supported,
            src_init_tx = src.initiate_tx_hash.is_some(),
            src_init_block = ?src.initiate_block_number,
            src_redeem_tx = src.redeem_tx_hash.is_some(),
            src_refund_tx = src.refund_tx_hash.is_some(),
            src_secret = src.secret.is_some(),
            src_filled = %src.filled_amount,
            src_amount = %src.amount,
            src_confs = src.current_confirmations,
            src_required_confs = src.required_confirmations,
            dst_init_tx = dst.initiate_tx_hash.is_some(),
            dst_init_block = ?dst.initiate_block_number,
            dst_redeem_tx = dst.redeem_tx_hash.is_some(),
            dst_refund_tx = dst.refund_tx_hash.is_some(),
            dst_secret = dst.secret.is_some(),
            dst_timelock = dst.timelock,
            latest_block = ?latest_block,
            blacklisted = order.create_order.additional_data.is_blacklisted,
            "noop diagnosis: order state",
        );

        if !src_supported && !dst_supported {
            tracing::warn!(
                order_id = %order_id,
                src_chain = %src.chain,
                dst_chain = %dst.chain,
                configured = %chain,
                "noop reason: neither source nor destination chain matches configured chain",
            );
            return;
        }

        if src_supported {
            let reason = source_redeem_block_reason(src, dst);
            tracing::warn!(
                order_id = %order_id,
                side = "source",
                reason,
                "noop reason: source-side action declined",
            );
        }

        if dst_supported {
            let reason = destination_action_block_reason(order, latest_block);
            tracing::warn!(
                order_id = %order_id,
                side = "destination",
                reason,
                "noop reason: destination-side action declined",
            );
        }
    }
}

fn has_valid_initiate(swap: &SingleSwap) -> bool {
    swap.initiate_tx_hash.is_some()
        && swap
            .initiate_block_number
            .as_ref()
            .map_or(false, |b| b > &BigDecimal::zero())
}

fn source_redeem_block_reason(src: &SingleSwap, dst: &SingleSwap) -> &'static str {
    if src.redeem_tx_hash.is_some() {
        return "source already redeemed";
    }
    if src.refund_tx_hash.is_some() {
        return "source already refunded";
    }
    if !src.initiate_tx_hash.is_some() {
        return "source has no initiate tx";
    }
    if !has_valid_initiate(src) {
        return "source initiate has no/zero block number";
    }
    if !dst.secret.is_some() {
        return "destination secret not yet revealed (waiting for counterparty redeem)";
    }
    "redeem preconditions met but additional_check rejected (price/policy)"
}

fn destination_action_block_reason(
    order: &MatchedOrderVerbose,
    latest_block: Option<&BigDecimal>,
) -> &'static str {
    let src = &order.source_swap;
    let dst = &order.destination_swap;

    if dst.refund_tx_hash.is_some() {
        return "destination already refunded";
    }
    if dst.redeem_tx_hash.is_some() && dst.secret.is_some() {
        return "destination already redeemed";
    }

    if dst.initiate_tx_hash.is_some() {
        if let Some(latest) = latest_block {
            if let Some(init_block) = dst.initiate_block_number.as_ref() {
                let timelock = BigDecimal::from(dst.timelock);
                if latest >= &(init_block + &timelock) {
                    return "destination initiated and timelock expired but refund declined (additional_check)";
                }
                return "destination initiated, waiting for timelock to expire before refund";
            }
        }
        return "destination already initiated, waiting for redeem/refund";
    }

    if order.create_order.additional_data.is_blacklisted {
        return "order blacklisted; will not initiate";
    }

    let is_source_bitcoin = src.chain.contains("bitcoin");
    if is_source_bitcoin {
        if !src.initiate_tx_hash.is_some() {
            return "source not yet initiated; cannot initiate destination";
        }
    } else {
        if !has_valid_initiate(src) {
            return "source initiate not valid (no tx or no block number)";
        }
        if src.current_confirmations < src.required_confirmations {
            return "source initiate awaiting confirmations";
        }
    }

    if src.filled_amount != src.amount {
        return "source not fully filled; cannot initiate destination";
    }

    "initiate preconditions met but additional_check rejected (likely price threshold or instant-refund preferred)"
}

async fn sleep_for_poll_interval(polling_interval_ms: u64) {
    tokio::time::sleep(Duration::from_millis(polling_interval_ms.max(1))).await;
}
