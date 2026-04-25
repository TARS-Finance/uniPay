use std::sync::Arc;

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use chrono::Utc;
use orderbook::{
    primitives::MatchedOrderVerbose,
    traits::Orderbook,
    OrderbookProvider,
};
use tars::orderbook::OrderMapper;
use tars::primitives::HTLCAction;
use tracing::{error, info, warn};

use crate::htlc::{
    bigdecimal_to_u256, parse_address, parse_secret_hash, ERC20HTLC, ERC20, NativeHTLC,
};
use crate::orders::PendingOrdersProvider;
use crate::settings::{Erc20Pair, Settings};

/// Zero address sentinel used for native token swaps.
const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

pub struct InitiatorService<P> {
    provider: Arc<P>,
    orderbook: Arc<OrderbookProvider>,
    orders_provider: PendingOrdersProvider,
    order_mapper: Arc<OrderMapper>,
    native_htlc: Address,
    erc20_pairs: Vec<Erc20Pair>,
    chain_name: String,
    executor_address: Address,
    polling_interval_ms: u64,
}

impl<P: Provider + Clone + Send + Sync + 'static> InitiatorService<P> {
    pub fn new(
        provider: Arc<P>,
        orderbook: Arc<OrderbookProvider>,
        orders_provider: PendingOrdersProvider,
        order_mapper: Arc<OrderMapper>,
        settings: &Settings,
        executor_address: Address,
    ) -> eyre::Result<Self> {
        let native_htlc = parse_address(&settings.initia.native_htlc_address)?;
        Ok(Self {
            provider,
            orderbook,
            orders_provider,
            order_mapper,
            native_htlc,
            erc20_pairs: settings.initia.erc20_pairs.clone(),
            chain_name: settings.initia.chain_name.clone(),
            executor_address,
            polling_interval_ms: settings.polling_interval_ms,
        })
    }

    pub async fn run(&self) {
        info!(chain = %self.chain_name, "InitiatorService started");
        loop {
            if let Err(e) = self.process().await {
                error!(chain = %self.chain_name, "InitiatorService error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(self.polling_interval_ms)).await;
        }
    }

    async fn process(&self) -> eyre::Result<()> {
        let executor_str = format!("{}", self.executor_address).to_lowercase();
        let orders = self.orders_provider.get_pending_orders(&self.chain_name, &executor_str).await?;
        info!(chain = %self.chain_name, count = orders.len(), solver = %executor_str, "Initiator polled pending orders");

        for order in &orders {
            let action_info = match self.order_mapper.map(order, None).await {
                Ok(info) => info,
                Err(e) => {
                    error!(order_id = %order.create_order.create_id, "OrderMapper error: {e}");
                    continue;
                }
            };
            info!(order_id = %order.create_order.create_id, action = ?action_info.action, "Initiator mapped order");
            if matches!(action_info.action, HTLCAction::Initiate) {
                if let Err(e) = self.initiate_destination(order).await {
                    error!(order_id = %order.create_order.create_id, "Failed to initiate destination: {e}");
                }
            }
        }

        Ok(())
    }

    async fn initiate_destination(&self, order: &MatchedOrderVerbose) -> eyre::Result<()> {
        let dest = &order.destination_swap;
        let amount = bigdecimal_to_u256(&dest.amount)?;
        let initiator = parse_address(&dest.initiator)?;
        let redeemer = parse_address(&dest.redeemer)?;
        let timelock = U256::from(dest.timelock as u64);
        let secret_hash = parse_secret_hash(&dest.secret_hash)?;

        let token_addr = dest
            .token_address
            .as_deref()
            .unwrap_or(ZERO_ADDRESS)
            .to_lowercase();

        let tx_hash;
        let block_number;

        if token_addr == ZERO_ADDRESS || token_addr == "primary" {
            // Native INIT swap
            let balance = self.provider.get_balance(self.executor_address).await?;
            if balance < amount {
                warn!(
                    order_id = %order.create_order.create_id,
                    required = %amount,
                    available = %balance,
                    "Insufficient native balance, skipping"
                );
                return Ok(());
            }

            let contract = NativeHTLC::new(self.native_htlc, self.provider.as_ref());
            let receipt = contract
                .initiateOnBehalf(initiator, redeemer, timelock, amount, secret_hash)
                .value(amount)
                .send()
                .await?
                .get_receipt()
                .await?;

            tx_hash = format!("{:?}", receipt.transaction_hash);
            block_number = receipt.block_number.unwrap_or(0) as i64;
        } else {
            // ERC20 swap — find the matching pair
            let pair = self
                .erc20_pairs
                .iter()
                .find(|p| p.token_address.to_lowercase() == token_addr)
                .ok_or_else(|| eyre::eyre!("No ERC20 pair configured for token {token_addr}"))?;

            let htlc_addr = parse_address(&pair.htlc_address)?;
            let token_contract_addr = parse_address(&pair.token_address)?;

            // Approve HTLC to pull the tokens if needed
            let token_contract = ERC20::new(token_contract_addr, self.provider.as_ref());
            let allowance = token_contract
                .allowance(self.executor_address, htlc_addr)
                .call()
                .await?;

            if allowance < amount {
                token_contract
                    .approve(htlc_addr, U256::MAX)
                    .send()
                    .await?
                    .get_receipt()
                    .await?;
                info!(token = %token_addr, htlc = %htlc_addr, "Approved ERC20 HTLC");
            }

            let contract = ERC20HTLC::new(htlc_addr, self.provider.as_ref());
            let receipt = contract
                .initiateOnBehalf(initiator, redeemer, timelock, amount, secret_hash)
                .send()
                .await?
                .get_receipt()
                .await?;

            tx_hash = format!("{:?}", receipt.transaction_hash);
            block_number = receipt.block_number.unwrap_or(0) as i64;
        }

        self.orderbook
            .update_swap_initiate(
                &dest.swap_id,
                dest.amount.clone(),
                &tx_hash,
                block_number,
                Utc::now(),
            )
            .await?;

        info!(
            order_id = %order.create_order.create_id,
            tx_hash = %tx_hash,
            token = %token_addr,
            "Destination initiated on Initia"
        );

        Ok(())
    }
}
