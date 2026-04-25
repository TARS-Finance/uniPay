use crate::{
    config::settings::{FeedType, VenueFeedConfig},
    pricing::{service::PricingService, types::PriceLevel},
};
use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Deserialize;
use serde_json::json;
use std::{collections::HashMap, time::Duration as StdDuration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

const MAINNET_WS_URL: &str = "wss://fstream.binance.com/stream";
const MAINNET_REST_BASE: &str = "https://fapi.binance.com";
const WS_TIMEOUT: StdDuration = StdDuration::from_secs(10);
const VOLUME_POLL_INTERVAL: StdDuration = StdDuration::from_secs(30);

/// Binance Futures order-book feed and volume poller.
pub struct BinanceOrderbookFeed {
    service: PricingService,
    cfg: VenueFeedConfig,
    client: Client,
}

struct LocalBook {
    last_update_id: u64,
    bids: Vec<PriceLevel>,
    asks: Vec<PriceLevel>,
    just_rebuilt: bool,
}

impl LocalBook {
    fn new() -> Self {
        Self {
            last_update_id: 0,
            bids: Vec::new(),
            asks: Vec::new(),
            just_rebuilt: false,
        }
    }

    fn reset(&mut self) {
        self.last_update_id = 0;
        self.bids.clear();
        self.asks.clear();
        self.just_rebuilt = false;
    }
}

#[allow(non_snake_case)]
#[derive(Debug, Deserialize)]
struct DepthUpdate {
    s: String,
    U: u64,
    u: u64,
    pu: u64,
    b: Vec<(Decimal, Decimal)>,
    a: Vec<(Decimal, Decimal)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DepthSnapshot {
    last_update_id: u64,
    bids: Vec<(Decimal, Decimal)>,
    asks: Vec<(Decimal, Decimal)>,
}

#[derive(Debug, Deserialize)]
struct Ticker24hr {
    symbol: String,
    #[serde(with = "rust_decimal::serde::str")]
    volume: Decimal,
}

impl BinanceOrderbookFeed {
    pub fn new(service: PricingService, cfg: VenueFeedConfig) -> Self {
        let client = Client::builder()
            .timeout(StdDuration::from_secs(30))
            .build()
            .expect("reqwest client build must not fail");
        Self {
            service,
            cfg,
            client,
        }
    }

    pub fn start(self) {
        let rest_base = rest_base_url_from_cfg(&self.cfg);
        let symbols = self
            .cfg
            .symbols
            .iter()
            .map(|mapping| mapping.venue_symbol.clone())
            .collect::<Vec<_>>();

        let volume_service = self.service.clone();
        let volume_client = self.client.clone();
        let volume_venue_id = self.cfg.venue_id.clone();
        let volume_rest_base = rest_base.clone();
        let volume_symbols = symbols.clone();
        tokio::spawn(async move {
            run_volume_poll(
                volume_service,
                volume_client,
                volume_venue_id,
                volume_rest_base,
                volume_symbols,
            )
            .await;
        });

        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(self) {
        let mut backoff_ms = self.cfg.reconnect_backoff.initial_ms.max(100);
        let ws_url = ws_url_from_cfg(&self.cfg);
        let rest_base = rest_base_url_from_cfg(&self.cfg);
        let symbols = self
            .cfg
            .symbols
            .iter()
            .map(|mapping| mapping.venue_symbol.clone())
            .collect::<Vec<_>>();

        loop {
            let mut books = symbols
                .iter()
                .map(|symbol| (symbol.clone(), LocalBook::new()))
                .collect::<HashMap<_, _>>();

            match self
                .connect_and_run(&ws_url, &rest_base, &symbols, &mut books)
                .await
            {
                Ok(()) => {
                    tracing::warn!(venue = %self.cfg.venue_id, "binance feed stream ended; reconnecting")
                }
                Err(error) => {
                    tracing::warn!(venue = %self.cfg.venue_id, ?error, "binance feed error; reconnecting")
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
        ws_url: &str,
        rest_base: &str,
        symbols: &[String],
        books: &mut HashMap<String, LocalBook>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (mut ws, _response) = connect_async(ws_url).await?;

        let params = symbols
            .iter()
            .map(|symbol| format!("{}@depth@100ms", symbol.to_lowercase()))
            .collect::<Vec<_>>();
        let sub = json!({
            "method": "SUBSCRIBE",
            "params": params,
            "id": 1,
        });
        ws.send(Message::Text(sub.to_string().into())).await?;

        tracing::info!(venue = %self.cfg.venue_id, symbols = symbols.len(), %ws_url, "binance feed connected");

        loop {
            let msg = match tokio::time::timeout(WS_TIMEOUT, ws.next()).await {
                Ok(Some(Ok(msg))) => msg,
                Ok(Some(Err(error))) => return Err(Box::new(error)),
                Ok(None) => return Ok(()),
                Err(_) => {
                    tracing::warn!(venue = %self.cfg.venue_id, "binance websocket receive timeout; reconnecting");
                    return Ok(());
                }
            };

            match msg {
                Message::Text(text) => self.handle_text(&text, rest_base, books).await,
                Message::Ping(payload) => {
                    if let Err(error) = ws.send(Message::Pong(payload)).await {
                        return Err(Box::new(error));
                    }
                }
                Message::Close(_) => return Ok(()),
                _ => {}
            }
        }
    }

    async fn handle_text(
        &self,
        text: &str,
        rest_base: &str,
        books: &mut HashMap<String, LocalBook>,
    ) {
        let json: serde_json::Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(_) => return,
        };
        let data = match json.get("data") {
            Some(value) => value.clone(),
            None => return,
        };

        let update: DepthUpdate = match serde_json::from_value(data) {
            Ok(update) => update,
            Err(error) => {
                tracing::warn!(venue = %self.cfg.venue_id, ?error, "failed to parse binance depth update; skipping");
                return;
            }
        };

        let Some(book) = books.get_mut(&update.s) else {
            tracing::warn!(venue = %self.cfg.venue_id, symbol = %update.s, "received update for unconfigured symbol");
            return;
        };

        if book.last_update_id == 0 {
            match self.fetch_snapshot(rest_base, &update.s).await {
                Ok(snapshot) => {
                    book.last_update_id = snapshot.last_update_id;
                    book.bids = price_levels_from_pairs(snapshot.bids);
                    book.asks = price_levels_from_pairs(snapshot.asks);
                    book.just_rebuilt = true;
                }
                Err(error) => {
                    tracing::warn!(venue = %self.cfg.venue_id, symbol = %update.s, ?error, "failed to fetch binance snapshot");
                    return;
                }
            }
        }

        if update.u < book.last_update_id {
            return;
        }

        if book.just_rebuilt {
            let ok = update.U <= book.last_update_id.saturating_add(1)
                && update.u >= book.last_update_id.saturating_add(1);
            if !ok {
                book.reset();
                return;
            }
            book.just_rebuilt = false;
        } else if update.pu != book.last_update_id {
            book.reset();
            return;
        }

        apply_side_update(&mut book.bids, update.b, true);
        apply_side_update(&mut book.asks, update.a, false);
        book.last_update_id = update.u;

        let depth = usize::from(self.cfg.book_depth);
        book.bids
            .sort_by(|left, right| right.price.cmp(&left.price));
        book.asks
            .sort_by(|left, right| left.price.cmp(&right.price));
        book.bids.truncate(depth);
        book.asks.truncate(depth);

        self.service
            .ingest_order_book(
                &self.cfg.venue_id,
                &update.s,
                book.bids.clone(),
                book.asks.clone(),
            )
            .await;
    }

    async fn fetch_snapshot(
        &self,
        rest_base: &str,
        symbol: &str,
    ) -> Result<DepthSnapshot, reqwest::Error> {
        let limit = binance_depth_limit(self.cfg.book_depth);
        let limit_str = limit.to_string();
        self.client
            .get(format!("{rest_base}/fapi/v1/depth"))
            .query(&[("symbol", symbol), ("limit", limit_str.as_str())])
            .send()
            .await?
            .json::<DepthSnapshot>()
            .await
    }
}

async fn run_volume_poll(
    service: PricingService,
    client: Client,
    venue_id: String,
    rest_base: String,
    symbols: Vec<String>,
) {
    let mut interval = tokio::time::interval(VOLUME_POLL_INTERVAL);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        match fetch_tickers_24hr(&client, &rest_base, &symbols).await {
            Ok(tickers) => {
                for ticker in tickers {
                    service
                        .ingest_volume_24h(&venue_id, &ticker.symbol, ticker.volume)
                        .await;
                }
            }
            Err(error) => {
                tracing::warn!(venue = %venue_id, ?error, "failed to fetch binance 24h ticker")
            }
        }
    }
}

async fn fetch_tickers_24hr(
    client: &Client,
    rest_base: &str,
    symbols: &[String],
) -> Result<Vec<Ticker24hr>, reqwest::Error> {
    let mut results = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        let ticker = client
            .get(format!("{rest_base}/fapi/v1/ticker/24hr"))
            .query(&[("symbol", symbol.as_str())])
            .send()
            .await?
            .json::<Ticker24hr>()
            .await?;
        results.push(ticker);
    }
    Ok(results)
}

fn apply_side_update(side: &mut Vec<PriceLevel>, updates: Vec<(Decimal, Decimal)>, is_bids: bool) {
    for (price, quantity) in updates {
        let pos = side.partition_point(|existing| {
            if is_bids {
                existing.price > price
            } else {
                existing.price < price
            }
        });
        let exact_match = side
            .get(pos)
            .is_some_and(|existing| existing.price == price);
        if quantity == Decimal::ZERO {
            if exact_match {
                side.remove(pos);
            }
        } else if exact_match {
            side[pos].quantity = quantity;
        } else {
            side.insert(pos, PriceLevel { price, quantity });
        }
    }
}

fn price_levels_from_pairs(pairs: Vec<(Decimal, Decimal)>) -> Vec<PriceLevel> {
    pairs
        .into_iter()
        .map(|(price, quantity)| PriceLevel { price, quantity })
        .collect()
}

fn ws_url_from_cfg(cfg: &VenueFeedConfig) -> String {
    cfg.feed_types
        .iter()
        .find_map(|feed_type| match feed_type {
            FeedType::WebSocket { url } => Some(url.clone()),
            FeedType::Rest { .. } => None,
        })
        .unwrap_or_else(|| MAINNET_WS_URL.to_string())
}

fn rest_base_url_from_cfg(cfg: &VenueFeedConfig) -> String {
    cfg.feed_types
        .iter()
        .find_map(|feed_type| match feed_type {
            FeedType::Rest { url } => Some(url.clone()),
            FeedType::WebSocket { .. } => None,
        })
        .unwrap_or_else(|| MAINNET_REST_BASE.to_string())
}

fn binance_depth_limit(book_depth: u8) -> u16 {
    const VALID: [u16; 7] = [5, 10, 20, 50, 100, 500, 1000];
    let depth = u16::from(book_depth);
    VALID
        .iter()
        .find(|&&value| value >= depth)
        .copied()
        .unwrap_or(1000)
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
