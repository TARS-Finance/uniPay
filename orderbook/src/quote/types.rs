use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};

/// Quote-side rendering of an asset amount and its USD value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteAssetView {
    pub asset: String,
    pub amount: BigDecimal,
    pub display: BigDecimal,
    pub value: BigDecimal,
}

/// One candidate route returned by the quote service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuoteRoute {
    pub strategy_id: String,
    pub source: QuoteAssetView,
    pub destination: QuoteAssetView,
    pub solver_id: String,
    pub estimated_time: i32,
    pub slippage: u64,
    pub fee: u64,
    pub fixed_fee: BigDecimal,
}

/// Public quote request supporting either exact-in or exact-out mode.
#[derive(Debug, Clone, Deserialize)]
pub struct QuoteRequest {
    pub from: String,
    pub to: String,
    pub from_amount: Option<BigDecimal>,
    pub to_amount: Option<BigDecimal>,
    #[serde(default)]
    pub affiliate_fee: u64,
    pub slippage: Option<u64>,
    pub strategy_id: Option<String>,
}

/// Full quote response containing all routes plus the selected best route.
#[derive(Debug, Clone, Serialize)]
pub struct QuoteResponse {
    pub best: Option<QuoteRoute>,
    pub routes: Vec<QuoteRoute>,
    pub input_token_price: f64,
    pub output_token_price: f64,
}
