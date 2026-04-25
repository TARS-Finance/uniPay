//! Runtime configuration for the Bitcoin wallet runner.
//!
//! The values here tune batching cadence, fee bounds, pending-request ageing,
//! and how aggressively the runtime treats change outputs as reusable chain
//! anchors for follow-up requests.

use crate::infrastructure::chain::bitcoin::tx_builder::DUST_LIMIT;

#[derive(Clone, Debug, PartialEq)]
pub struct WalletConfig {
    /// Runner tick cadence in seconds.
    pub tick_interval_secs: u64,
    /// Consecutive missing observations before a lineage is reconciled.
    pub missing_batch_threshold: u32,
    /// Hard cap on spend/send outputs in a single built transaction.
    pub max_outputs_per_batch: usize,
    /// Pending requests older than this are dropped instead of retried forever.
    pub max_pending_ttl_secs: u64,
    /// Upper fee-rate clamp accepted by the runtime when quoting/bumping fees.
    pub max_fee_rate: f64,
    /// Lower fee-rate floor used when fee providers return implausibly low data.
    pub min_fee_rate: f64,
    /// Smallest change output the builder will intentionally keep.
    pub min_change_value: u64,
    /// Number of unrelated live lineages allowed at once.
    pub max_concurrent_lineages: usize,
    /// Minimum confirmations before an anchor is considered final and expired.
    pub chain_anchor_confirmations: u64,
}

impl Default for WalletConfig {
    fn default() -> Self {
        Self {
            tick_interval_secs: 10,
            missing_batch_threshold: 3,
            max_outputs_per_batch: 96,
            max_pending_ttl_secs: 3600,
            max_fee_rate: 100.0,
            min_fee_rate: 1.0,
            min_change_value: DUST_LIMIT,
            max_concurrent_lineages: 8,
            chain_anchor_confirmations: 6,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::infrastructure::chain::bitcoin::tx_builder::DUST_LIMIT;

    use super::WalletConfig;

    #[test]
    fn wallet_config_default_matches_spec_defaults() {
        let config = WalletConfig::default();
        assert_eq!(config.tick_interval_secs, 10);
        assert_eq!(config.missing_batch_threshold, 3);
        assert_eq!(config.max_outputs_per_batch, 96);
        assert_eq!(config.max_pending_ttl_secs, 3600);
        assert_eq!(config.max_fee_rate, 100.0);
        assert_eq!(config.min_fee_rate, 1.0);
        assert_eq!(config.min_change_value, DUST_LIMIT);
        assert_eq!(config.max_concurrent_lineages, 8);
        assert_eq!(config.chain_anchor_confirmations, 6);
    }
}
