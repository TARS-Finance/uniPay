use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Public USD price view for a single local asset.
#[derive(Debug, Clone, Serialize)]
pub struct AssetPrice {
    pub asset_id: String,
    pub usd_price: f64,
}

/// Snapshot of the current market state used by pricing.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MarketState {
    pub mid_prices: HashMap<String, MidPriceSnapshot>,
    pub volatilities: HashMap<String, Decimal>,
    pub peg_deviations: HashMap<String, Decimal>,
}

/// Mid-price snapshot following the richer munger pricing model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MidPriceSnapshot {
    pub vwmp: Decimal,
    pub best_bid: Decimal,
    pub best_ask: Decimal,
    pub bid_depth_usd: Decimal,
    pub ask_depth_usd: Decimal,
    pub active_venue_count: u8,
    pub computed_at: DateTime<Utc>,
    pub staleness_ok: bool,
    pub aggregator_only: bool,
}

/// One order-book level used for VWMP and depth calculations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceLevel {
    pub price: Decimal,
    pub quantity: Decimal,
}

/// In-memory order-book snapshot for one venue symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderBook {
    pub venue: String,
    pub symbol: String,
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
    pub last_updated: DateTime<Utc>,
    pub is_stale: bool,
}

/// Cached 24h notional volume for one venue symbol.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSnapshot {
    pub volume_usd: Decimal,
    pub last_updated: DateTime<Utc>,
}

/// Cached aggregator price for one canonical asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatorPriceSnapshot {
    pub price: Decimal,
    pub fetched_at: DateTime<Utc>,
}

/// Health metadata for a venue worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VenueHealth {
    pub venue: String,
    pub connected: bool,
    pub stale: bool,
    pub last_message_at: Option<DateTime<Utc>>,
    pub reconnection_count: u32,
    pub sequence_gaps: u32,
}
