use crate::pricing::service::PricingService;
use reqwest::{Client, Url};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

const CMC_API_KEY_HEADER: &str = "X-CMC_PRO_API_KEY";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Background CoinMarketCap fetcher that feeds canonical prices into the market-data service.
pub struct CmcPriceFetcher {
    client: Client,
    url: Url,
    api_keys: Vec<String>,
    fiat_ids_map: HashMap<String, String>,
    service: PricingService,
    interval: Duration,
}

impl CmcPriceFetcher {
    pub fn new(
        api_url: String,
        api_keys: Vec<String>,
        service: PricingService,
        fiat_ids_map: HashMap<String, String>,
        base_cooldown_secs: u64,
    ) -> eyre::Result<Self> {
        Ok(Self {
            client: Client::builder().timeout(REQUEST_TIMEOUT).build()?,
            url: Url::parse(&api_url)?,
            api_keys,
            fiat_ids_map,
            service,
            interval: Duration::from_secs(base_cooldown_secs.max(1)),
        })
    }

    pub fn start(self) {
        tokio::spawn(async move {
            self.run().await;
        });
    }

    async fn run(self) {
        let mut key_index = 0usize;
        loop {
            if let Err(error) = self.fetch_and_update(key_index).await {
                tracing::warn!(?error, "cmc price fetch failed");
            }
            key_index = key_index.wrapping_add(1);
            tokio::time::sleep(self.interval).await;
        }
    }

    async fn fetch_and_update(&self, key_index: usize) -> eyre::Result<()> {
        if self.fiat_ids_map.is_empty() {
            return Ok(());
        }

        let mut ids = self.fiat_ids_map.values().cloned().collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();

        let mut request = self
            .client
            .get(self.url.clone())
            .query(&[("id", ids.join(",")), ("convert", "USD".to_string())]);
        if let Some(api_key) = select_api_key(&self.api_keys, key_index) {
            request = request.header(CMC_API_KEY_HEADER, api_key);
        }

        let response = request.send().await?.error_for_status()?;
        let payload: CmcResponse = response.json().await?;

        for (canonical, provider_id) in &self.fiat_ids_map {
            let Some(entry) = payload.data.get(provider_id) else {
                continue;
            };
            let Some(price_f64) = entry.first_price_usd() else {
                continue;
            };
            let Some(price) = Decimal::from_f64(price_f64) else {
                continue;
            };
            self.service
                .ingest_aggregator_price("cmc", canonical, price)
                .await;
        }

        Ok(())
    }
}

fn select_api_key(api_keys: &[String], index: usize) -> Option<&str> {
    if api_keys.is_empty() {
        return None;
    }
    let key = api_keys[index % api_keys.len()].trim();
    if key.is_empty() { None } else { Some(key) }
}

#[derive(Debug, Deserialize)]
struct CmcResponse {
    data: HashMap<String, CmcDataEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CmcDataEntry {
    Single(CmcAsset),
    Multiple(Vec<CmcAsset>),
}

impl CmcDataEntry {
    fn first_price_usd(&self) -> Option<f64> {
        match self {
            Self::Single(asset) => asset.price_usd(),
            Self::Multiple(assets) => assets.first().and_then(CmcAsset::price_usd),
        }
    }
}

#[derive(Debug, Deserialize)]
struct CmcAsset {
    quote: HashMap<String, CmcQuote>,
}

impl CmcAsset {
    fn price_usd(&self) -> Option<f64> {
        self.quote.get("USD").map(|quote| quote.price)
    }
}

#[derive(Debug, Deserialize)]
struct CmcQuote {
    price: f64,
}
