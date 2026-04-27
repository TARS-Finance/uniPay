use alloy::primitives::U256;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum EvmError {
    #[error("Contract error: {0}")]
    ContractError(#[from] alloy::contract::Error),

    #[error("Pending transaction error: {0}")]
    PendingTransactionError(#[from] alloy::providers::PendingTransactionError),

    #[error("Transport error: {0}")]
    TransportError(#[from] alloy::transports::TransportError),

    #[error("RPC request failed: {0}")]
    RequestFailed(String),

    #[error("Failed to decode address: {0}")]
    DecodeAddressError(#[from] alloy::hex::FromHexError),

    #[error("Failed to check and set allowance: {0}")]
    AllowanceError(String),

    #[error("Failed to sign typed data: {0}")]
    SignatureError(#[from] alloy::signers::Error),
}

#[derive(Error, Debug)]
pub enum HTLCError {
    #[error("Swap expired. Need {needed} more blocks")]
    NotExpired { needed: U256 },

    #[error("Swap already redeemed")]
    AlreadyRedeemed,

    #[error("Failed to simulate {action}: {reason}")]
    SimulationFailed { action: String, reason: String },

    #[error("Cannot do initiate_with_signature for native tokens")]
    NativeTokenNotSupported,

    #[error(transparent)]
    EvmError(#[from] EvmError),

    #[error("Action not supported: {action}")]
    UnsupportedAction { action: String },

    #[error("Unsupported version: {version} for HTLC contract {asset}")]
    UnsupportedVersion { version: String, asset: String },
}

#[derive(Error, Debug)]
pub enum MulticallError {
    #[error("Multicall error: {0}")]
    Error(String),

    #[error(transparent)]
    HTLCError(#[from] HTLCError)
}

/// Sanitizes EVM/RPC errors to remove sensitive information (like URLs)
pub fn sanitize_error(err: impl AsRef<str>) -> String {
    let raw_error = err.as_ref();

    if raw_error.contains("error sending request for url") {
        return "error sending request to RPC".to_string();
    } else if raw_error.contains("connection error:") {
        return "RPC connection failed".to_string();
    } else if raw_error.contains("reqwest::Error") {
        return "network error".to_string();
    }

    raw_error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_error() {
        let test_cases = [
            (
                "error sending request for url http://example.com",
                "error sending request to RPC",
            ),
            (
                "connection error: failed to connect to host",
                "RPC connection failed",
            ),
            ("reqwest::Error: something went wrong", "network error"),
            ("some other error", "some other error"),
        ];

        for (input, expected) in test_cases {
            let sanitized = sanitize_error(input);
            assert_eq!(sanitized, expected);
        }
    }
}
