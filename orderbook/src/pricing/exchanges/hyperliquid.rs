use crate::{
    config::settings::{FeedType, VenueFeedConfig},
    pricing::{service::PricingService, types::PriceLevel},
};
use hyperliquid_rust_sdk::{BaseUrl, BookLevel, InfoClient, L2BookData, Message, Subscription};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use std::{str::FromStr, time::Duration as StdDuration};

const WS_TIMEOUT: StdDuration = StdDuration::from_secs(60);
const VOLUME_POLL_INTERVAL: StdDuration = StdDuration::from_secs(60);
const MAINNET_REST_URL: &str = "https://api.hyperliquid.xyz/info";
const TESTNET_REST_URL: &str = "https://api.hyperliquid-testnet.xyz/info";

#[derive(Debug, Deserialize)]
struct AssetCtx {
    #[serde(rename = "dayNtlVlm", with = "rust_decimal::serde::str")]
    day_ntl_vlm: Decimal,
}

#[derive(Debug, Deserialize)]
struct AssetMeta {
    name: String,
}

#[derive(Debug, Deserialize)]
struct UniverseMeta {
    universe: Vec<AssetMeta>,
}

/// Hyperliquid L2 book feed and volume poller.
pub struct HyperliquidOrderbookFeed {
    service: PricingService,
    cfg: VenueFeedConfig,
}

impl HyperliquidOrderbookFeed {
    pub fn new(service: PricingService, cfg: VenueFeedConfig) -> Self {
        Self { service, cfg }
    }

    pub fn start(self) {
        let use_testnet = is_testnet_from_cfg(&self.cfg);
        let rest_url = if use_testnet {
            TESTNET_REST_URL.to_string()
        } else {
            MAINNET_REST_URL.to_string()
        };
        let coins = self
            .cfg
            .symbols
            .iter()
            .map(|mapping| mapping.venue_symbol.clone())
            .collect::<Vec<_>>();

        let volume_service = self.service.clone();
        let volume_venue_id = self.cfg.venue_id.clone();
        let volume_rest_url = rest_url.clone();
        let volume_coins = coins.clone();
        tokio::spawn(async move {
            run_volume_poll(
                volume_service,
                volume_venue_id,
                volume_rest_url,
                volume_coins,
            )
            .await;
        });

        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(self) {
        let mut backoff_ms = self.cfg.reconnect_backoff.initial_ms.max(100);
        let use_testnet = is_testnet_from_cfg(&self.cfg);

        loop {
            let base_url = if use_testnet {
                BaseUrl::Testnet
            } else {
                BaseUrl::Mainnet
            };
            match self.connect_and_run(base_url).await {
                Ok(()) => {
                    tracing::warn!(venue = %self.cfg.venue_id, "hyperliquid feed ended; reconnecting")
                }
                Err(error) => {
                    tracing::warn!(venue = %self.cfg.venue_id, ?error, "hyperliquid feed error; reconnecting")
                }
            }

            self.service
                .mark_venue_disconnected(&self.cfg.venue_id)
                .await;
            tokio::time::sleep(StdDuration::from_millis(backoff_ms)).await;
            backoff_ms = next_backoff_ms(backoff_ms, &self.cfg.reconnect_backoff);
        }
    }

    async fn connect_and_run(
        &self,
        base_url: BaseUrl,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut client = InfoClient::with_reconnect(None, Some(base_url)).await?;
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel::<Message>();

        for mapping in &self.cfg.symbols {
            client
                .subscribe(
                    Subscription::L2Book {
                        coin: mapping.venue_symbol.clone(),
                    },
                    sender.clone(),
                )
                .await?;
        }

        tracing::info!(venue = %self.cfg.venue_id, symbols = self.cfg.symbols.len(), "hyperliquid feed connected");

        loop {
            let message = match tokio::time::timeout(WS_TIMEOUT, receiver.recv()).await {
                Ok(Some(message)) => message,
                Ok(None) => return Ok(()),
                Err(_) => {
                    tracing::warn!(venue = %self.cfg.venue_id, "hyperliquid receive timeout; reconnecting");
                    return Ok(());
                }
            };

            let book_data: L2BookData = match message {
                Message::L2Book(msg) => msg.data,
                _ => continue,
            };

            let [bids_raw, asks_raw]: [Vec<BookLevel>; 2] = match book_data.levels.try_into() {
                Ok(levels) => levels,
                Err(_) => continue,
            };

            let mut bids = book_levels_to_price_levels(bids_raw);
            let mut asks = book_levels_to_price_levels(asks_raw);
            let depth = usize::from(self.cfg.book_depth);
            bids.sort_by(|left, right| right.price.cmp(&left.price));
            asks.sort_by(|left, right| left.price.cmp(&right.price));
            bids.truncate(depth);
            asks.truncate(depth);

            self.service
                .ingest_order_book(&self.cfg.venue_id, &book_data.coin, bids, asks)
                .await;
        }
    }
}

async fn run_volume_poll(
    service: PricingService,
    venue_id: String,
    rest_url: String,
    coins: Vec<String>,
) {
    let client = match Client::builder()
        .timeout(StdDuration::from_secs(30))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            tracing::warn!(venue = %venue_id, ?error, "failed to build hyperliquid volume client");
            return;
        }
    };

    let mut interval = tokio::time::interval(VOLUME_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        match fetch_meta_and_asset_ctxs(&client, &rest_url).await {
            Ok((meta, ctxs)) => {
                for (asset_meta, ctx) in meta.universe.iter().zip(ctxs.iter()) {
                    if coins.contains(&asset_meta.name) {
                        service
                            .ingest_volume_24h(&venue_id, &asset_meta.name, ctx.day_ntl_vlm)
                            .await;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(venue = %venue_id, ?error, "failed to fetch hyperliquid metaAndAssetCtxs")
            }
        }
    }
}

async fn fetch_meta_and_asset_ctxs(
    client: &Client,
    rest_url: &str,
) -> Result<(UniverseMeta, Vec<AssetCtx>), reqwest::Error> {
    let response: serde_json::Value = client
        .post(rest_url)
        .json(&serde_json::json!({ "type": "metaAndAssetCtxs" }))
        .send()
        .await?
        .json()
        .await?;

    let meta =
        serde_json::from_value(response[0].clone()).unwrap_or(UniverseMeta { universe: vec![] });
    let ctxs = serde_json::from_value(response[1].clone()).unwrap_or_default();
    Ok((meta, ctxs))
}

fn book_levels_to_price_levels(levels: Vec<BookLevel>) -> Vec<PriceLevel> {
    levels
        .into_iter()
        .filter_map(|level| {
            let price = Decimal::from_str(&level.px).ok()?;
            let quantity = Decimal::from_str(&level.sz).ok()?;
            Some(PriceLevel { price, quantity })
        })
        .collect()
}

fn is_testnet_from_cfg(cfg: &VenueFeedConfig) -> bool {
    cfg.feed_types.iter().any(|feed_type| match feed_type {
        FeedType::WebSocket { url } | FeedType::Rest { url } => url.contains("testnet"),
    })
}

fn next_backoff_ms(current_ms: u64, cfg: &crate::config::settings::BackoffConfig) -> u64 {
    let min_ms = cfg.initial_ms.max(100);
    let max_ms = cfg.max_ms.max(min_ms);
    let multiplier = cfg.multiplier.to_f64().unwrap_or(2.0).max(1.0);
    let grown = (current_ms as f64 * multiplier).round();
    let grown = if grown.is_finite() && grown > 0.0 {
        grown as u64
    } else {
        current_ms
    };
    grown.clamp(min_ms, max_ms)
}
