use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Consistent success envelope for all JSON endpoints.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub ok: bool,
    pub data: T,
}

/// Wraps successful handler output in the standard response envelope.
pub fn success<T: Serialize>(data: T) -> Json<ApiResponse<T>> {
    Json(ApiResponse { ok: true, data })
}

/// Quote-compatible response envelope used by the `/fiat` compatibility endpoint.
#[derive(Debug, Serialize)]
pub struct LegacyApiResponse<T> {
    pub status: LegacyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<LegacyApiError>,
}

/// Status values used by the legacy quote-compatible envelope.
#[derive(Debug, Serialize)]
pub enum LegacyStatus {
    Ok,
    Error,
}

/// Error payload shape used by the legacy quote-compatible envelope.
#[derive(Debug, Serialize)]
pub struct LegacyApiError {
    pub code: u16,
    pub message: String,
}

/// Wraps successful handler output in the quote-compatible envelope.
pub fn legacy_success<T: Serialize>(data: T) -> Response {
    Json(LegacyApiResponse {
        status: LegacyStatus::Ok,
        result: Some(data),
        error: None,
    })
    .into_response()
}

/// Wraps failed handler output in the quote-compatible envelope.
pub fn legacy_error(status: StatusCode, message: impl Into<String>) -> Response {
    (
        status,
        Json(LegacyApiResponse::<()> {
            status: LegacyStatus::Error,
            result: None,
            error: Some(LegacyApiError {
                code: status.as_u16(),
                message: message.into(),
            }),
        }),
    )
        .into_response()
}
