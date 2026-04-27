use std::time::Duration;
use eyre::Result;
use orderbook::primitives::SwapChain;
use primitives::{handle_api_response, HTLCActionRequest, RelayError};
use reqwest::Client;
use serde_json::json;
use url::Url;

const REQUEST_TIMEOUT: u64 = 10;

#[derive(Debug, Clone)]
pub struct EVMRelay {
    pub url: Url,
    pub client: Client,
}

impl EVMRelay {
    pub fn new(url: &str) -> Result<Self> {
        let url = Url::parse(url)?;
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("Content-Type", "application/json".parse()?);

        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT))
            .default_headers(headers)
            .build()?;
        Ok(Self { url, client })
    }

    pub async fn initiate_with_signature(
        &self,
        order_id: &str,
        signature: &str,
        perform_on: SwapChain,
    ) -> Result<String, RelayError> {
        let request = json!({
            "order_id": order_id,
            "signature": signature,
            "perform_on": perform_on,
        });

        let url = self.url.join("initiate")?;
        let response = self.client.post(url).json(&request).send().await?;

        handle_api_response::<String>(response).await
    }

    pub async fn redeem(
        &self,
        order_id: &str,
        secret: &str,
        perform_on: SwapChain,
    ) -> Result<String, RelayError> {
        let request = json!({
            "order_id": order_id,
            "secret": secret,
            "perform_on": perform_on,
        });

        let url = self.url.join("redeem")?;
        let response = self.client.post(url).json(&request).send().await?;

        handle_api_response::<String>(response).await
    }

    pub async fn execute_action(
        &self,
        order_id: &str,
        action_request: HTLCActionRequest,
    ) -> Result<Option<String>, RelayError> {
        let url = self.url.join(format!("v2/orders/{order_id}").as_str())?;

        let body = serde_json::to_value(&action_request.action)?;

        let response = self
            .client
            .patch(url)
            .query(&[("action", action_request.action.to_string().as_str())])
            .headers(action_request.headers)
            .json(&body)
            .send()
            .await?;

        let result = handle_api_response::<String>(response).await?;
        Ok(Some(result))
    }
}
