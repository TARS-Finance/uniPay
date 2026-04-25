use std::sync::Arc;

use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use chrono::Utc;
use orderbook::{primitives::SwapChain, traits::Orderbook, OrderbookProvider};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

use crate::htlc::{
    compute_order_id, parse_address, parse_secret_bytes, parse_secret_hash,
    bigdecimal_to_u256, ERC20HTLC, NativeHTLC,
};
use crate::settings::{Erc20Pair, Settings};

const ZERO_ADDRESS: &str = "0x0000000000000000000000000000000000000000";

#[derive(Clone)]
pub struct ApiState<P> {
    provider: Arc<P>,
    orderbook: Arc<OrderbookProvider>,
    native_htlc: Address,
    erc20_pairs: Vec<Erc20Pair>,
    chain_id: u64,
}

fn bad(msg: impl ToString) -> (StatusCode, Json<ErrorResponse>) {
    (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: msg.to_string() }))
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Deserialize)]
pub struct SecretRequest {
    pub order_id: String,
    pub secret: String,
}

#[derive(Serialize)]
pub struct SecretResponse {
    pub tx_hash: String,
}

impl<P: Provider + Clone + Send + Sync + 'static> ApiState<P> {
    pub fn new(
        provider: Arc<P>,
        orderbook: Arc<OrderbookProvider>,
        settings: &Settings,
    ) -> eyre::Result<Self> {
        let native_htlc = parse_address(&settings.initia.native_htlc_address)?;
        Ok(Self {
            provider,
            orderbook,
            native_htlc,
            erc20_pairs: settings.initia.erc20_pairs.clone(),
            chain_id: settings.initia.chain_id,
        })
    }

    pub fn router(self) -> Router {
        let cors = CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any);

        Router::new()
            .route("/health", get(health_handler))
            .route("/secret", post(secret_handler::<P>))
            .layer(cors)
            .with_state(self)
    }
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok", "service": "initia-executor" }))
}

async fn secret_handler<P: Provider + Clone + Send + Sync + 'static>(
    State(state): State<ApiState<P>>,
    Json(req): Json<SecretRequest>,
) -> Result<Json<SecretResponse>, (StatusCode, Json<ErrorResponse>)> {
    state.handle_secret(req).await
        .map(|tx_hash| Json(SecretResponse { tx_hash }))
        .map_err(|e| { error!("Secret endpoint error: {e}"); bad(e) })
}

impl<P: Provider + Clone + Send + Sync + 'static> ApiState<P> {
    async fn handle_secret(&self, req: SecretRequest) -> eyre::Result<String> {
        let secret_hex = req.secret.strip_prefix("0x").unwrap_or(&req.secret);
        let secret_raw =
            hex::decode(secret_hex).map_err(|e| eyre::eyre!("Invalid secret hex: {e}"))?;

        let matched = self
            .orderbook
            .get_swap(&req.order_id, SwapChain::Destination)
            .await
            .map_err(|e| eyre::eyre!("Orderbook error: {e}"))?
            .ok_or_else(|| eyre::eyre!("Order '{}' not found", req.order_id))?;

        if matched.initiate_tx_hash.is_none() {
            return Err(eyre::eyre!(
                "Destination not yet initiated — wait for executor to lock funds first"
            ));
        }
        if matched.redeem_tx_hash.is_some() {
            return Err(eyre::eyre!("Already redeemed"));
        }
        if matched.refund_tx_hash.is_some() {
            return Err(eyre::eyre!("Already refunded"));
        }

        let computed: [u8; 32] = Sha256::digest(&secret_raw).into();
        let stored = hex::decode(
            matched.secret_hash.strip_prefix("0x").unwrap_or(&matched.secret_hash),
        )
        .map_err(|e| eyre::eyre!("Invalid stored secretHash: {e}"))?;

        if computed.as_slice() != stored.as_slice() {
            return Err(eyre::eyre!("Secret does not match secretHash — rejected"));
        }

        let token_addr = matched
            .token_address
            .as_deref()
            .unwrap_or(ZERO_ADDRESS)
            .to_lowercase();

        let htlc_addr = if token_addr == ZERO_ADDRESS || token_addr == "primary" {
            matched
                .htlc_address
                .as_deref()
                .map(parse_address)
                .transpose()?
                .unwrap_or(self.native_htlc)
        } else {
            let pair = self
                .erc20_pairs
                .iter()
                .find(|p| p.token_address.to_lowercase() == token_addr)
                .ok_or_else(|| {
                    eyre::eyre!("No ERC20 pair configured for token {token_addr}")
                })?;
            parse_address(&pair.htlc_address)?
        };

        let initiator = parse_address(&matched.initiator)?;
        let redeemer = parse_address(&matched.redeemer)?;
        let timelock = U256::from(matched.timelock as u64);
        let amount = bigdecimal_to_u256(&matched.amount)?;
        let secret_hash = parse_secret_hash(&matched.secret_hash)?;
        let secret_bytes = parse_secret_bytes(secret_hex)?;

        let order_id = compute_order_id(
            self.chain_id,
            secret_hash,
            initiator,
            redeemer,
            timelock,
            amount,
            htlc_addr,
        );

        let receipt = if token_addr == ZERO_ADDRESS {
            NativeHTLC::new(htlc_addr, self.provider.as_ref())
                .redeem(order_id, secret_bytes)
                .send()
                .await?
                .get_receipt()
                .await?
        } else {
            ERC20HTLC::new(htlc_addr, self.provider.as_ref())
                .redeem(order_id, secret_bytes)
                .send()
                .await?
                .get_receipt()
                .await?
        };

        let tx_hash = format!("{:?}", receipt.transaction_hash);
        let block_number = receipt.block_number.unwrap_or(0) as i64;

        self.orderbook
            .update_swap_redeem(&matched.swap_id, &tx_hash, secret_hex, block_number, Utc::now())
            .await?;

        info!(
            order_id = %req.order_id,
            tx_hash = %tx_hash,
            "Destination redeemed via /secret endpoint"
        );

        Ok(tx_hash)
    }
}
