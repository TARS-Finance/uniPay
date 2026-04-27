use std::time::Duration;
use eyre::Result;
use primitives::{handle_api_response, HTLCActionRequest, RelayError};
use reqwest::Client;
use url::Url;

const REQUEST_TIMEOUT: u64 = 10;

#[derive(Debug, Clone)]
pub struct BitcoinRelay {
    pub url: Url,
    pub client: Client,
}

impl BitcoinRelay {
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

    pub async fn execute_action(
        &self,
        order_id: &str,
        action_request: HTLCActionRequest
    ) -> Result<Option<String>, RelayError> {
        let url = self.url.join(format!("v2/orders/{order_id}").as_str())?;

        let body = serde_json::to_value(&action_request.action)?;

        let request = self
            .client
            .patch(url)
            .headers(action_request.headers.into())
            .query(&[("action", action_request.action.to_string().as_str())])
            .json(&body);

        let response = request.send().await?;
        let result = handle_api_response::<String>(response).await?;
        Ok(Some(result))
    }
}
