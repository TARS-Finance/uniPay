use thiserror::Error;

/// OrderbookError contains various errors returned by Orderbook trait implementations
#[derive(Debug, Error)]
pub enum OrderbookError {
    /// Error when the order is not found
    #[error("Order not found for order_id: {order_id}")]
    OrderNotFound { order_id: String },

    /// Error when the swap is not found with the given order_id
    #[error("Swap not found for order_id: {order_id}")]
    SwapNotFound { order_id: String },

    /// Error when the database operation fails
    #[error("Database error: {0}")]
    DatabaseError(#[from] sqlx::Error),

    /// Error when the internal operation fails such as getting current time
    #[error("Internal error: {0}")]
    InternalError(String),

    /// Error when pagination params are invalid
    #[error("Invalid pagination params: {0}")]
    InvalidParams(String),

    /// Error when invalid timestamp is provided
    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(String),

    /// Order already exists
    #[error("Order already exists: {0}")]
    OrderAlreadyExists(String),
}
