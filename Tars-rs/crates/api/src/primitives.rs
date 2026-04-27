//! Primitive datatypes for REST API responses
//!
//! This module provides standardized response structures for REST APIs.
//! The main components are:
//! - `Status`: An enum representing the status of an API response
//! - `Response<T>`: A generic structure for all API responses with standardized fields
//! - Helper functions for creating common response types

use axum::{http::StatusCode, response::IntoResponse, response::Response as AxumResponse, Json};
use serde::{Deserialize, Serialize};

/// Status of an API response
///
/// Used to indicate whether an API call was successful or encountered an error.
/// This is included as a top-level field in every response.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Status {
    /// Operation completed successfully
    Ok,
    /// Operation encountered an error
    Error,
}

/// Standard API response wrapper
///
/// This structure wraps all API responses to provide a consistent format:
/// - `status`: Indicates if the request was successful or encountered an error
/// - `data`: Contains the actual response data when successful
/// - `error`: Contains error details when the request fails
///
/// # Examples
///
/// ```
/// # use crate::api::primitives::{Response, Status};
/// let success = Response::ok("success data");
/// let error = Response::<()>::error("something went wrong");
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response<T> {
    /// Status of the response (Ok or Error)
    pub status: Status,

    /// The response payload when status is Ok
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,

    /// Error details when status is Error
    /// Only present when an error occurs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    /// The status code of the response
    #[serde(skip)]
    pub status_code: StatusCode,
}

impl<T> Response<T> {
    /// Creates a successful response with the given data
    ///
    /// # Arguments
    ///
    /// * `data` - The data to include in the successful response
    ///
    /// # Returns
    ///
    /// A JSON-wrapped Response with Ok status and the provided data
    pub fn ok(data: T) -> Self {
        Self {
            status: Status::Ok,
            result: Some(data),
            error: None,
            status_code: StatusCode::OK,
        }
    }

    /// Creates a successful response with the given data and status code
    ///
    /// # Arguments
    ///
    /// * `data` - The data to include in the successful response
    /// * `status` - The status code to include in the response
    ///
    /// # Returns
    ///
    /// A JSON-wrapped Response with Ok status and the provided data and status code
    pub fn ok_with_status(data: T, status_code: StatusCode) -> Self {
        Self {
            status: Status::Ok,
            result: Some(data),
            error: None,
            status_code,
        }
    }

    /// Creates an error response with the given error message
    ///
    /// # Arguments
    ///
    /// * `error` - Any type that can be converted to a String
    ///
    /// # Returns
    ///
    /// A JSON-wrapped Response with Error status and the provided error message
    pub fn error<E: ToString>(error: E, status_code: StatusCode) -> Self {
        Self {
            status: Status::Error,
            error: Some(error.to_string()),
            result: None,
            status_code,
        }
    }

    /// Wraps the current response in a JSON wrapper for use with axum
    ///
    /// This is a convenience method when you already have a Response instance
    pub fn into_json(self) -> Json<Self> {
        Json(self)
    }
}

impl<T> IntoResponse for Response<T>
where
    T: serde::Serialize,
{
    fn into_response(self) -> AxumResponse {
        let status_code = self.status_code;
        let mut response = Json(self).into_response();
        *response.status_mut() = status_code;
        response
    }
}

/// Type alias for API result (success or error)
pub type ApiResult<T> = Result<Response<T>, Response<()>>;

#[deprecated(note = "use Response::error() instead")]
/// Creates an error response with the given error message
///
/// This is a shorthand for `Response::<()>::error(error)`
///
/// # Arguments
///
/// * `error` - Any type that can be converted to a String
///
/// # Returns
///
/// A JSON-wrapped Response with Error status and the provided error message
pub fn res_err<E: ToString>(error: E) -> Response<()> {
    Response::<()>::error(error, StatusCode::OK)
}

#[deprecated(note = "use Response::ok() instead")]
/// Creates a successful response with the given data
///
/// This is a shorthand for `Response::ok(data)`
///
/// # Arguments
///
/// * `data` - The data to include in the successful response
///
/// # Returns
///
/// A JSON-wrapped Response with Ok status and the provided data
pub fn res_ok<T>(data: T) -> Response<T> {
    Response::ok(data)
}

/// Error structure for legacy API responses
///
/// # Examples
///
/// ```
/// let error = Error {
///     code: 404,
///     message: "Not Found".to_string(),
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Error {
    /// Error code
    pub code: u32,
    /// Error message
    pub message: String,
}

/// Standard legacy API response wrapper
///
/// This structure wraps all API responses to provide a consistent format:
/// - `status`: Indicates if the request was successful or encountered an error
/// - `result`: Contains the actual response data when successful
/// - `error`: Contains error details when the request fails
///
/// # Examples
///
/// ```
/// # use crate::quote::primitives::{Response, Status};
/// let success = ResponseLegacy::ok("success data");
/// let error = ResponseLegacy::<()>::error("something went wrong");
/// ```
///
///
/// NOTE: This is a legacy Response format and will be updated when Quote API responses are updated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResponseLegacy<T> {
    /// Status of the response (Ok or Error)
    pub status: Status,

    /// The response payload when status is Ok
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<T>,

    /// Error details when status is Error
    /// Only present when an error occurs
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

#[cfg(test)]
mod tests {
    use super::{Response, Status};
    use axum::{http::StatusCode, response::IntoResponse};

    #[test]
    fn test_response_ok() {
        let response = Response::ok("test data");
        let body = response.into_json();
        assert_eq!(body.status, Status::Ok);
        assert_eq!(body.result, Some("test data"));
        assert_eq!(body.error, None);
    }

    #[test]
    fn test_response_error() {
        let response = Response::<String>::error("test error", StatusCode::OK);
        let body = response.into_json();
        assert_eq!(body.status, Status::Error);
        assert_eq!(body.result, None);
        assert_eq!(body.error, Some("test error".to_string()));
    }

    #[test]
    fn test_response_into_response() {
        let response = Response::ok("test data");
        let response = response.into_response();
        assert!(response.headers().contains_key("content-type"));
        assert_eq!(response.headers()["content-type"], "application/json");
    }

    #[test]
    fn test_status_serialization() {
        let status = Status::Ok;
        let serialized = serde_json::to_string(&status).unwrap();
        assert_eq!(serialized, "\"Ok\"");

        let status = Status::Error;
        let serialized = serde_json::to_string(&status).unwrap();
        assert_eq!(serialized, "\"Error\"");
    }

    #[test]
    fn test_error_with_different_types() {
        // Test with a &str
        let response = Response::<()>::error("error message", StatusCode::OK);
        let body = response.into_json();
        assert_eq!(body.error, Some("error message".to_string()));

        // Test with a String
        let error_string = "another error".to_string();
        let response = Response::<()>::error(error_string, StatusCode::OK);
        let body = response.into_json();
        assert_eq!(body.error, Some("another error".to_string()));

        // Test with a custom error type
        #[derive(Debug)]
        struct CustomError(&'static str);
        impl ToString for CustomError {
            fn to_string(&self) -> String {
                self.0.to_string()
            }
        }

        let custom_error = CustomError("custom error");
        let response = Response::<()>::error(custom_error, StatusCode::OK);
        let body = response.into_json();
        assert_eq!(body.error, Some("custom error".to_string()));
    }
}
