use bigdecimal::{BigDecimal, Zero};
use primitives::{ContractInfo, HTLCVersion};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub asset: String,
    pub token_id: String,
    pub decimals: u8,
    pub htlc_address: String,
    pub token_address: String,
    pub version: HTLCVersion,
}

fn default_fixed_fee() -> BigDecimal {
    BigDecimal::zero()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Strategy {
    pub id: String,
    pub source_chain_address: String,
    pub dest_chain_address: String,
    pub source_chain: String,
    pub dest_chain: String,
    pub source_asset: Asset,
    pub dest_asset: Asset,
    pub makers: Vec<String>,
    pub min_amount: BigDecimal,
    pub max_amount: BigDecimal,
    pub min_source_timelock: u64,
    pub destination_timelock: u64,
    pub min_source_confirmations: u64,
    pub fee: u64,
    #[serde(default = "default_fixed_fee")]
    pub fixed_fee: BigDecimal,
    pub solver_id: Option<String>,
    pub max_slippage: u64,
    #[serde(default)]
    pub version: HTLCVersion,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct QuoteResult {
    pub input_token_price: f64,
    pub output_token_price: f64,
    pub quotes: HashMap<String, BigDecimal>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FiatResult {
    pub input_token_price: f64,
    pub output_token_price: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FiatBatchRequest {
    pub order_pairs: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FiatResponseItem {
    pub is_success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<FiatResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AssetResponseItem {
    pub id: String,
    pub chain: String,
    pub icon: String,
    pub htlc: Option<ContractInfo>,
    pub token: Option<ContractInfo>,
    pub decimals: u8,
    pub min_amount: String,
    pub max_amount: String,
    pub price: Option<BigDecimal>,
}
