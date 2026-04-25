use config::ConfigError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BitcoinSettings {
    pub chain_identifier: String,
    pub network: String,
    pub electrs_url: String,
    pub bitcoind_url: String,
    pub bitcoind_user: String,
    pub bitcoind_pass: String,
    pub database_url: String,
    pub batcher_interval_secs: u64,
    pub default_fee_rate: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    pub pending_orders_url: String,
    pub fiat_provider_url: String,
    pub polling_interval_ms: u64,
    pub bitcoin: BitcoinSettings,
}

impl Settings {
    pub fn from_toml_str(raw: &str) -> Result<Self, ConfigError> {
        config::Config::builder()
            .add_source(config::File::from_str(raw, config::FileFormat::Toml))
            .build()?
            .try_deserialize()
    }

    pub fn try_from_toml(path: &str) -> Result<Self, ConfigError> {
        config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?
            .try_deserialize()
    }

    pub fn executor_btc_private_key(&self) -> Result<String, std::env::VarError> {
        std::env::var("EXECUTOR_BTC_PRIVATE_KEY")
    }
}

#[cfg(test)]
mod tests {
    use super::Settings;

    #[test]
    fn loads_bitcoin_executor_settings() {
        let settings = Settings::from_toml_str(
            r#"
pending_orders_url = "http://127.0.0.1:8080/"
fiat_provider_url = "http://127.0.0.1:8090/"
polling_interval_ms = 1500

[bitcoin]
chain_identifier = "bitcoin"
network = "regtest"
electrs_url = "http://127.0.0.1:30000"
bitcoind_url = "http://127.0.0.1:18443"
bitcoind_user = "admin1"
bitcoind_pass = "123"
database_url = "postgres://postgres:postgres@localhost:5432/postgres"
batcher_interval_secs = 1
default_fee_rate = 2.0
"#,
        )
        .expect("settings");

        assert_eq!(settings.pending_orders_url, "http://127.0.0.1:8080/");
        assert_eq!(settings.polling_interval_ms, 1500);
        assert_eq!(settings.bitcoin.chain_identifier, "bitcoin");
        assert_eq!(settings.bitcoin.network, "regtest");
        assert_eq!(settings.bitcoin.electrs_url, "http://127.0.0.1:30000");
    }
}
