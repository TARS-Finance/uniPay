//! UTXO transaction builder — generic traits + Bitcoin adaptor.
//!
//! Provides iterative fee adjustment, SACP (HTLC) inputs, cover UTXO
//! selection, and change management for Bitcoin batch transactions.

pub mod builder;
pub mod cover_utxo;
pub mod deps;
pub mod fee;
pub mod fee_builder;
pub mod primitives;
pub mod tx_adaptor;
pub mod validation;

pub use builder::{BitcoinTxBuilder, BuildTxReceipt};
pub use deps::TxBuilderError;
pub use primitives::*;
