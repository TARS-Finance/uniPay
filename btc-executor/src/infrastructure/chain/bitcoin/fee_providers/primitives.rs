//! Fee estimate primitives for Bitcoin fee rate providers.
//!
//! Aligned with garden-rs `FeeEstimate` and `FeeLevel` for multi-provider
//! fee rate aggregation.

/// Fee level for selecting which estimate to use.
///
/// Munger uses `HalfHour` (3-block target) for HTLC operations by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FeeLevel {
    /// Fastest possible fee rate (1-block target)
    Fastest,
    /// ~30 minutes (3-block target) — suitable for HTLC operations
    #[default]
    HalfHour,
    /// ~60 minutes (6-block target)
    Hour,
    /// Minimum acceptable fee rate
    Minimum,
    /// Economical fee rate
    Economy,
}

/// Unified fee rate suggestions (sat/vB) from a fee provider.
#[derive(Debug, Clone)]
pub struct FeeEstimate {
    pub fastest_fee: f64,
    pub half_hour_fee: f64,
    pub hour_fee: f64,
    pub minimum_fee: f64,
    pub economy_fee: f64,
}

impl Default for FeeEstimate {
    fn default() -> Self {
        Self {
            fastest_fee: 2.0,
            half_hour_fee: 2.0,
            hour_fee: 2.0,
            minimum_fee: 2.0,
            economy_fee: 2.0,
        }
    }
}

impl FeeLevel {
    /// Returns the fee rate for this level from a `FeeEstimate`.
    pub fn from_estimate(&self, estimate: &FeeEstimate) -> f64 {
        match self {
            FeeLevel::Fastest => estimate.fastest_fee,
            FeeLevel::HalfHour => estimate.half_hour_fee,
            FeeLevel::Hour => estimate.hour_fee,
            FeeLevel::Minimum => estimate.minimum_fee,
            FeeLevel::Economy => estimate.economy_fee,
        }
    }
}
