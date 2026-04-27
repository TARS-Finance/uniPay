use bigdecimal::BigDecimal;
use primitives::AssetId;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Represents a fee structure with both fixed and percentage components.
///
/// Fees can be applied to trading routes and support two types:
/// - Fixed fees (in USD, supports decimals like 0.20 for $0.20)
/// - Percentage-based fees (in basis points where 1 bps = 0.01%)
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq)]
pub struct Fee {
    /// Fixed fee amount in USD (e.g., 10.0 for $10.00, 0.20 for $0.20)
    pub fixed: f64,
    /// Percentage fee in basis points (1 bips = 0.01%)
    pub percent_bips: u32,
}

/// Represents a source amount structure with minimum and maximum values.
///
/// Source amounts define the valid range for source asset amounts in a trade.
#[derive(Deserialize, Debug, Clone, Serialize, PartialEq)]
pub struct SourceAmount {
    /// Minimum source amount for the source asset
    pub min: BigDecimal,
    /// Maximum source amount for the source asset
    pub max: BigDecimal,
}

/// Default policy type for a solver.
///
/// Determines the base behavior when no specific rules apply:
/// - `Open`: Assets can trade freely unless explicitly restricted
/// - `Closed`: All trades are blocked unless explicitly allowed
#[derive(Deserialize, Debug, Clone, Serialize)]
pub enum DefaultPolicy {
    /// Open policy - allow trades unless explicitly blocked
    #[serde(rename = "open")]
    Open,
    /// Closed policy - block trades unless explicitly allowed
    #[serde(rename = "closed")]
    Closed,
}

/// Unified route override structure containing all optional route-specific overrides.
///
/// This structure allows a single route to specify multiple types of overrides
/// (fee, max slippage, confirmation target, source amount) in one place.
/// All fields are optional, allowing routes to override only the specific values they need.
///
/// # Route Format
///
/// Routes are specified as strings in the format:
/// - Forward: `"source -> destination"` (e.g., `"bitcoin:btc -> ethereum:wbtc"`)
/// - Bidirectional: `"asset1 <-> asset2"` (applies in both directions)
#[derive(Deserialize, Debug, Clone, Serialize)]
pub struct RouteOverride {
    /// The route string, e.g., "bitcoin:btc -> ethereum:wbtc"
    pub route: String,
    /// Optional fee override for this route
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee: Option<Fee>,
    /// Optional max slippage override for this route
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_slippage: Option<u64>,
    /// Optional confirmation target override for this route
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_target: Option<u64>,
    /// Optional source amount override for this route
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_amount: Option<SourceAmount>,
}

/// Configuration for a solver's trading policy rules and fee structure.
///
/// This structure defines all the rules that govern which asset pairs can trade,
/// how they're restricted, and what fees apply. It serves as the input to create
/// a `SolverPolicy` instance.
///
/// # Fields
///
/// - `default`: Base policy behavior (open or closed)
/// - `isolation_groups`: Restrict assets to only trade with specific other assets
/// - `blacklist_pairs`: Explicitly blocked trading pairs
/// - `whitelist_overrides`: Pairs allowed despite blacklist rules
/// - `default_fee`: Base fee structure for all routes
/// - `default_max_slippage`: Default max slippage for all routes
/// - `default_confirmation_target`: Default confirmation target for all routes
/// - `overrides`: Unified route-specific overrides (fee, slippage, confirmation target, source amount)
/// - `max_limits`: Maximum source liquidity limit for assets
#[derive(Deserialize, Debug, Clone, Serialize)]
pub struct SolverPolicyConfig {
    /// Solver id
    pub solver_id: String,

    /// Default policy setting (e.g., "open", "closed")
    pub default: DefaultPolicy,
    /// List of isolation groups (e.g., "bitcoin:btc -> ethereum:wbtc")
    pub isolation_groups: Vec<String>,
    /// List of blacklist pairs
    pub blacklist_pairs: Vec<String>,
    /// List of whitelist overrides
    pub whitelist_overrides: Vec<String>,
    /// Default fee structure for this solver
    pub default_fee: Fee,
    /// Default max slippage
    pub default_max_slippage: u64,
    /// Default confirmation target
    pub default_confirmation_target: u64,
    /// Unified route-specific overrides for fees, slippage, confirmation targets, and source amounts
    pub overrides: Vec<RouteOverride>,
    /// Maximum source liquidity limit for assets
    pub max_limits: HashMap<AssetId, BigDecimal>,
}

impl From<SolverInfo> for SolverPolicyConfig {
    /// Extracts the policy configuration from solver information.
    ///
    /// This conversion is useful when fetching solver data from a registry
    /// and needing to create a `SolverPolicy` instance.
    fn from(solver: SolverInfo) -> Self {
        Self {
            solver_id: solver.id,
            default: solver.policy.default,
            isolation_groups: solver.policy.isolation_groups,
            blacklist_pairs: solver.policy.blacklist_pairs,
            whitelist_overrides: solver.policy.whitelist_overrides,
            default_fee: solver.policy.default_fee,
            default_max_slippage: solver.policy.default_max_slippage,
            default_confirmation_target: solver.policy.default_confirmation_target,
            overrides: solver.policy.overrides,
            max_limits: solver.policy.max_limits,
        }
    }
}

/// Information about a blockchain and the solver's presence on it.
///
/// Each `ChainInfo` represents one blockchain where the solver operates,
/// including the solver's address and the assets it supports on that chain.
#[derive(Deserialize, Debug, Clone, Serialize)]
pub struct ChainInfo {
    /// The chain identifier (e.g., "bitcoin", "ethereum", "starknet")
    pub chain: String,
    /// The address of the solver on this chain
    pub address: String,
    /// The assets supported by the solver on this chain (e.g., ["bitcoin:btc", "ethereum:wbtc"])
    pub assets: Vec<String>,
}

/// Complete information about a solver including its policy and chain presence.
///
/// `SolverInfo` is typically fetched from a registry and contains all the
/// information needed to interact with and validate trades through a solver.
#[derive(Deserialize, Debug, Clone, Serialize)]
pub struct SolverInfo {
    /// Unique identifier for the solver
    pub id: String,
    /// Chains and assets associated with the solver
    pub chains: Vec<ChainInfo>,
    /// Policy rules for the solver
    pub policy: SolverPolicyConfig,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solver_info_deserialization() {
        let chains_json = r#"[
            {
                "chain": "bitcoin",
                "address": "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh",
                "assets": ["btc", "wbtc"]
            }
        ]"#;
        let policy_json = r#"{
            "default": "open",
            "isolation_groups": ["bitcoin:btc -> ethereum:wbtc"],
            "blacklist_pairs": [],
            "whitelist_overrides": [],
            "default_fee": {
                "fixed": 10.0,
                "percent_bips": 50
            },
            "overrides": []
        }"#;

        let chains: Vec<ChainInfo> = serde_json::from_str(chains_json).unwrap();
        let policy: SolverPolicyConfig = serde_json::from_str(policy_json).unwrap();

        let solver = SolverInfo {
            id: "solver-123".to_string(),
            chains,
            policy,
        };

        assert_eq!(solver.id, "solver-123");
        assert_eq!(solver.chains.len(), 1);
        assert_eq!(solver.policy.default_fee.fixed, 10.0);
    }

    #[test]
    fn test_fee_serialization() {
        let fee = Fee {
            fixed: 10.0,
            percent_bips: 50,
        };

        let json = serde_json::to_string(&fee).unwrap();
        let deserialized: Fee = serde_json::from_str(&json).unwrap();

        assert_eq!(fee, deserialized);
    }

    #[test]
    fn test_route_override_serialization() {
        let route_override = RouteOverride {
            route: "bitcoin:btc -> ethereum:wbtc".to_string(),
            fee: Some(Fee {
                fixed: 5.0,
                percent_bips: 25,
            }),
            max_slippage: None,
            confirmation_target: None,
            source_amount: None,
        };

        let json = serde_json::to_string(&route_override).unwrap();
        let deserialized: RouteOverride = serde_json::from_str(&json).unwrap();

        assert_eq!(route_override.route, deserialized.route);
        assert_eq!(route_override.fee, deserialized.fee);
    }
}
