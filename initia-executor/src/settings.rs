use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize, Clone)]
pub struct Settings {
    pub initia: InitiaSettings,
    pub database: DatabaseSettings,
    pub solver_orders_url: String,
    pub fiat_provider_url: String,
    #[serde(default = "default_polling_interval")]
    pub polling_interval_ms: u64,
}

fn default_polling_interval() -> u64 {
    3000
}

#[derive(Debug, Deserialize, Clone)]
pub struct InitiaSettings {
    pub chain_name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    /// NativeHTLC contract address (0x0000...0000 token sentinel)
    pub native_htlc_address: String,
    /// ERC20 HTLC pairs: each maps a token address to its HTLC contract
    #[serde(default)]
    pub erc20_pairs: Vec<Erc20Pair>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Erc20Pair {
    pub token_address: String,
    pub htlc_address: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseSettings {
    pub db_url: String,
}

impl Settings {
    pub fn load(path: &str) -> eyre::Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| eyre::eyre!("Cannot read config {path}: {e}"))?;
        toml::from_str(&contents).map_err(|e| eyre::eyre!("Invalid config TOML: {e}"))
    }
}
