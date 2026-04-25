use eyre::{Result, bail};
use reqwest;
use std::time::Duration;
use tars::{
    api::primitives::{Response, Status},
    orderbook::primitives::MatchedOrderVerbose,
};

const REQUEST_TIMEOUT: u64 = 60;

#[derive(Debug, thiserror::Error)]
pub enum PendingOrdersError {
    #[error("Failed to get pending orders: {0}")]
    PendingOrdersError(String),
}

#[derive(Debug, Clone)]
pub struct PendingOrdersProvider {
    client: reqwest::Client,
    url: reqwest::Url,
}

impl PendingOrdersProvider {
    pub fn new(url: reqwest::Url) -> Self {
        let mut normalized_url = url;
        if !normalized_url.path().ends_with('/') {
            normalized_url.set_path(&format!("{}/", normalized_url.path()));
        }

        Self {
            client: reqwest::Client::new(),
            url: normalized_url,
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
            .await?;

        if !response.status().is_success() {
            let error_body = response.text().await?;
            bail!(PendingOrdersError::PendingOrdersError(error_body));
        }

        let orders: Response<Vec<MatchedOrderVerbose>> = response.json().await?;

        match orders.status {
            Status::Ok => Ok(orders.result.unwrap_or_default()),
            _ => bail!(PendingOrdersError::PendingOrdersError(
                orders.error.unwrap_or_else(|| "unknown error".to_string())
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
            .and(path_regex(r"/bitcoin$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&mock_server)
            .await;

        mock_server.uri()
    }

    #[tokio::test]
    async fn test_get_pending_orders() {
        let uri = setup_server().await;
        let provider = PendingOrdersProvider::new(reqwest::Url::parse(&uri).unwrap());
        let orders = provider
            .get_pending_orders(
                "bitcoin",
                "ecf8fc65ef8dc4f4ec0f2fc3a9191400c75af2e4d5fdd2c4f196822945812c6d",
            )
            .await
            .unwrap();
        assert_eq!(orders.len(), 1);
        let order = orders.first().unwrap();
        assert_eq!(order.create_order.create_id, "1");
    }

    #[tokio::test]
    async fn preserves_path_prefix_when_base_url_has_no_trailing_slash() {
        let mock_server = MockServer::start().await;

        let order = orderbook::test_utils::default_matched_order();
        let response = Response {
            status: Status::Ok,
            result: Some(vec![order]),
            error: None,
            status_code: StatusCode::OK,
        };

        Mock::given(method("GET"))
            .and(path_regex(r"/api/bitcoin$"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&response))
            .mount(&mock_server)
            .await;

        let provider =
            PendingOrdersProvider::new(reqwest::Url::parse(&format!("{}/api", mock_server.uri())).unwrap());
        let orders = provider
            .get_pending_orders(
                "bitcoin",
                "ecf8fc65ef8dc4f4ec0f2fc3a9191400c75af2e4d5fdd2c4f196822945812c6d",
            )
            .await
            .unwrap();

        assert_eq!(orders.len(), 1);
    }
}
