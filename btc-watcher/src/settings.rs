use crate::adapters::ZmqSettings;
use crate::core::ChainSettings;
use tars::utils::OtelTracingConfig;
use serde::Deserialize;

/// Application settings loaded from environment variables or config file.
#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    /// Database configuration
    pub db_url: String,
    /// Deadline buffer in seconds
    pub deadline_buffer_secs: i64,
    /// ZMQ endpoints configuration
    pub zmq: ZmqSettings,
    /// Indexer URL
    pub indexer_url: String,
    /// Chain-specific configuration
    pub chain: ChainSettings,
    /// Otel opts
    pub otel_opts: Option<OtelTracingConfig>,
    /// Screener URL
    pub screener_url: Option<String>,
    /// RPC
    pub rpc: RpcSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcSettings {
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
}
