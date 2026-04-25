//! Fee estimation used by the Bitcoin wallet runner.
//!
//! The standalone executor only wires Electrs-based estimation, so the copied
//! public API surface is kept intentionally small.

pub mod constants;
pub mod electrs;
pub mod primitives;
pub mod traits;

pub use electrs::ElectrsFeeRateEstimator;
pub use primitives::{FeeEstimate, FeeLevel};
pub use traits::{FeeEstimatorError, FeeRateEstimator};
