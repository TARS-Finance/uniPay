use thiserror::Error;

#[derive(Error, Debug)]
pub enum PolicyError {
    #[error("Invalid asset id: {0}, {1}")]
    InvalidAssetId(String, String),
    #[error("Invalid asset pair: {0}, {1}")]
    InvalidAssetPair(String, String),
}
