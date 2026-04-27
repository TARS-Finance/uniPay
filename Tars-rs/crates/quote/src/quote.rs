use crate::{
    primitives::{FiatResult, QuoteResult},
    AssetResponseItem, Strategy,
};
use api::primitives::{Response, ResponseLegacy, Status};
use bigdecimal::BigDecimal;
use eyre::Result;
use moka::future::Cache;
use orderbook::primitives::{CreatableAdditionalData, MatchedOrderVerbose, Order};
use reqwest::{Client, Url};
use std::{collections::HashMap, time::Duration};

const CACHE_KEY: &str = "strategies_cache";
const CACHE_TTL: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT_MS: u64 = 10000;

#[derive(Debug, thiserror::Error)]
pub enum QuoteError {
    #[error("API error: {0}")]
    Api(String),
    #[error("Request failed: {0}")]
    Request(String),
    #[error("Failed to parse: {0}")]
    ParseError(String),
}

impl From<reqwest::Error> for QuoteError {
    fn from(err: reqwest::Error) -> Self {
        QuoteError::Request(format!("Network error: {}", err))
    }
}

impl From<serde_json::Error> for QuoteError {
    fn from(err: serde_json::Error) -> Self {
        QuoteError::ParseError(format!("JSON parse error: {}", err))
    }
}

impl From<url::ParseError> for QuoteError {
    fn from(err: url::ParseError) -> Self {
        QuoteError::ParseError(format!("URL parse error: {}", err))
    }
}

#[derive(Debug)]
pub struct QuoteProvider {
    url: Url,
    client: Client,
    strategies_cache: Cache<&'static str, HashMap<String, Strategy>>,
    prices_cache: Cache<String, BigDecimal>,
}

impl QuoteProvider {
    pub fn new(base_url: &str) -> Self {
        let url = Url::parse(base_url).expect("Failed to parse Quote Provider URL");

        let client = Client::builder()
            .timeout(Duration::from_millis(REQUEST_TIMEOUT_MS))
            .build()
            .expect("Failed to create Quote Provider client");

        Self {
            url,
            client,
            strategies_cache: Cache::builder().time_to_live(CACHE_TTL).build(),
            prices_cache: Cache::builder().time_to_live(CACHE_TTL).build(),
        }
    }

    pub async fn get_quote(
        &self,
        order_pair: &str,
        amount: &BigDecimal,
        is_exact_out: bool,
    ) -> Result<QuoteResult, QuoteError> {
        let response = self
            .client
            .get(self.url.clone())
            .query(&[
                ("order_pair", order_pair.to_string()),
                ("amount", amount.to_string()),
                ("exact_out", is_exact_out.to_string()),
            ])
            .send()
            .await?;

        handle_response(response).await
    }

    pub async fn get_fiat_prices(&self, order_pair: &str) -> Result<FiatResult, QuoteError> {
        let url = self.url.join("fiat")?;
        let response = self
            .client
            .get(url)
            .query(&[("order_pair", order_pair.to_string())])
            .send()
            .await?;

        handle_response(response).await
    }

    pub async fn get_strategies(&self) -> Result<HashMap<String, Strategy>, QuoteError> {
        if let Some(strategies) = self.strategies_cache.get(CACHE_KEY).await {
            return Ok(strategies);
        }

        let url = self.url.join("strategies")?;
        let response = self.client.get(url).send().await?;

        let strategies: HashMap<String, Strategy> = handle_response(response).await?;
        self.strategies_cache
            .insert(CACHE_KEY, strategies.clone())
            .await;

        Ok(strategies)
    }

    pub async fn get_match_order(
        &self,
        order: &Order<CreatableAdditionalData>,
    ) -> Result<MatchedOrderVerbose, QuoteError> {
        let mut url = self.url.join("attested")?;
        url.query_pairs_mut().append_pair("match_order", "true");

        let response = self.client.post(url).json(order).send().await?;
        handle_response(response).await
    }

    pub async fn get_asset_price(&self, asset_id: &str) -> Result<BigDecimal, QuoteError> {
        if let Some(price) = self.prices_cache.get(asset_id).await {
            return Ok(price);
        }

        let url = self.url.join("v2/assets")?;
        let response = self.client.get(url).send().await?;
        let status = response.status().as_u16();
        let text = response.text().await?;

        let assets: Response<Vec<AssetResponseItem>> =
            serde_json::from_str(&text).map_err(|_| {
                QuoteError::ParseError(format!("JSON parse failed (status: {}): {}", status, text))
            })?;

        let assets = assets
            .result
            .ok_or_else(|| QuoteError::ParseError("empty response".to_string()))?;

        for asset in assets {
            if let Some(price) = asset.price {
                self.prices_cache.insert(asset.id, price).await;
            }
        }

        self.prices_cache
            .get(asset_id)
            .await
            .ok_or_else(|| QuoteError::ParseError("asset not found".to_string()))
    }
}

async fn handle_response<T>(response: reqwest::Response) -> Result<T, QuoteError>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let status = response.status().as_u16();
    let text = response.text().await?;

    let resp: ResponseLegacy<T> = serde_json::from_str(&text).map_err(|_| {
        QuoteError::ParseError(format!("JSON parse failed (status: {}): {}", status, text))
    })?;

    match resp.status {
        Status::Ok => resp
            .result
            .ok_or_else(|| QuoteError::ParseError("empty response".to_string())),
        Status::Error => {
            let (code, message) = resp
                .error
                .map(|e| (e.code, e.message))
                .unwrap_or((0, "Unknown error".to_string()));
            match code {
                500 => Err(QuoteError::Request(format!(
                    "Internal server error: {}",
                    message
                ))),
                _ => Err(QuoteError::Api(message)),
            }
        }
    }
}
