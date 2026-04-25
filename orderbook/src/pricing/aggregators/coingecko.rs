use crate::pricing::service::PricingService;
use reqwest::{Client, Url};
use rust_decimal::Decimal;
use rust_decimal::prelude::FromPrimitive;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

const COINGECKO_API_KEY_HEADER: &str = "x-cg-demo-api-key";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// Background CoinGecko fetcher that feeds canonical prices into the market-data service.
pub struct CoingeckoPriceFetcher {
    client: Client,
    url: Url,
    api_keys: Vec<String>,
    fiat_ids_map: HashMap<String, String>,
    service: PricingService,
    interval: Duration,
}

impl CoingeckoPriceFetcher {
    pub fn new(
        api_url: String,
        api_keys: Vec<String>,
        service: PricingService,
        fiat_ids_map: HashMap<String, String>,
        base_cooldown_secs: u64,
    ) -> eyre::Result<Self> {
        let url = Url::parse(&api_url)?;
        let client = Client::builder().timeout(REQUEST_TIMEOUT).build()?;
        Ok(Self {
            client,
            url,
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
                tracing::warn!(?error, "coingecko price fetch failed");
            }
            key_index = key_index.wrapping_add(1);
            tokio::time::sleep(self.interval).await;
        }
    }

    async fn fetch_and_update(&self, key_index: usize) -> eyre::Result<()> {
        if self.fiat_ids_map.is_empty() {
            return Ok(());
        }

        let mut ids = self
            .fiat_ids_map
            .values()
            .map(|value| value.to_ascii_lowercase())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();

        let mut request = self.client.get(self.url.clone()).query(&[
            ("ids".to_string(), ids.join(",")),
            ("vs_currencies".to_string(), "usd".to_string()),
        ]);
        if let Some(api_key) = select_api_key(&self.api_keys, key_index) {
            request = request.header(COINGECKO_API_KEY_HEADER, api_key);
        }

        let response = request
            .header("User-Agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?;
        let payload: HashMap<String, CoingeckoQuote> = response.json().await?;

        for (canonical, provider_id) in &self.fiat_ids_map {
            let Some(quote) = payload.get(&provider_id.to_ascii_lowercase()) else {
                continue;
            };
            let Some(price) = Decimal::from_f64(quote.usd) else {
                continue;
            };
            self.service
                .ingest_aggregator_price("coingecko", canonical, price)
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
struct CoingeckoQuote {
    usd: f64,
}
