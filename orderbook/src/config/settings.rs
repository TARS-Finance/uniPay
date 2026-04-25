use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// How a venue feed should be polled or subscribed to.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedType {
    Rest { url: String },
    WebSocket { url: String },
}

/// Backoff parameters used by reconnecting market-data workers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct BackoffConfig {
    pub initial_ms: u64,
    pub max_ms: u64,
    pub multiplier: Decimal,
}

/// Maps one venue-native symbol to a canonical pricing asset.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SymbolMapping {
    pub venue_symbol: String,
    pub canonical_asset: String,
}

/// Per-venue market-data config copied from the munger pricing model.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VenueFeedConfig {
    pub venue_id: String,
    #[serde(default)]
    pub feed_types: Vec<FeedType>,
    #[serde(default)]
    pub symbols: Vec<SymbolMapping>,
    pub book_depth: u8,
    pub reconnect_backoff: BackoffConfig,
    #[serde(default)]
    pub weight_override: Option<Decimal>,
}

/// Per-aggregator market-data config copied from the munger pricing model.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AggregatorFeedConfig {
    pub agg_id: String,
    pub api_url: String,
    #[serde(default)]
    pub api_keys: Vec<String>,
    #[serde(default)]
    pub fiat_ids_map: HashMap<String, String>,
    pub base_cooldown_secs: u64,
}

/// Munger-style market-data settings for VWMP computation.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MarketDataSettings {
    pub venue_staleness_threshold_secs: u64,
    pub aggregator_staleness_threshold_secs: u64,
    pub min_active_venues: u8,
    pub outlier_threshold_pct: Decimal,
    pub vwmp_tick_interval_secs: u64,
    pub vol_window_secs: u64,
    pub vol_sample_interval_secs: u64,
    #[serde(default = "default_aggregator_weight")]
    pub aggregator_weight: Decimal,
    #[serde(default)]
    pub venues: HashMap<String, VenueFeedConfig>,
    #[serde(default)]
    pub aggregators: HashMap<String, AggregatorFeedConfig>,
}

impl Default for MarketDataSettings {
    fn default() -> Self {
        Self {
            venue_staleness_threshold_secs: 10,
            aggregator_staleness_threshold_secs: 30,
            min_active_venues: 1,
            outlier_threshold_pct: Decimal::new(5, 0),
            vwmp_tick_interval_secs: 1,
            vol_window_secs: 300,
            vol_sample_interval_secs: 5,
            aggregator_weight: default_aggregator_weight(),
            venues: HashMap::new(),
            aggregators: HashMap::new(),
        }
    }
}

fn default_aggregator_weight() -> Decimal {
    Decimal::new(5, 1)
}

/// Controls how market prices are fetched and overridden.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PricingSettings {
    #[serde(default = "default_refresh_interval_secs")]
    pub refresh_interval_secs: u64,
    #[serde(default = "default_coingecko_api_url")]
    pub coingecko_api_url: String,
    #[serde(default)]
    pub coingecko_api_key: Option<String>,
    #[serde(default)]
    pub static_prices: HashMap<String, f64>,
    #[serde(default)]
    pub asset_canonicals: HashMap<String, String>,
    #[serde(default)]
    pub market_data: MarketDataSettings,
}

impl Default for PricingSettings {
    /// Provides practical defaults for local development.
    fn default() -> Self {
        Self {
            refresh_interval_secs: default_refresh_interval_secs(),
            coingecko_api_url: default_coingecko_api_url(),
            coingecko_api_key: None,
            static_prices: Default::default(),
            asset_canonicals: Default::default(),
            market_data: MarketDataSettings::default(),
        }
    }
}

fn default_refresh_interval_secs() -> u64 {
    30
}

fn default_coingecko_api_url() -> String {
    "https://api.coingecko.com/api/v3/simple/price".to_string()
}

/// Configures quote generation and order signing behavior.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QuoteSettings {
    #[serde(default = "default_deadline_minutes")]
    pub order_deadline_in_minutes: i64,
    #[serde(default)]
    pub quote_private_key: Option<String>,
    #[serde(default)]
    pub max_user_slippage_bps: u64,
    #[serde(default = "default_eta")]
    pub default_eta_seconds: i32,
}

impl Default for QuoteSettings {
    /// Mirrors the default quote behavior expected by the unified service.
    fn default() -> Self {
        Self {
            order_deadline_in_minutes: default_deadline_minutes(),
            quote_private_key: None,
            max_user_slippage_bps: 300,
            default_eta_seconds: default_eta(),
        }
    }
}

/// Default create-order deadline for non-Bitcoin flows.
fn default_deadline_minutes() -> i64 {
    60
}

/// Fallback ETA returned when a route does not provide a chain-specific estimate.
fn default_eta() -> i32 {
    20
}

/// Root service settings loaded from `Settings.toml`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Settings {
    pub addr: String,
    pub db_url: String,
    pub chain_json_path: String,
    pub policy_path: String,
    pub chain_ids: HashMap<String, String>,
    #[serde(default)]
    pub pricing: PricingSettings,
    #[serde(default)]
    pub quote: QuoteSettings,
    #[serde(default)]
    pub discord_webhook_url: Option<String>,
}

impl Settings {
    /// Reads and deserializes the service settings file.
    pub fn from_toml(path: &str) -> eyre::Result<Self> {
        let config = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;
        Ok(config.try_deserialize()?)
    }
}
