use crate::{
    config::settings::{AggregatorFeedConfig, PricingSettings, VenueFeedConfig},
    metadata::MetadataIndex,
    pricing::{
        aggregators::{cmc::CmcPriceFetcher, coingecko::CoingeckoPriceFetcher},
        computation::{
            VwmpSample, compute_realized_volatility, depth_usd, snapshot_from_samples,
            within_outlier_threshold,
        },
        exchanges::{binance::BinanceOrderbookFeed, hyperliquid::HyperliquidOrderbookFeed},
        mapping::PricingMapping,
        types::{
            AggregatorPriceSnapshot, MarketState, MidPriceSnapshot, OrderBook, PriceLevel,
            VenueHealth, VolumeSnapshot,
        },
    },
};
use chrono::Utc;
use moka::future::Cache;
use rust_decimal::Decimal;
use rust_decimal::prelude::{FromPrimitive, ToPrimitive};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};

const ORDER_BOOK_CACHE_CAPACITY: u64 = 10_000;
const VOLUME_24H_CACHE_CAPACITY: u64 = 10_000;
const AGGREGATOR_PRICES_CACHE_CAPACITY: u64 = 10_000;
const SNAPSHOT_CACHE_CAPACITY: u64 = 2_048;
const VOLATILITY_CACHE_CAPACITY: u64 = 2_048;
const VWMP_HISTORY_CACHE_CAPACITY: u64 = 2_048;
const PEG_DEVIATION_CACHE_CAPACITY: u64 = 8_192;
const VENUE_HEALTH_CACHE_CAPACITY: u64 = 256;

/// Munger-style market-data service that backs `price_for(asset_id)` with computed snapshots.
#[derive(Clone)]
pub struct PricingService {
    settings: PricingSettings,
    metadata: Arc<MetadataIndex>,
    mapping: PricingMapping,
    order_books: Cache<(String, String), OrderBook>,
    volume_24h: Cache<(String, String), VolumeSnapshot>,
    aggregator_prices: Cache<(String, String), AggregatorPriceSnapshot>,
    vwmp_snapshots: Cache<String, MidPriceSnapshot>,
    volatilities: Cache<String, Decimal>,
    vwmp_history: Cache<String, VecDeque<(chrono::DateTime<Utc>, Decimal)>>,
    peg_deviations: Cache<String, Decimal>,
    venue_health: Cache<String, VenueHealth>,
}

impl PricingService {
    /// Creates the market-data service and its in-memory caches.
    pub fn new(settings: PricingSettings, metadata: Arc<MetadataIndex>) -> Self {
        let mapping = PricingMapping::new(&settings, metadata.clone());
        Self {
            settings,
            metadata,
            mapping,
            order_books: Cache::new(ORDER_BOOK_CACHE_CAPACITY),
            volume_24h: Cache::new(VOLUME_24H_CACHE_CAPACITY),
            aggregator_prices: Cache::new(AGGREGATOR_PRICES_CACHE_CAPACITY),
            vwmp_snapshots: Cache::new(SNAPSHOT_CACHE_CAPACITY),
            volatilities: Cache::new(VOLATILITY_CACHE_CAPACITY),
            vwmp_history: Cache::new(VWMP_HISTORY_CACHE_CAPACITY),
            peg_deviations: Cache::new(PEG_DEVIATION_CACHE_CAPACITY),
            venue_health: Cache::new(VENUE_HEALTH_CACHE_CAPACITY),
        }
    }

    /// Starts the background workers used to maintain current market state.
    pub fn start(self: &Arc<Self>) {
        self.ingest_static_prices();
        self.spawn_venue_workers();
        self.spawn_aggregator_workers();
        self.spawn_vwmp_snapshot_worker();
        self.spawn_staleness_refresh_worker();
    }

    /// Returns the computed canonical mid price for a local asset.
    pub async fn price_for(&self, asset_id: &str) -> Option<f64> {
        self.snapshot_for(asset_id)
            .await
            .and_then(|snapshot| snapshot.vwmp.to_f64())
    }

    /// Returns the richer pricing snapshot for a local asset.
    pub async fn snapshot_for(&self, asset_id: &str) -> Option<MidPriceSnapshot> {
        let canonical = self.mapping.canonical_or_asset(asset_id);
        self.vwmp_snapshots.get(&canonical).await
    }

    /// Returns the full current market state assembled from service caches.
    pub async fn market_state(&self) -> MarketState {
        let mid_prices = self
            .vwmp_snapshots
            .iter()
            .map(|(key, value)| (key.as_ref().clone(), value))
            .collect();
        let volatilities = self
            .volatilities
            .iter()
            .map(|(key, value)| (key.as_ref().clone(), value))
            .collect();
        let peg_deviations = self
            .peg_deviations
            .iter()
            .map(|(key, value)| (key.as_ref().clone(), value))
            .collect();

        MarketState {
            mid_prices,
            volatilities,
            peg_deviations,
        }
    }

    /// Returns whether the canonical snapshot for a local asset is currently healthy.
    pub async fn is_healthy(&self, asset_id: &str) -> bool {
        self.snapshot_for(asset_id)
            .await
            .map(|snapshot| snapshot.staleness_ok)
            .unwrap_or(false)
    }

    /// Ingests an order-book snapshot for one venue symbol.
    pub async fn ingest_order_book(
        &self,
        venue: &str,
        symbol: &str,
        bids: Vec<PriceLevel>,
        asks: Vec<PriceLevel>,
    ) {
        self.order_books
            .insert(
                (venue.to_string(), symbol.to_string()),
                OrderBook {
                    venue: venue.to_string(),
                    symbol: symbol.to_string(),
                    bids,
                    asks,
                    last_updated: Utc::now(),
                    is_stale: false,
                },
            )
            .await;

        self.mark_venue_connected(venue).await;
        self.refresh_vwmp_for_venue_symbol(venue, symbol).await;
    }

    /// Ingests 24h volume for one venue symbol.
    pub async fn ingest_volume_24h(&self, venue: &str, symbol: &str, volume_usd: Decimal) {
        self.volume_24h
            .insert(
                (venue.to_string(), symbol.to_string()),
                VolumeSnapshot {
                    volume_usd,
                    last_updated: Utc::now(),
                },
            )
            .await;
        self.refresh_vwmp_for_venue_symbol(venue, symbol).await;
    }

    /// Ingests one aggregator price for a canonical asset.
    pub async fn ingest_aggregator_price(&self, aggregator: &str, canonical: &str, price: Decimal) {
        self.aggregator_prices
            .insert(
                (aggregator.to_string(), canonical.to_string()),
                AggregatorPriceSnapshot {
                    price,
                    fetched_at: Utc::now(),
                },
            )
            .await;
        self.refresh_vwmp_for_canonical(canonical).await;
    }

    fn ingest_static_prices(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            for (asset_id, price) in &service.settings.static_prices {
                if let Some(decimal_price) = Decimal::from_f64(*price) {
                    let canonical = service.mapping.canonical_or_asset(asset_id);
                    service
                        .ingest_aggregator_price("static", &canonical, decimal_price)
                        .await;
                }
            }
        });
    }

    fn spawn_aggregator_workers(self: &Arc<Self>) {
        let service = self.clone();
        let aggregators = if service.settings.market_data.aggregators.is_empty() {
            service.legacy_aggregators_from_metadata()
        } else {
            service.settings.market_data.aggregators.clone()
        };

        for aggregator in aggregators.values() {
            match aggregator.agg_id.as_str() {
                "coingecko" => {
                    if let Ok(fetcher) = CoingeckoPriceFetcher::new(
                        aggregator.api_url.clone(),
                        aggregator.api_keys.clone(),
                        service.as_ref().clone(),
                        aggregator.fiat_ids_map.clone(),
                        aggregator.base_cooldown_secs,
                    ) {
                        fetcher.start();
                    }
                }
                "cmc" => {
                    if let Ok(fetcher) = CmcPriceFetcher::new(
                        aggregator.api_url.clone(),
                        aggregator.api_keys.clone(),
                        service.as_ref().clone(),
                        aggregator.fiat_ids_map.clone(),
                        aggregator.base_cooldown_secs,
                    ) {
                        fetcher.start();
                    }
                }
                other => tracing::warn!(aggregator = other, "unsupported pricing aggregator"),
            }
        }
    }

    fn spawn_venue_workers(self: &Arc<Self>) {
        for venue_cfg in self.settings.market_data.venues.values() {
            match venue_cfg.venue_id.as_str() {
                "binance" => {
                    BinanceOrderbookFeed::new(self.as_ref().clone(), venue_cfg.clone()).start()
                }
                "hyperliquid" => {
                    HyperliquidOrderbookFeed::new(self.as_ref().clone(), venue_cfg.clone()).start()
                }
                other => tracing::warn!(venue = other, "unsupported pricing venue"),
            }
        }
    }

    fn spawn_vwmp_snapshot_worker(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(
                service.settings.market_data.vwmp_tick_interval_secs.max(1),
            ));
            let canonical_assets = service.canonical_assets_from_config();

            loop {
                interval.tick().await;
                for canonical in &canonical_assets {
                    service.refresh_vwmp_for_canonical(canonical).await;
                }
            }
        });
    }

    fn spawn_staleness_refresh_worker(self: &Arc<Self>) {
        let service = self.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            loop {
                interval.tick().await;
                service.refresh_staleness().await;
            }
        });
    }

    async fn refresh_staleness(&self) {
        let now = Utc::now();
        let mut fresh_venues = HashSet::new();
        let mut updated_books = Vec::new();

        for (key, mut book) in self.order_books.iter() {
            let cache_key = key.as_ref().clone();
            let stale = (now - book.last_updated).num_seconds()
                > self.settings.market_data.venue_staleness_threshold_secs as i64;
            if !stale {
                fresh_venues.insert(cache_key.0.clone());
            }
            if book.is_stale != stale {
                book.is_stale = stale;
                updated_books.push((cache_key, book));
            }
        }

        for (cache_key, book) in updated_books {
            self.order_books.insert(cache_key, book).await;
        }

        let updated_venues = self
            .venue_health
            .iter()
            .filter_map(|(key, mut health)| {
                let venue = key.as_ref().clone();
                let should_be_stale = !fresh_venues.contains(&venue);
                if health.stale == should_be_stale {
                    None
                } else {
                    health.stale = should_be_stale;
                    Some((venue, health))
                }
            })
            .collect::<Vec<_>>();

        for (venue, health) in updated_venues {
            self.venue_health.insert(venue, health).await;
        }
    }

    async fn mark_venue_connected(&self, venue: &str) {
        let mut entry = self
            .venue_health
            .get(&venue.to_string())
            .await
            .unwrap_or_else(|| VenueHealth {
                venue: venue.to_string(),
                connected: false,
                stale: true,
                last_message_at: None,
                reconnection_count: 0,
                sequence_gaps: 0,
            });

        entry.connected = true;
        entry.stale = false;
        entry.last_message_at = Some(Utc::now());
        self.venue_health.insert(venue.to_string(), entry).await;
    }

    /// Marks a venue disconnected and immediately stales its cached books.
    pub(crate) async fn mark_venue_disconnected(&self, venue: &str) {
        let mut updated_books = Vec::new();
        for (key, mut book) in self.order_books.iter() {
            let cache_key = key.as_ref().clone();
            if cache_key.0 == venue && !book.is_stale {
                book.is_stale = true;
                updated_books.push((cache_key, book));
            }
        }
        for (cache_key, book) in updated_books {
            self.order_books.insert(cache_key, book).await;
        }

        let mut entry = self
            .venue_health
            .get(&venue.to_string())
            .await
            .unwrap_or_else(|| VenueHealth {
                venue: venue.to_string(),
                connected: false,
                stale: true,
                last_message_at: None,
                reconnection_count: 0,
                sequence_gaps: 0,
            });
        entry.connected = false;
        entry.stale = true;
        entry.reconnection_count = entry.reconnection_count.saturating_add(1);
        self.venue_health.insert(venue.to_string(), entry).await;
    }

    async fn refresh_vwmp_for_venue_symbol(&self, venue: &str, symbol: &str) {
        let canonicals = self
            .settings
            .market_data
            .venues
            .values()
            .filter(|config| config.venue_id == venue)
            .flat_map(|config| config.symbols.iter())
            .filter(|mapping| mapping.venue_symbol == symbol)
            .map(|mapping| mapping.canonical_asset.clone())
            .collect::<Vec<_>>();

        for canonical in canonicals {
            self.refresh_vwmp_for_canonical(&canonical).await;
        }
    }

    async fn refresh_vwmp_for_canonical(&self, canonical: &str) {
        if let Some(snapshot) = self.compute_vwmp(canonical).await {
            self.vwmp_snapshots
                .insert(canonical.to_string(), snapshot.clone())
                .await;
            if snapshot.staleness_ok {
                self.update_vwmp_history_and_volatility(canonical, snapshot.vwmp)
                    .await;
            } else {
                self.volatilities.invalidate(&canonical.to_string()).await;
            }
        } else {
            self.vwmp_snapshots.invalidate(&canonical.to_string()).await;
            self.volatilities.invalidate(&canonical.to_string()).await;
        }
    }

    async fn update_vwmp_history_and_volatility(&self, canonical: &str, vwmp: Decimal) {
        let now = Utc::now();
        let cutoff =
            now - chrono::TimeDelta::seconds(self.settings.market_data.vol_window_secs as i64);
        let mut history = self
            .vwmp_history
            .get(&canonical.to_string())
            .await
            .unwrap_or_default();

        let should_sample = history
            .back()
            .map(|(timestamp, _)| {
                (now - *timestamp).num_seconds()
                    >= self.settings.market_data.vol_sample_interval_secs as i64
            })
            .unwrap_or(true);
        if should_sample {
            history.push_back((now, vwmp));
        }

        while history
            .front()
            .map(|(timestamp, _)| *timestamp < cutoff)
            .unwrap_or(false)
        {
            history.pop_front();
        }
        self.vwmp_history
            .insert(canonical.to_string(), history.clone())
            .await;

        if let Some(volatility) = compute_realized_volatility(&history, &self.settings.market_data)
        {
            self.volatilities
                .insert(canonical.to_string(), volatility)
                .await;
        }
    }

    async fn compute_vwmp(&self, canonical: &str) -> Option<MidPriceSnapshot> {
        let mut samples = Vec::new();
        let now = Utc::now();

        for venue_cfg in self.settings.market_data.venues.values() {
            self.push_cex_samples(&mut samples, canonical, venue_cfg, now)
                .await;
        }
        self.push_aggregator_samples(&mut samples, canonical, now)
            .await;

        if samples.is_empty() {
            return None;
        }

        let mut mids = samples.iter().map(|sample| sample.mid).collect::<Vec<_>>();
        mids.sort_unstable();
        let median = if mids.len() % 2 == 1 {
            mids[mids.len() / 2]
        } else {
            (mids[mids.len() / 2 - 1] + mids[mids.len() / 2]) / Decimal::new(2, 0)
        };
        samples.retain(|sample| {
            within_outlier_threshold(
                sample.mid,
                median,
                self.settings.market_data.outlier_threshold_pct,
            )
        });

        snapshot_from_samples(&samples, self.settings.market_data.min_active_venues, now)
    }

    async fn push_cex_samples(
        &self,
        samples: &mut Vec<VwmpSample>,
        canonical: &str,
        venue_cfg: &VenueFeedConfig,
        now: chrono::DateTime<Utc>,
    ) {
        for mapping in &venue_cfg.symbols {
            if mapping.canonical_asset != canonical {
                continue;
            }

            let Some(book) = self
                .order_books
                .get(&(venue_cfg.venue_id.clone(), mapping.venue_symbol.clone()))
                .await
            else {
                continue;
            };
            if book.is_stale {
                continue;
            }

            let Some(best_bid) = book.bids.first().map(|level| level.price) else {
                continue;
            };
            let Some(best_ask) = book.asks.first().map(|level| level.price) else {
                continue;
            };
            if best_bid <= Decimal::ZERO || best_ask <= Decimal::ZERO || best_bid > best_ask {
                continue;
            }

            let weight = if let Some(override_weight) = venue_cfg.weight_override {
                override_weight
            } else {
                let key = (venue_cfg.venue_id.clone(), mapping.venue_symbol.clone());
                match self.volume_24h.get(&key).await {
                    Some(snapshot)
                        if (now - snapshot.last_updated).num_seconds()
                            <= self.settings.market_data.venue_staleness_threshold_secs as i64
                            && snapshot.volume_usd > Decimal::ZERO =>
                    {
                        snapshot.volume_usd
                    }
                    _ => continue,
                }
            };

            samples.push(VwmpSample {
                mid: (best_bid + best_ask) / Decimal::new(2, 0),
                best_bid,
                best_ask,
                bid_depth_usd: depth_usd(&book.bids, usize::from(venue_cfg.book_depth)),
                ask_depth_usd: depth_usd(&book.asks, usize::from(venue_cfg.book_depth)),
                weight,
                is_cex: true,
            });
        }
    }

    async fn push_aggregator_samples(
        &self,
        samples: &mut Vec<VwmpSample>,
        canonical: &str,
        now: chrono::DateTime<Utc>,
    ) {
        for (key, snapshot) in self.aggregator_prices.iter() {
            if key.as_ref().1 != canonical {
                continue;
            }
            if (now - snapshot.fetched_at).num_seconds()
                > self
                    .settings
                    .market_data
                    .aggregator_staleness_threshold_secs as i64
            {
                continue;
            }
            if snapshot.price <= Decimal::ZERO
                || self.settings.market_data.aggregator_weight <= Decimal::ZERO
            {
                continue;
            }

            samples.push(VwmpSample {
                mid: snapshot.price,
                best_bid: Decimal::ZERO,
                best_ask: Decimal::ZERO,
                bid_depth_usd: Decimal::ZERO,
                ask_depth_usd: Decimal::ZERO,
                weight: self.settings.market_data.aggregator_weight,
                is_cex: false,
            });
        }
    }

    fn canonical_assets_from_config(&self) -> Vec<String> {
        let mut canonicals = HashSet::new();
        for venue_cfg in self.settings.market_data.venues.values() {
            for mapping in &venue_cfg.symbols {
                canonicals.insert(mapping.canonical_asset.clone());
            }
        }
        for aggregator_cfg in self.settings.market_data.aggregators.values() {
            for canonical in aggregator_cfg.fiat_ids_map.keys() {
                canonicals.insert(canonical.clone());
            }
        }
        for asset in &self.metadata.assets {
            canonicals.insert(self.mapping.canonical_or_asset(&asset.asset.id.to_string()));
        }
        canonicals.into_iter().collect()
    }

    fn legacy_aggregators_from_metadata(&self) -> HashMap<String, AggregatorFeedConfig> {
        if self.settings.coingecko_api_url.is_empty() {
            return HashMap::new();
        }

        let mut fiat_ids_map = HashMap::new();
        for asset in &self.metadata.assets {
            let Some(coingecko_id) = asset.coingecko_id.clone() else {
                continue;
            };
            fiat_ids_map
                .entry(self.mapping.canonical_or_asset(&asset.asset.id.to_string()))
                .or_insert(coingecko_id);
        }

        HashMap::from([(
            "coingecko".to_string(),
            AggregatorFeedConfig {
                agg_id: "coingecko".to_string(),
                api_url: self.settings.coingecko_api_url.clone(),
                api_keys: self
                    .settings
                    .coingecko_api_key
                    .clone()
                    .map(|value| vec![value])
                    .unwrap_or_default(),
                fiat_ids_map,
                base_cooldown_secs: self.settings.refresh_interval_secs.max(1),
            },
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::settings::MarketDataSettings,
        metadata::chains::{RawAsset, RawChain, RawTokenIds},
    };
    use std::fs;
    use tars::primitives::{ContractInfo, HTLCVersion};

    fn primary_contract() -> ContractInfo {
        ContractInfo {
            address: "primary".to_string(),
            schema: Some("primary".to_string()),
        }
    }

    fn test_metadata() -> Arc<MetadataIndex> {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("chain.json");
        let chains = vec![RawChain {
            chain: "bitcoin_testnet".to_string(),
            id: "bitcoin".to_string(),
            icon: "icon".to_string(),
            explorer_url: "explorer".to_string(),
            confirmation_target: 1,
            source_timelock: "12".to_string(),
            destination_timelock: "6".to_string(),
            supported_htlc_schemas: vec!["primary".to_string()],
            supported_token_schemas: vec!["primary".to_string()],
            assets: vec![RawAsset {
                id: "bitcoin_testnet:btc".to_string(),
                name: "Bitcoin".to_string(),
                chain: "bitcoin_testnet".to_string(),
                icon: "icon".to_string(),
                htlc: Some(primary_contract()),
                token: Some(primary_contract()),
                decimals: 8,
                min_amount: "1".to_string(),
                max_amount: "100".to_string(),
                chain_icon: "icon".to_string(),
                chain_id: None,
                chain_type: "bitcoin".to_string(),
                version: Some(HTLCVersion::V2),
                explorer_url: "explorer".to_string(),
                min_timelock: 12,
                token_ids: Some(RawTokenIds {
                    coingecko: Some("bitcoin".to_string()),
                    aggregate: Some("BTC".to_string()),
                    cmc: Some("1".to_string()),
                }),
                solver: "solver".to_string(),
            }],
        }];

        fs::write(&path, serde_json::to_string(&chains).unwrap()).unwrap();
        Arc::new(MetadataIndex::load(path.to_str().unwrap()).unwrap())
    }

    #[tokio::test]
    async fn price_for_returns_canonical_mid_price() {
        let metadata = test_metadata();
        let settings = PricingSettings {
            refresh_interval_secs: 30,
            coingecko_api_url: "http://localhost".to_string(),
            coingecko_api_key: None,
            static_prices: HashMap::new(),
            asset_canonicals: HashMap::from([(
                "bitcoin_testnet:btc".to_string(),
                "BTC".to_string(),
            )]),
            market_data: MarketDataSettings::default(),
        };
        let service = PricingService::new(settings, metadata);
        service
            .ingest_aggregator_price("coingecko", "BTC", Decimal::new(90000, 0))
            .await;

        let price = service.price_for("bitcoin_testnet:btc").await.unwrap();
        assert_eq!(price, 90000.0);
        let snapshot = service.snapshot_for("bitcoin_testnet:btc").await.unwrap();
        assert_eq!(snapshot.vwmp, Decimal::new(90000, 0));
        assert!(snapshot.aggregator_only);
    }
}
