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
use tracing::{error, info};

use crate::htlc::{
    bigdecimal_to_u256, compute_order_id, parse_address, parse_secret_hash,
    ERC20HTLC, NativeHTLC,
};
use crate::orders::PendingOrdersProvider;
use crate::settings::{Erc20Pair, Settings};

const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

pub struct RedeemerService<P> {
    provider: Arc<P>,
    orderbook: Arc<OrderbookProvider>,
    orders_provider: PendingOrdersProvider,
    order_mapper: Arc<OrderMapper>,
    native_htlc: Address,
    erc20_pairs: Vec<Erc20Pair>,
    chain_name: String,
    chain_id: u64,
    executor_address: Address,
    polling_interval_ms: u64,
}

impl<P: Provider + Clone + Send + Sync + 'static> RedeemerService<P> {
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
            chain_id: settings.initia.chain_id,
            executor_address,
            polling_interval_ms: settings.polling_interval_ms,
        })
    }

    pub async fn run(&self) {
        info!(chain = %self.chain_name, "RedeemerService started");
        loop {
            if let Err(e) = self.process().await {
                error!(chain = %self.chain_name, "RedeemerService error: {e}");
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(self.polling_interval_ms)).await;
        }
    }

    async fn process(&self) -> eyre::Result<()> {
        let executor_str = format!("{}", self.executor_address).to_lowercase();
        let orders = self.orders_provider.get_pending_orders(&self.chain_name, &executor_str).await?;

        for order in &orders {
            let action_info = match self.order_mapper.map(order, None).await {
                Ok(info) => info,
                Err(e) => {
                    error!(order_id = %order.create_order.create_id, "OrderMapper error: {e}");
                    continue;
                }
            };
            if let HTLCAction::Redeem { secret } = action_info.action {
                if let Err(e) = self.redeem_source(order, &secret).await {
                    error!(order_id = %order.create_order.create_id, "Failed to redeem source: {e}");
                }
            }
        }

        Ok(())
    }

    async fn redeem_source(&self, order: &MatchedOrderVerbose, secret_bytes: &[u8]) -> eyre::Result<()> {
        let src = &order.source_swap;

        let initiator = parse_address(&src.initiator)?;
        let redeemer = parse_address(&src.redeemer)?;
        let timelock = U256::from(src.timelock as u64);
        let amount = bigdecimal_to_u256(&src.amount)?;
        let secret_hash = parse_secret_hash(&src.secret_hash)?;

        let token_addr = src
            .token_address
            .as_deref()
            .unwrap_or(ZERO_ADDRESS)
            .to_lowercase();

        let htlc_addr = if token_addr == ZERO_ADDRESS || token_addr == "primary" {
            src.htlc_address
                .as_deref()
                .map(parse_address)
                .transpose()?
                .unwrap_or(self.native_htlc)
        } else {
            let pair = self
                .erc20_pairs
                .iter()
                .find(|p| p.token_address.to_lowercase() == token_addr)
                .ok_or_else(|| eyre::eyre!("No ERC20 pair for token {token_addr}"))?;
            parse_address(&pair.htlc_address)?
        };

        let order_id =
            compute_order_id(self.chain_id, secret_hash, initiator, redeemer, timelock, amount, htlc_addr);

        let tx_hash;
        let block_number;

        if token_addr == ZERO_ADDRESS || token_addr == "primary" {
            let contract = NativeHTLC::new(htlc_addr, self.provider.as_ref());
            let receipt = contract
                .redeem(order_id, secret_bytes.to_vec().into())
                .send()
                .await?
                .get_receipt()
                .await?;
            tx_hash = format!("{:?}", receipt.transaction_hash);
            block_number = receipt.block_number.unwrap_or(0) as i64;
        } else {
            let contract = ERC20HTLC::new(htlc_addr, self.provider.as_ref());
            let receipt = contract
                .redeem(order_id, secret_bytes.to_vec().into())
                .send()
                .await?
                .get_receipt()
                .await?;
            tx_hash = format!("{:?}", receipt.transaction_hash);
            block_number = receipt.block_number.unwrap_or(0) as i64;
        }

        let secret_hex = hex::encode(secret_bytes);
        self.orderbook
            .update_swap_redeem(&src.swap_id, &tx_hash, &secret_hex, block_number, Utc::now())
            .await?;

        info!(
            order_id = %order.create_order.create_id,
            tx_hash = %tx_hash,
            token = %token_addr,
            "Source redeemed on Initia"
        );

        Ok(())
    }
}
