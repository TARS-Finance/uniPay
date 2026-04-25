use std::collections::HashMap;

use config::{Config, File};
use serde::{Deserialize, Serialize};
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
/// Global settings for the app.
pub struct Settings {
    /// `db_url` is the connection string for the database.
    ///
    /// For example, `postgres://user:password@localhost/dbname`.
    pub db_url: String,
    /// pooling interval for multiwatcher in seconds
    pub multiwatcher_polling_interval: u64,
    /// deadline buffer for swap store
    pub deadline_buffer: i64,
    /// ignore chains for swap store
    pub chains: Vec<ChainSettings>,
    /// ignore chains for swap store
    pub ignore_chains: Vec<String>,
}

#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
pub struct ChainSettings {
    pub name: String,
    pub rpc_url: String,
    pub multicall_address: String,
    /// polling interval for watcher in seconds
    pub polling_interval: u64,
    /// supported assets for watcher
    pub supported_assets: HashMap<String, Vec<String>>,
    pub max_block_span: u64,
}

impl Settings {
    /// Load settings from environment variables.
    /// Will panic if any required environment variables are missing.
    pub fn from_toml(path: &str) -> Self {
        let config = Config::builder()
            .add_source(File::with_name(path))
            .build()
            .unwrap();
        config.try_deserialize().unwrap()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_settings() {
        let settings = super::Settings::from_toml("Settings.toml");
        println!("{:#?}", settings);
    }
}
