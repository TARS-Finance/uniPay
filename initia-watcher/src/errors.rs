use thiserror::Error;

pub type Result<T> = std::result::Result<T, WatcherError>;

#[derive(Error, Debug)]
pub enum WatcherError {
    #[error("Failed to fetch logs for {0} from block {1} to {2}: {3}")]
    FetchLogs(String, u64, u64, String),

    #[error("Missing topic at index {0}")]
    MissingTopic(usize),

    #[error("Database error: {0}")]
    Database(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("RPC error: {0}")]
    Rpc(String),

    #[error("ABI decode error: {0}")]
    Decode(String),

    #[error("Invalid address: {0}")]
    InvalidAddress(String),
}

impl From<sqlx::Error> for WatcherError {
    fn from(e: sqlx::Error) -> Self {
        WatcherError::Database(e.to_string())
    }
}
