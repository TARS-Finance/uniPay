use crate::primitives::Response;
use reqwest::StatusCode;
use tracing::{error, warn};

/// The error message to include in the response
const INTERNAL_ERROR: &str = "Internal Error";

/// Creates an HTTP 500 Internal Server Error response with a custom error message
///
/// # Arguments
/// * `message` - The error message to include in the error log
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn internal_error(message: &str) -> Response<()> {
    error!(
        status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        "{:#?}", message
    );
    Response::error(INTERNAL_ERROR, StatusCode::INTERNAL_SERVER_ERROR)
}

/// Creates an HTTP 400 Bad Request response with a custom error message
///
/// # Arguments
/// * `message` - The error message to include in the response
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn bad_request(message: &str) -> Response<()> {
    warn!(status = StatusCode::BAD_REQUEST.as_u16(), "{:#?}", message);
    Response::error(message, StatusCode::BAD_REQUEST)
}

/// Creates an HTTP 404 Not Found response with a custom error message
///
/// # Arguments
/// * `message` - The error message to include in the response
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn not_found(message: &str) -> Response<()> {
    error!(status = StatusCode::NOT_FOUND.as_u16(), "{:#?}", message);
    Response::error(message, StatusCode::NOT_FOUND)
}

/// Creates an HTTP 401 Unauthorized response with a custom error message
///
/// # Arguments
/// * `message` - The error message to include in the response
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn unauthorized(message: &str) -> Response<()> {
    warn!(status = StatusCode::UNAUTHORIZED.as_u16(), "{:#?}", message);
    Response::error(message, StatusCode::UNAUTHORIZED)
}

/// Creates an HTTP 400 Bad Request response with a custom error message and a report
///
/// # Arguments
/// * `message` - The error message to include in the response
/// * `report` - The report to include in the response
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn bad_request_with_report(message: &str, report: &eyre::Report) -> Response<()> {
    warn!(
        status = StatusCode::BAD_REQUEST.as_u16(),
        "{:#?} {:#?}", message, report
    );
    Response::error(message, StatusCode::BAD_REQUEST)
}

/// Creates an HTTP 404 Not Found response with a custom error message and a report
///
/// # Arguments
/// * `message` - The error message to include in the response
/// * `error` - The error to include in the response
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn not_found_with_error(message: &str, error: &eyre::Report) -> Response<()> {
    warn!(
        status = StatusCode::NOT_FOUND.as_u16(),
        "{:#?} {:#?}", message, error
    );
    Response::error(message, StatusCode::NOT_FOUND)
}

/// Creates an HTTP 500 Internal Server Error response with a custom error message and a report
///
/// # Arguments
/// * `message` - The error message to include in the error log
/// * `error` - The error to include in the response
///
/// # Returns
/// * `Response<()>` - A response containing the error response
pub fn internal_error_with_error(message: &str, error: &eyre::Report) -> Response<()> {
    error!(
        status = StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
        "{:#?} {:#?}", message, error
    );
    Response::error(INTERNAL_ERROR, StatusCode::INTERNAL_SERVER_ERROR)
}
