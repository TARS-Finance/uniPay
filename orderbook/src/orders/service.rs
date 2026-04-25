use crate::{
    common::{normalize_address, sha256_hex},
    config::settings::QuoteSettings,
    error::AppError,
    orders::matching::{SwapIdGenerator, calculate_match_fee, sign_order_payload},
    orders::types::CreateOrderRequest,
    quote::{service::QuoteService, types::QuoteRequest},
};
use bigdecimal::BigDecimal;
use chrono::Utc;
use std::{collections::HashMap, sync::Arc};
use tars::orderbook::{
    OrderbookProvider,
    primitives::{
        AdditionalData, CreateOrder, MatchedOrderVerbose, MaybeString, Order, SingleSwap,
    },
    traits::Orderbook,
};

/// Creates matched orders by reusing the quote path and persisting the result.
#[derive(Clone)]
pub struct OrderService {
    orderbook: Arc<OrderbookProvider>,
    quote_service: Arc<QuoteService>,
    quote_settings: QuoteSettings,
    swap_id_generator: SwapIdGenerator,
}

impl OrderService {
    /// Creates the order service with access to quote logic and persistence.
    pub fn new(
        orderbook: Arc<OrderbookProvider>,
        quote_service: Arc<QuoteService>,
        quote_settings: QuoteSettings,
        chain_ids: HashMap<String, u128>,
    ) -> Self {
        Self {
            orderbook,
            quote_service,
            quote_settings,
            swap_id_generator: SwapIdGenerator::new(chain_ids),
        }
    }

    /// Validates, prices, signs, and persists a matched order.
    pub async fn create_order(
        &self,
        request: CreateOrderRequest,
    ) -> Result<MatchedOrderVerbose, AppError> {
        validate_secret_hash(&request.secret_hash)?;

        // The secret hash remains the primary uniqueness guard for new orders.
        if self.orderbook.exists(&request.secret_hash).await? {
            return Err(AppError::conflict("secret hash already exists"));
        }

        // Reuse the quote pipeline so pricing and route selection stay consistent.
        let quote = self
            .quote_service
            .quote(QuoteRequest {
                from: request.from.clone(),
                to: request.to.clone(),
                from_amount: request.from_amount.clone(),
                to_amount: request.to_amount.clone(),
                affiliate_fee: request.affiliate_fee,
                slippage: request.slippage,
                strategy_id: request.strategy_id.clone(),
            })
            .await?;

        let route = quote
            .best
            .ok_or_else(|| AppError::bad_request("no route available for order"))?;
        let strategy = self
            .quote_service
            .strategy(&route.strategy_id)
            .ok_or_else(|| AppError::bad_request("selected strategy no longer exists"))?
            .clone();

        // Build the shared order metadata and timing values once for both swaps.
        let now = Utc::now();
        let create_id = sha256_hex(&[request.secret_hash.as_bytes()]);
        let source_timelock = (strategy.min_source_timelock.saturating_mul(12)) as i32;
        let deadline = if strategy.source_chain.contains("bitcoin") {
            now + chrono::TimeDelta::hours(6)
        } else {
            now + chrono::TimeDelta::minutes(self.quote_settings.order_deadline_in_minutes)
        };

        let additional_data = AdditionalData {
            strategy_id: strategy.id.clone(),
            bitcoin_optional_recipient: request.bitcoin_optional_recipient.clone(),
            source_delegator: request.source_delegator.clone(),
            input_token_price: quote.input_token_price,
            output_token_price: quote.output_token_price,
            sig: String::new(),
            deadline: deadline.timestamp(),
            instant_refund_tx_bytes: None,
            redeem_tx_bytes: None,
            tx_hash: None,
            is_blacklisted: false,
            integrator: None,
            version: strategy
                .source_asset
                .version
                .clone()
                .max(strategy.dest_asset.version.clone()),
            bitcoin: None,
        };

        // Source and destination swap IDs must reproduce the target chain's native derivation rules.
        let source_swap_id = self
            .swap_id_generator
            .generate_swap_id(
                &strategy.source_chain,
                &request.initiator_source_address,
                &strategy.source_chain_address,
                source_timelock as u64,
                &request.secret_hash,
                &route.source.amount,
                &strategy.source_asset.version,
                &strategy.source_asset.htlc_address,
            )
            .map_err(AppError::from)?;
        let destination_swap_id = self
            .swap_id_generator
            .generate_swap_id(
                &strategy.dest_chain,
                &strategy.dest_chain_address,
                &request.initiator_destination_address,
                strategy.destination_timelock,
                &request.secret_hash,
                &route.destination.amount,
                &strategy.dest_asset.version,
                &strategy.dest_asset.htlc_address,
            )
            .map_err(AppError::from)?;

        // Sign the canonical order payload before storing it in the orderbook tables.
        let unsigned_order = Order {
            source_chain: strategy.source_chain.clone(),
            destination_chain: strategy.dest_chain.clone(),
            source_asset: normalize_address(
                &strategy.source_chain,
                &strategy.source_asset.htlc_address,
            ),
            destination_asset: normalize_address(
                &strategy.dest_chain,
                &strategy.dest_asset.htlc_address,
            ),
            initiator_source_address: Some(normalize_address(
                &strategy.source_chain,
                &request.initiator_source_address,
            )),
            initiator_destination_address: Some(normalize_address(
                &strategy.dest_chain,
                &request.initiator_destination_address,
            )),
            source_amount: route.source.amount.clone(),
            destination_amount: route.destination.amount.clone(),
            fee: Some(BigDecimal::from(1)),
            user_id: None,
            nonce: Some(
                request
                    .nonce
                    .clone()
                    .unwrap_or_else(|| BigDecimal::from(rand::random::<u64>())),
            ),
            min_destination_confirmations: Some(0),
            timelock: Some(source_timelock as u64),
            secret_hash: Some(request.secret_hash.clone()),
            affiliate_fees: Default::default(),
            additional_data: additional_data.clone(),
        };
        let signature = sign_order_payload(
            &unsigned_order,
            self.quote_settings.quote_private_key.as_deref(),
        )
        .await
        .unwrap_or_default();
        let nonce = unsigned_order
            .nonce
            .clone()
            .unwrap_or_else(|| BigDecimal::from(0));
        let additional_data = AdditionalData {
            sig: signature,
            ..additional_data
        };

        // Assemble the full matched-order graph exactly as the shared orderbook expects it.
        let matched_order = MatchedOrderVerbose {
            created_at: now,
            updated_at: now,
            deleted_at: None,
            source_swap: SingleSwap {
                created_at: now,
                updated_at: now,
                deleted_at: None,
                swap_id: source_swap_id,
                chain: strategy.source_chain.clone(),
                asset: normalize_address(
                    &strategy.source_chain,
                    &strategy.source_asset.htlc_address,
                ),
                htlc_address: Some(strategy.source_asset.htlc_address.clone()),
                token_address: Some(strategy.source_asset.token_address.clone()),
                initiator: normalize_address(
                    &strategy.source_chain,
                    &request.initiator_source_address,
                ),
                redeemer: normalize_address(&strategy.source_chain, &strategy.source_chain_address),
                timelock: source_timelock,
                filled_amount: BigDecimal::from(0),
                amount: route.source.amount.clone(),
                secret_hash: request.secret_hash.clone(),
                secret: MaybeString::new(String::new()),
                initiate_tx_hash: MaybeString::new(String::new()),
                redeem_tx_hash: MaybeString::new(String::new()),
                refund_tx_hash: MaybeString::new(String::new()),
                initiate_block_number: None,
                redeem_block_number: None,
                refund_block_number: None,
                required_confirmations: strategy.min_source_confirmations as i32,
                current_confirmations: 0,
                initiate_timestamp: None,
                redeem_timestamp: None,
                refund_timestamp: None,
            },
            destination_swap: SingleSwap {
                created_at: now,
                updated_at: now,
                deleted_at: None,
                swap_id: destination_swap_id,
                chain: strategy.dest_chain.clone(),
                asset: normalize_address(&strategy.dest_chain, &strategy.dest_asset.htlc_address),
                htlc_address: Some(strategy.dest_asset.htlc_address.clone()),
                token_address: Some(strategy.dest_asset.token_address.clone()),
                initiator: normalize_address(&strategy.dest_chain, &strategy.dest_chain_address),
                redeemer: normalize_address(
                    &strategy.dest_chain,
                    &request.initiator_destination_address,
                ),
                timelock: strategy.destination_timelock as i32,
                filled_amount: BigDecimal::from(0),
                amount: route.destination.amount.clone(),
                secret_hash: request.secret_hash.clone(),
                secret: MaybeString::new(String::new()),
                initiate_tx_hash: MaybeString::new(String::new()),
                redeem_tx_hash: MaybeString::new(String::new()),
                refund_tx_hash: MaybeString::new(String::new()),
                initiate_block_number: None,
                redeem_block_number: None,
                refund_block_number: None,
                required_confirmations: 0,
                current_confirmations: 0,
                initiate_timestamp: None,
                redeem_timestamp: None,
                refund_timestamp: None,
            },
            create_order: CreateOrder {
                created_at: now,
                updated_at: now,
                deleted_at: None,
                create_id,
                block_number: BigDecimal::from(0),
                source_chain: strategy.source_chain.clone(),
                destination_chain: strategy.dest_chain.clone(),
                source_asset: normalize_address(
                    &strategy.source_chain,
                    &strategy.source_asset.htlc_address,
                ),
                destination_asset: normalize_address(
                    &strategy.dest_chain,
                    &strategy.dest_asset.htlc_address,
                ),
                initiator_source_address: normalize_address(
                    &strategy.source_chain,
                    &request.initiator_source_address,
                ),
                initiator_destination_address: normalize_address(
                    &strategy.dest_chain,
                    &request.initiator_destination_address,
                ),
                source_amount: route.source.amount.clone(),
                destination_amount: route.destination.amount.clone(),
                fee: if strategy.fee == 0 {
                    BigDecimal::from(0)
                } else {
                    calculate_match_fee(
                        &route.source.amount,
                        &route.destination.amount,
                        strategy.source_asset.decimals,
                        strategy.dest_asset.decimals,
                        quote.input_token_price,
                        quote.output_token_price,
                    )
                    .map_err(AppError::from)?
                },
                nonce,
                min_destination_confirmations: 0,
                timelock: source_timelock,
                secret_hash: request.secret_hash,
                user_id: None,
                affiliate_fees: Some(Vec::new()),
                additional_data,
            },
        };

        self.orderbook.create_matched_order(&matched_order).await?;
        Ok(matched_order)
    }
}

/// Rejects malformed secret hashes before any quote or persistence work begins.
fn validate_secret_hash(secret_hash: &str) -> Result<(), AppError> {
    if secret_hash.len() != 64 {
        return Err(AppError::bad_request(
            "secret_hash must be a 64 character hex string",
        ));
    }

    if secret_hash.starts_with("0x") {
        return Err(AppError::bad_request("secret_hash must not be 0x-prefixed"));
    }

    hex::decode(secret_hash).map_err(|_| AppError::bad_request("invalid secret_hash hex"))?;
    Ok(())
}
