use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::Serialize;
use thiserror::Error;

/// Application-wide error type exposed through the HTTP API.
#[derive(Debug, Error)]
pub enum AppError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("upstream error: {0}")]
    Upstream(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// Builds a 400-style error for invalid client input.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest(message.into())
    }

    /// Builds a 404-style error for missing resources.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound(message.into())
    }

    /// Builds a 409-style error for uniqueness or state conflicts.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict(message.into())
    }

    /// Builds a 500-style error for unexpected local failures.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal(message.into())
    }
}

/// Standard JSON error payload returned by the HTTP server.
#[derive(Debug, Serialize)]
struct ErrorBody {
    ok: bool,
    error: String,
}

impl IntoResponse for AppError {
    /// Maps internal error categories to stable HTTP responses.
    fn into_response(self) -> axum::response::Response {
        let status = match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::Upstream(_) => StatusCode::BAD_GATEWAY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };

        let body = ErrorBody {
            ok: false,
            error: self.to_string(),
        };

        (status, Json(body)).into_response()
    }
}

impl From<eyre::Report> for AppError {
    /// Treats generic reports as internal errors at the API boundary.
    fn from(value: eyre::Report) -> Self {
        Self::Internal(value.to_string())
    }
}

impl From<tars::orderbook::errors::OrderbookError> for AppError {
    /// Preserves the important orderbook conflict case and collapses the rest.
    fn from(value: tars::orderbook::errors::OrderbookError) -> Self {
        match value {
            tars::orderbook::errors::OrderbookError::OrderAlreadyExists(message) => {
                Self::Conflict(message)
            }
            other => Self::Internal(other.to_string()),
        }
    }
}
