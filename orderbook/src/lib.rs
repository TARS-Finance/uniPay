//! Unified quote and orderbook service built on top of `tars-rs`.

/// Shared application state and dependency wiring.
pub mod app_state;
/// Common helpers used across protocol-specific modules.
pub mod common;
/// Runtime configuration models loaded from `Settings.toml`.
pub mod config;
/// HTTP-friendly application error type.
pub mod error;
/// Solver balance fetching and liquidity tracking.
pub mod liquidity;
/// Chain and asset metadata loaded from `chain.json`.
pub mod metadata;
/// Order creation, swap ID generation, and request models.
pub mod orders;
/// Asset USD pricing refresh and cache logic.
pub mod pricing;
/// Quote request models and matching logic.
pub mod quote;
/// Read-only wrappers over the persisted orderbook.
pub mod read_api;
/// Strategy loading and supported-pair derivation.
pub mod registry;
/// HTTP routes, handlers, and response wrappers.
pub mod server;

pub use app_state::AppState;
