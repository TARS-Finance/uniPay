use eyre::{Result, bail};
use orderbook::primitives::MatchedOrderVerbose;
use reqwest::Url;
use serde::Deserialize;
use std::time::Duration;

const REQUEST_TIMEOUT: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum PendingOrdersError {
    #[error("Failed to get pending orders: {0}")]
    Request(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
enum Status {
    Ok,
    Error,
}

#[derive(Deserialize)]
struct SolverOrdersResponse<T> {
    status: Status,
    result: Option<T>,
    error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingOrdersProvider {
    client: reqwest::Client,
    url: Url,
}

impl PendingOrdersProvider {
    pub fn new(url: Url) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
        }
    }

    pub async fn get_pending_orders(
        &self,
        chain: &str,
        solver_id: &str,
    ) -> Result<Vec<MatchedOrderVerbose>> {
        let url = self.url.join(chain)?;
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT))
            .query(&[("solver", solver_id)])
            .send()
            .await
            .map_err(|e| PendingOrdersError::Request(e.to_string()))?;

        if !response.status().is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!(PendingOrdersError::Request(body));
        }

        let resp: SolverOrdersResponse<Vec<MatchedOrderVerbose>> = response
            .json()
            .await
            .map_err(|e| PendingOrdersError::Request(e.to_string()))?;

        match resp.status {
            Status::Ok => Ok(resp.result.unwrap_or_default()),
            Status::Error => bail!(PendingOrdersError::Request(
                resp.error.unwrap_or_else(|| "unknown error".into())
            )),
        }
    }
}
