use serde::{Deserialize, Serialize};

/// Query params for the quote-compatible fiat endpoint.
#[derive(Debug, Clone, Deserialize)]
pub struct UsdOrderPairParams {
    pub order_pair: String,
}

/// USD prices returned by the quote-compatible fiat endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct UsdOrderPairResponse {
    pub input_token_price: f64,
    pub output_token_price: f64,
}
