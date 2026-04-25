use eyre::{Result, bail};
use reqwest;
use std::time::Duration;
use tars::{
    api::primitives::{Response, Status},
    orderbook::primitives::MatchedOrderVerbose,
};

// Default request timeout in seconds
const REQUEST_TIMEOUT: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum PendingOrdersError {
    #[error("Failed to get pending orders: {0}")]
    PendingOrdersError(String),
}

/// PendingOrdersProvider is a abstract interface for fetching pending orders from the solver orders API
#[derive(Debug, Clone)]
pub struct PendingOrdersProvider {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl PendingOrdersProvider {
    /// Create a new PendingOrdersProvider
    ///
    /// # Arguments
    /// * `url` - The URL of the pending orders provider
    ///
    pub fn new(url: reqwest::Url) -> Self {
        Self {
            client: reqwest::Client::new(),
            url,
        }
    }

    /// Get pending orders from the provider for a specific chain
    ///
    /// # Arguments
    /// * `chain` - The chain to get pending orders for
    ///
    /// # Returns
    /// * `Result<Vec<MatchedOrderVerbose>>` - The pending orders
    ///
    pub async fn get_pending_orders(&self, chain: &str) -> Result<Vec<MatchedOrderVerbose>> {
        let url = self.url.join(chain)?;
        let response = self
            .client
            .get(url)
            .timeout(Duration::from_secs(REQUEST_TIMEOUT))
            .send()
            .await?;

        if !response.status().is_success() {
            let error_body = response.text().await?;
            bail!(PendingOrdersError::PendingOrdersError(error_body));
        }

        let orders: Response<Vec<MatchedOrderVerbose>> = response.json().await?;

        match orders.status {
            Status::Ok => Ok(orders.result.unwrap()),
            _ => bail!(PendingOrdersError::PendingOrdersError(
                orders.error.unwrap()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use tars::orderbook;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path_regex},
    };

    use super::*;

    async fn setup_server() -> String {
        let mock_server = MockServer::start().await;

        let order = orderbook::test_utils::default_matched_order();
        let orders = vec![order];

        let response = Response {
            status: Status::Ok,
            result: Some(orders),
            error: None,
            status_code: StatusCode::OK,
        };

        Mock::given(method("GET"))
            .and(path_regex(r"/ethereum$")) // Match any URL ending with /ethereum
            .respond_with(ResponseTemplate::new(200).set_body_json(&response)) // Pass the orders array directly
            .mount(&mock_server)
            .await;

        mock_server.uri()
    }

    #[tokio::test]
    async fn test_get_pending_orders() {
        let uri = setup_server().await;
        let provider = PendingOrdersProvider::new(reqwest::Url::parse(&uri).unwrap());
        let orders = provider
            .get_pending_orders("ethereum")
            .await
            .unwrap();
        assert_eq!(orders.len(), 1);
        let order = orders.first().unwrap();
        assert_eq!(order.create_order.create_id, "1");
    }
}
