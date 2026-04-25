//! Generic UTXO transaction builder traits.
//!
//! Vendored from standard-rs `deps.rs` with `eyre::Result` replaced by a local
//! `TxBuilderError` type.  These traits abstract the chain-specific pieces so
//! that the iterative fee-builder algorithm (`fee_builder.rs`) can remain
//! chain-agnostic.

use async_trait::async_trait;

// ── Error type ──────────────────────────────────────────────────────────────

/// Errors that can occur during transaction building.
#[derive(Debug, thiserror::Error)]
pub enum TxBuilderError {
    #[error("validation error: {0}")]
    Validation(String),

    #[error("insufficient UTXOs: need {needed} sats, have {available} sats")]
    InsufficientFunds { needed: u64, available: u64 },

    #[error("fee builder exceeded max iterations")]
    MaxIterationsExceeded,

    #[error("bitcoin consensus error: {0}")]
    Consensus(String),

    #[error("sighash error: {0}")]
    Sighash(String),

    #[error("client error: {0}")]
    Client(String),
}

// ── Generic UTXO traits ──────────────────────────────────────────────────────

/// Provides cover UTXOs from the wallet to pay transaction fees.
///
/// Cover UTXOs are wallet-owned P2TR outputs that are added to a transaction
/// alongside SACP (HTLC spend) inputs to cover the miner fee and produce
/// change.
#[async_trait]
pub trait CoverUtxoProvider {
    type Utxo;
    type Tx;

    /// Returns the UTXOs that have already been selected by this provider.
    fn selected(&self) -> &[Self::Utxo];

    /// Adds pre-selected UTXOs (e.g. from a previous iteration).
    fn add(&mut self, utxos: Vec<Self::Utxo>);

    /// Returns all UTXOs available for selection, excluding those already
    /// selected and those spent by the given transaction.
    async fn available(&self, tx: &Self::Tx) -> Result<Vec<Self::Utxo>, TxBuilderError>;

    /// Selects UTXOs whose total value is at least `needed` sats.
    ///
    /// Uses a greedy algorithm: sort by value descending, accumulate until
    /// the target is met.
    fn select(
        &self,
        utxos: Vec<Self::Utxo>,
        needed: u64,
    ) -> Result<Vec<Self::Utxo>, TxBuilderError>;
}

/// Encode/decode a chain-specific transaction to/from raw bytes.
pub trait UtxoChainTxCodec {
    type Tx;

    /// Encode a transaction into deterministic raw bytes for size accounting or
    /// fallback reconstruction.
    fn encode(&self, tx: &Self::Tx) -> Result<Vec<u8>, TxBuilderError>;
    /// Decode a previously encoded transaction.
    fn decode(&self, data: &[u8]) -> Result<Self::Tx, TxBuilderError>;
}

/// Estimates fees for a chain-specific transaction.
///
/// `current()` returns the fee currently embedded in the transaction
/// (sum(inputs) - sum(outputs)).  `target()` returns the fee the transaction
/// *should* pay given the current fee-rate and its virtual size.
pub trait UtxoChainTxFeeEstimator: UtxoChainTxCodec {
    type Params;
    type CoverUtxoProvider;

    /// The fee the transaction currently pays (may be negative if outputs
    /// exceed inputs — meaning more cover UTXOs are needed).
    fn current(
        &self,
        params: &Self::Params,
        cover: &Self::CoverUtxoProvider,
        tx: &Self::Tx,
    ) -> Result<i64, TxBuilderError>;

    /// The fee the transaction *should* pay at the requested fee-rate.
    fn target(&self, params: &Self::Params, tx: &Self::Tx) -> Result<u64, TxBuilderError>;
}

/// Builds a chain-specific transaction from SACP spends, send outputs,
/// cover UTXOs, and a change amount.
pub trait UtxoChainTxAdaptor: UtxoChainTxFeeEstimator + UtxoChainTxCodec {
    type Utxo;

    /// Build a chain transaction from mandatory inputs, fee-cover UTXOs, and
    /// the current proposed change amount.
    fn build(
        &self,
        params: &Self::Params,
        cover_utxos: &[Self::Utxo],
        change: u64,
    ) -> Result<Self::Tx, TxBuilderError>;
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_display_validation() {
        let err = TxBuilderError::Validation("empty inputs".into());
        assert_eq!(err.to_string(), "validation error: empty inputs");
    }

    #[test]
    fn error_display_insufficient_funds() {
        let err = TxBuilderError::InsufficientFunds {
            needed: 10_000,
            available: 5_000,
        };
        assert!(err.to_string().contains("10000"));
        assert!(err.to_string().contains("5000"));
    }

    #[test]
    fn error_display_max_iterations() {
        let err = TxBuilderError::MaxIterationsExceeded;
        assert!(err.to_string().contains("max iterations"));
    }
}
