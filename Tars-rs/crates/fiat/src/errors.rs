#[derive(Debug, thiserror::Error)]
pub enum FiatError {
    #[error("Fiat Provider creation failed: {0}")]
    FiatProviderCreationFailed(String),

    #[error("Fiat price Api request failed: {0}")]
    FiatApiRequestFailed(String),

    #[error("Fiat price Api error : {0}")]
    FiatApiError(String),
}
