use bigdecimal::BigDecimal;
use serde::Deserialize;

/// Public create-order request accepted by the unified service.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateOrderRequest {
    pub from: String,
    pub to: String,
    pub from_amount: Option<BigDecimal>,
    pub to_amount: Option<BigDecimal>,
    pub initiator_source_address: String,
    pub initiator_destination_address: String,
    pub secret_hash: String,
    pub strategy_id: Option<String>,
    #[serde(default)]
    pub affiliate_fee: u64,
    pub slippage: Option<u64>,
    pub nonce: Option<BigDecimal>,
    pub bitcoin_optional_recipient: Option<String>,
    pub source_delegator: Option<String>,
}
