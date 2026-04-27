use crate::primitives::MatchedOrderVerbose;
use api::primitives::{Response, Status};
use eyre::Result;
use std::time::Duration;

const REQUEST_TIMEOUT: u64 = 60;

/// Provides access to pending orders from a remote API.
#[derive(Debug, Clone)]
pub struct PendingOrdersProvider {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl PendingOrdersProvider {
    /// Creates a new `PendingOrdersProvider`.
    ///
    /// # Parameters
    /// - `url`: The URL endpoint to fetch pending orders from (without trailing slash).
    /// - `timeout`: Optional timeout in seconds for HTTP requests.
    ///
    /// # Returns
    /// A new instance of `PendingOrdersProvider`.
    pub fn new(url: &str, timeout: Option<u64>) -> Self {
        let url = reqwest::Url::parse(url).expect("Invalid pending_orders_url");
        let timeout = Duration::from_secs(timeout.unwrap_or(REQUEST_TIMEOUT));
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .expect("Failed to build reqwest client");

        Self { client, url }
    }

    /// Fetches pending orders from the remote API.
    ///
    /// # Parameters
    /// - `chain`: An optional `String` specifying the chain to fetch pending orders for.
    ///   If `None`, pending orders from all chains are fetched.
    ///
    /// # Returns
    /// A `Result` containing a vector of `MatchedOrderVerbose` on success, or an error on failure.
    pub async fn get_pending_orders(
        &self,
        chain: Option<String>,
    ) -> Result<Vec<MatchedOrderVerbose>> {
        let url = match chain {
            Some(chain) => self.url.join(&format!("/{}", chain))?,
            None => self.url.clone(),
        };

        let response = self.client.get(url.clone()).send().await?;

        if !response.status().is_success() {
            let error = response.text().await?;
            return Err(eyre::eyre!("Failed to fetch pending orders: {}", error));
        }

        let resp: Response<Vec<MatchedOrderVerbose>> = response.json().await?;
        if resp.status != Status::Ok {
            if let Some(err) = resp.error {
                return Err(eyre::eyre!(err));
            } else {
                return Err(eyre::eyre!("Internal error"));
            }
        }
        match resp.result {
            Some(orders) => Ok(orders),
            None => {
                if let Some(err) = resp.error {
                    Err(eyre::eyre!(err))
                } else {
                    Err(eyre::eyre!("Internal error"))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use reqwest::StatusCode;
    use wiremock::{
        matchers::{method, path_regex},
        Mock, MockServer, ResponseTemplate,
    };

    use crate::test_utils;

    use super::*;

    async fn setup_server() -> String {
        let mock_server = MockServer::start().await;

        let order = test_utils::default_matched_order();
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
        let provider = PendingOrdersProvider::new(&uri, None);
        let orders = provider
            .get_pending_orders(Some("ethereum".to_string()))
            .await
            .unwrap();
        assert_eq!(orders.len(), 1);
        let order = orders.first().unwrap();
        assert_eq!(order.create_order.create_id, "1");
    }
}
