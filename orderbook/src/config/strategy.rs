use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use tars::primitives::HTLCVersion;

/// Default fixed fee when a strategy omits one from config.
fn default_fixed_fee() -> BigDecimal {
    BigDecimal::from(0)
}

/// Asset fields copied from strategy config into the runtime registry.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StrategyAssetConfig {
    pub asset: String,
    pub htlc_address: String,
    pub token_address: String,
    pub token_id: String,
    #[serde(default)]
    pub display_symbol: Option<String>,
    pub decimals: u8,
    pub version: HTLCVersion,
}

/// Strategy file schema used to construct quoteable routes.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StrategyConfig {
    pub id: String,
    pub source_chain_address: String,
    pub dest_chain_address: String,
    pub source_chain: String,
    pub dest_chain: String,
    pub source_asset: StrategyAssetConfig,
    pub dest_asset: StrategyAssetConfig,
    #[serde(default)]
    pub makers: Vec<String>,
    pub min_amount: BigDecimal,
    pub max_amount: BigDecimal,
    pub min_source_timelock: u64,
    pub destination_timelock: u64,
    pub min_source_confirmations: u64,
    pub fee: u64,
    #[serde(default = "default_fixed_fee")]
    pub fixed_fee: BigDecimal,
    #[serde(default)]
    pub max_slippage: u64,
}
