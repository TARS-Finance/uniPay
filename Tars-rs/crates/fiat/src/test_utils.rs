use api::primitives::Response;
use rand::Rng;
use reqwest::StatusCode;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::method;

use crate::FiatPriceResult;

pub const MOCK_FIAT_SERVER_URL: &str = "http://127.0.0.1:8080";

/// Start a mock server that returns a random value for the fiat API
pub async fn start_mock_fiat_server() -> String {
    // Start a new mock server
    let mock_server = MockServer::start().await;
    
    // Generate a random value for the response
    let value: f64 = rand::thread_rng().gen_range(1.0..10.0);
    let response = Response {
        status : api::primitives::Status::Ok,
        error : None,
        result : Some(FiatPriceResult {
            input_token_price: value,
            output_token_price: value,
        }),
        status_code: StatusCode::OK,
    };
    
    // Set up the mock response
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&mock_server)
        .await;
    
    mock_server.uri()
}