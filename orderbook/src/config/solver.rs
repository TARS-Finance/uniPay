use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Per-chain solver runtime details used by the liquidity watcher.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ChainConfig {
    pub rpc_url: String,
    pub native_decimals: u8,
    pub native_asset_id: String,
    pub address: String,
    #[serde(default)]
    pub solver_account: Option<String>,
    #[serde(default)]
    pub supported_assets: Vec<String>,
}

impl ChainConfig {
    /// Returns the address that should be queried for on-chain balances.
    pub fn liquidity_account(&self) -> &str {
        &self.address
    }

    /// Returns the identity used by order creation and committed-funds lookups.
    pub fn order_identity(&self) -> &str {
        self.solver_account.as_deref().unwrap_or(&self.address)
    }

    /// Backwards-compatible alias for older call sites.
    pub fn solver_account(&self) -> &str {
        self.order_identity()
    }
}

/// Top-level solver settings for the in-process liquidity subsystem.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SolverSettings {
    pub solver_id: String,
    pub solver_name: String,
    pub polling_interval_ms: u64,
    #[serde(default = "default_cache_ttl")]
    pub cache_ttl_ms: u64,
    pub chains: HashMap<String, ChainConfig>,
}

/// Default freshness target for liquidity snapshots.
fn default_cache_ttl() -> u64 {
    10_000
}
