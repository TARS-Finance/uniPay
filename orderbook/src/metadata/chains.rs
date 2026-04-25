use serde::{Deserialize, Serialize};
use tars::primitives::{ContractInfo, HTLCVersion};

/// External market-data identifiers attached to an asset definition.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawTokenIds {
    pub coingecko: Option<String>,
    pub aggregate: Option<String>,
    pub cmc: Option<String>,
}

/// Raw asset record from `chain.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawAsset {
    pub id: String,
    pub name: String,
    pub chain: String,
    pub icon: String,
    pub htlc: Option<ContractInfo>,
    pub token: Option<ContractInfo>,
    pub decimals: u8,
    pub min_amount: String,
    pub max_amount: String,
    pub chain_icon: String,
    pub chain_id: Option<String>,
    pub chain_type: String,
    pub version: Option<HTLCVersion>,
    pub explorer_url: String,
    pub min_timelock: u64,
    pub token_ids: Option<RawTokenIds>,
    pub solver: String,
}

/// Raw chain record from `chain.json`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RawChain {
    pub chain: String,
    pub id: String,
    pub icon: String,
    pub explorer_url: String,
    pub confirmation_target: u64,
    pub source_timelock: String,
    pub destination_timelock: String,
    #[serde(default)]
    pub supported_htlc_schemas: Vec<String>,
    #[serde(default)]
    pub supported_token_schemas: Vec<String>,
    #[serde(default)]
    pub assets: Vec<RawAsset>,
}
