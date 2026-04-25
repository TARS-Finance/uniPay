use config::ConfigError;
use serde::{Deserialize, Serialize};

/// AES secret key for decrypting the encrypted environment variables
const AES_SECRET_KEY: &str = "cb8c16717909e9e463231468d3ba8058a7d753764e6b129a12c4d5d00ce592ac";

// Configuration for a chain
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ChainSettings {
    // RPC URL for the chain
    pub rpc_url: String,
    // Multicall contract address for the chain
    pub multicall_address: String,
    // Chain identifier for the chain
    pub chain_identifier: String,
    // Polling interval for the executor
    pub polling_interval: u64,
    // Transaction timeout in milliseconds
    pub transaction_timeout: u64,
}

// Configuration for the executor
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Settings {
    // Discord webhook URL for logging errors
    #[serde(skip_serializing_if = "Option::is_none")]
    pub discord_webhook_url: Option<String>,
    // List of chains that executor supports
    pub chains: Vec<ChainSettings>,
    // Price provider URL
    pub fiat_provider_url: String,
    // Pending orders provider URL
    pub pending_orders_url: String,
    // Private key for all executors
    #[serde(deserialize_with = "tars::utils::deserialize_env_field")]
    pub private_key: String,
}

impl Settings {
    /// Attempts to load settings from a TOML file, returning a Result.
    pub fn try_from_toml(path: &str) -> Result<Self, ConfigError> {
        unsafe {
            std::env::set_var("AES_SECRET_KEY", AES_SECRET_KEY);
        }

        let config = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;

        config.try_deserialize()
    }
}
