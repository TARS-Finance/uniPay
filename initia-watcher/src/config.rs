use serde::Deserialize;
use std::fs;

use crate::errors::{Result, WatcherError};

pub const DEFAULT_INTERVAL_MS: u64 = 2000;
pub const DEFAULT_BLOCK_SPAN: u64 = 1000;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub database_url: String,
    pub watcher_interval_ms: Option<u64>,
    pub watcher_block_span: Option<u64>,
    pub chains: Vec<ChainConfig>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChainConfig {
    pub name: String,
    pub chain_id: u64,
    pub rpc_url: String,
    pub pairs: Vec<PairConfig>,
}

/// One HTLC watcher is spawned per pair per chain.
#[derive(Debug, Deserialize, Clone)]
pub struct PairConfig {
    pub token_address: String,
    pub htlc_address: String,
}

impl Config {
    pub fn load(path: &str) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .map_err(|e| WatcherError::Config(format!("Cannot read {path}: {e}")))?;
        toml::from_str(&contents)
            .map_err(|e| WatcherError::Config(format!("Invalid TOML in {path}: {e}")))
    }

    pub fn interval_ms(&self) -> u64 {
        self.watcher_interval_ms.unwrap_or(DEFAULT_INTERVAL_MS)
    }

    pub fn block_span(&self) -> u64 {
        self.watcher_block_span.unwrap_or(DEFAULT_BLOCK_SPAN)
    }
}
