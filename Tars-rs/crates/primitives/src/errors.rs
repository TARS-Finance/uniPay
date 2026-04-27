use api::primitives::{Response, Status};
use reqwest::StatusCode;
use axum::{response::IntoResponse};

// Errors that can occur when interacting with Relay's
#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    /// Error response returned by the API
    #[error("{0}")]
    Api(String, StatusCode),
    /// Request failed
    #[error("{0}")]
    Request(String, StatusCode),
    /// Failed to parse URL or JSON
    #[error("{0}")]
    ParseError(String),
}

impl From<reqwest::Error> for RelayError {
    fn from(err: reqwest::Error) -> Self {
        RelayError::Request(err.to_string(), StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl From<serde_json::Error> for RelayError {
    fn from(err: serde_json::Error) -> Self {
        RelayError::ParseError(err.to_string())
    }
}

impl From<url::ParseError> for RelayError {
    fn from(err: url::ParseError) -> Self {
        RelayError::ParseError(err.to_string())
    }
}

impl IntoResponse for RelayError {
    fn into_response(self) -> axum::response::Response {
        match self {
            RelayError::Api(msg, status) |
            RelayError::Request(msg, status) => {
                (status, msg).into_response()
            }
            RelayError::ParseError(msg) => {
                (StatusCode::INTERNAL_SERVER_ERROR, msg).into_response()
            }
        }
    }
}

pub async fn handle_api_response<T>(response: reqwest::Response) -> Result<T, RelayError>
where
    T: serde::de::DeserializeOwned,
{
    let api_response: Response<T> = response.json().await?;

    match api_response.status {
        Status::Ok => api_response
            .result
            .ok_or_else(|| RelayError::ParseError("Empty result".to_string())),
        Status::Error => {
            let message = api_response
                .error
                .unwrap_or_else(|| "Unknown error".to_string());
            Err(RelayError::Api(message, api_response.status_code))
        }
    }
}