use crate::FeeRateEstimator;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Default fee level.
pub const DEFAULT_FEE_LEVEL: FeeLevel = FeeLevel::Fastest;

/// A unified structure representing fee rate suggestions (in sat/vB)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeEstimate {
    /// Fastest possible fee rate in sat/vB
    pub fastest_fee: f64,

    /// Fee rate for confirmation within ~30 minutes (half an hour) in sat/vB
    pub half_hour_fee: f64,

    /// Fee rate for confirmation within ~60 minutes (one hour) in sat/vB
    pub hour_fee: f64,

    /// Minimum acceptable fee rate in sat/vB
    pub minimum_fee: f64,

    /// Economical fee rate suitable for low-priority transactions in sat/vB
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
/// Represents different levels of transaction fee priorities for Bitcoin transactions.
#[derive(Debug, Clone, Deserialize, Copy, PartialEq, Eq)]
pub enum FeeLevel {
    /// Fastest possible fee rate
    Fastest,
    /// Fee rate for confirmation within ~30 minutes (half an hour)
    HalfHour,
    /// Fee rate for confirmation within ~60 minutes (one hour)
    Hour,
    /// Minimum acceptable fee rate
    Minimum,
    /// Economical fee rate suitable for low-priority transactions
    Economy,
}

/// Default fee level.  
impl Default for FeeLevel {
    fn default() -> Self {
        FeeLevel::Fastest
    }
}

impl FeeLevel {
    /// Retrieves the fee rate associated with the selected fee level from a `FeeEstimate`.
    ///
    /// This function returns the fee rate corresponding to the chosen `FeeLevel` from the provided
    /// `FeeEstimate`. The `FeeEstimate` provides different fee estimates for various transaction
    /// confirmation speeds.
    ///
    /// # Arguments
    /// * `fee_estimate` - A reference to a `FeeEstimate` object that contains the fee rates for different levels of transaction priority.
    ///
    /// # Returns
    /// * `f64` - The fee rate for the selected fee level.
    pub fn from(self, fee_estimate: &FeeEstimate) -> f64 {
        match self {
            FeeLevel::Fastest => fee_estimate.fastest_fee,
            FeeLevel::HalfHour => fee_estimate.half_hour_fee,
            FeeLevel::Hour => fee_estimate.hour_fee,
            FeeLevel::Minimum => fee_estimate.minimum_fee,
            FeeLevel::Economy => fee_estimate.economy_fee,
        }
    }
}

/// A type alias for a thread-safe, reference-counted `FeeRateEstimator` trait object.
pub type ArcFeeRateEstimator = Arc<dyn FeeRateEstimator + Send + Sync>;
