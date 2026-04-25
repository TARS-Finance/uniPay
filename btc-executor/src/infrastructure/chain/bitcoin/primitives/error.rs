//! Error types for Bitcoin HTLC primitives.

/// Errors that can occur during Bitcoin HTLC primitive operations.
#[derive(Debug, thiserror::Error)]
pub enum BitcoinPrimitivesError {
    /// secp256k1 cryptographic error
    #[error("secp256k1 error: {0}")]
    Secp(#[from] bitcoin::secp256k1::Error),

    /// Sighash computation error
    #[error("sighash error: {0}")]
    Sighash(String),

    /// Hex decoding error
    #[error("hex decode error: {0}")]
    Hex(#[from] hex::FromHexError),

    /// Invalid parameter provided
    #[error("invalid parameter: {0}")]
    InvalidParam(String),

    /// Signing failure
    #[error("signing error: {0}")]
    Signing(String),

    /// Taproot tree construction error
    #[error("taproot error: {0}")]
    Taproot(String),
}
