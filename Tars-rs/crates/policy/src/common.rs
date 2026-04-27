use primitives::{AssetId, AssetPair, PairDirection};

/// Checks if two `AssetId`s match, considering wildcards.
///
/// Returns `true` if the `chain` or `token` fields of either `source` or `destination`
/// are wildcards (`"*"`) or if they are equal. Both the chain and token must match
/// (either by wildcard or equality) for the function to return `true`.
///
/// # Arguments
///
/// * `source` - The source `AssetId`.
/// * `destination` - The destination `AssetId`.
pub fn is_asset_id_match(source: &AssetId, destination: &AssetId) -> bool {
    // Check for wildcards in both source and destination AssetId fields
    let chain_match = source.chain() == "*"
        || destination.chain() == "*"
        || source.chain() == destination.chain();
    let token_match = source.token() == "*"
        || destination.token() == "*"
        || source.token() == destination.token();
    chain_match && token_match
}

/// Checks if two `AssetPair`s match, considering wildcards and direction.
///
/// Returns `true` if:
/// - The source `AssetId`s match (with wildcard support)
/// - The destination `AssetId`s match (with wildcard support)
/// - The directions are compatible:
///   - If either direction is `Both`, they match
///   - If both directions are `Forward`, they match
///
/// # Arguments
///
/// * `pair1` - The first `AssetPair`.
/// * `pair2` - The second `AssetPair`.
///
/// # Examples
///
/// ```
/// use primitives::{AssetId, AssetPair, PairDirection};
/// use std::str::FromStr;
///
/// let pair1 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
/// let pair2 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
/// // This would return true (exact match)
///
/// let pair3 = AssetPair::from_str("*:btc -> ethereum:eth").unwrap();
/// // This would match with pair1 (wildcard on source chain)
///
/// let pair4 = AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap();
/// // This would match with pair1 (Both direction matches Forward)
/// ```
pub fn is_asset_pair_match(pair1: &AssetPair, pair2: &AssetPair) -> bool {
    // Destructure both pairs
    let (source1, direction1, destination1) = (&pair1.0, &pair1.1, &pair1.2);
    let (source2, direction2, destination2) = (&pair2.0, &pair2.1, &pair2.2);

    // Check if source AssetIds match
    let source_match = is_asset_id_match(source1, source2);

    // Check if destination AssetIds match
    let destination_match = is_asset_id_match(destination1, destination2);

    // Check if directions are compatible
    // Both direction is more permissive and matches with either Forward or Both
    let direction_match = matches!(
        (direction1, direction2),
        (PairDirection::Both, _)
            | (_, PairDirection::Both)
            | (PairDirection::Forward, PairDirection::Forward)
    );

    source_match && destination_match && direction_match
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[tokio::test]
    async fn test_asset_pair_match_exact() {
        // Test exact matches
        let pair1 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair2 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair1, &pair2),
            "Exact match should return true"
        );

        // Test exact match with Both direction
        let pair3 = AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap();
        let pair4 = AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair3, &pair4),
            "Exact bidirectional match should return true"
        );
    }

    #[tokio::test]
    async fn test_asset_pair_match_wildcard_source() {
        // Test wildcard on source chain
        let pair1 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair2 = AssetPair::from_str("*:btc -> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair1, &pair2),
            "Wildcard on source chain should match"
        );

        // Test wildcard on source token
        let pair3 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair4 = AssetPair::from_str("bitcoin:* -> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair3, &pair4),
            "Wildcard on source token should match"
        );

        // Test wildcard on both source fields
        let pair5 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair6 = AssetPair::from_str("*:* -> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair5, &pair6),
            "Wildcard on both source fields should match"
        );
    }

    #[tokio::test]
    async fn test_asset_pair_match_wildcard_destination() {
        // Test wildcard on destination chain
        let pair1 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair2 = AssetPair::from_str("bitcoin:btc -> *:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair1, &pair2),
            "Wildcard on destination chain should match"
        );

        // Test wildcard on destination token
        let pair3 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair4 = AssetPair::from_str("bitcoin:btc -> ethereum:*").unwrap();
        assert!(
            is_asset_pair_match(&pair3, &pair4),
            "Wildcard on destination token should match"
        );

        // Test wildcard on both destination fields
        let pair5 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair6 = AssetPair::from_str("bitcoin:btc -> *:*").unwrap();
        assert!(
            is_asset_pair_match(&pair5, &pair6),
            "Wildcard on both destination fields should match"
        );
    }

    #[tokio::test]
    async fn test_asset_pair_match_wildcard_both() {
        // Test wildcards on both source and destination
        let pair1 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair2 = AssetPair::from_str("*:btc -> ethereum:*").unwrap();
        assert!(
            is_asset_pair_match(&pair1, &pair2),
            "Wildcards on both source and destination should match"
        );

        // Test full wildcard pair
        let pair3 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair4 = AssetPair::from_str("*:* -> *:*").unwrap();
        assert!(
            is_asset_pair_match(&pair3, &pair4),
            "Full wildcard pair should match any pair"
        );
    }

    #[tokio::test]
    async fn test_asset_pair_match_direction() {
        let pair_forward = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair_both = AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap();

        // Forward should match Forward
        let pair_forward2 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair_forward, &pair_forward2),
            "Forward should match Forward"
        );

        // Both should match Forward
        assert!(
            is_asset_pair_match(&pair_forward, &pair_both),
            "Both direction should match Forward"
        );

        // Forward should match Both (symmetric)
        assert!(
            is_asset_pair_match(&pair_both, &pair_forward),
            "Forward should match Both (symmetric)"
        );

        // Both should match Both
        let pair_both2 = AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair_both, &pair_both2),
            "Both should match Both"
        );
    }

    #[tokio::test]
    async fn test_asset_pair_no_match() {
        // Different source chains (no wildcard)
        let pair1 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair2 = AssetPair::from_str("solana:btc -> ethereum:eth").unwrap();
        assert!(
            !is_asset_pair_match(&pair1, &pair2),
            "Different source chains should not match"
        );

        // Different source tokens (no wildcard)
        let pair3 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair4 = AssetPair::from_str("bitcoin:usdc -> ethereum:eth").unwrap();
        assert!(
            !is_asset_pair_match(&pair3, &pair4),
            "Different source tokens should not match"
        );

        // Different destination chains (no wildcard)
        let pair5 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair6 = AssetPair::from_str("bitcoin:btc -> solana:eth").unwrap();
        assert!(
            !is_asset_pair_match(&pair5, &pair6),
            "Different destination chains should not match"
        );

        // Different destination tokens (no wildcard)
        let pair7 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair8 = AssetPair::from_str("bitcoin:btc -> ethereum:usdc").unwrap();
        assert!(
            !is_asset_pair_match(&pair7, &pair8),
            "Different destination tokens should not match"
        );
    }

    #[tokio::test]
    async fn test_asset_pair_match_complex() {
        // Complex scenario with multiple wildcards
        let pair1 = AssetPair::from_str("starknet:usdc <-> solana:usdc").unwrap();
        let pair2 = AssetPair::from_str("*:usdc -> *:usdc").unwrap();
        assert!(
            is_asset_pair_match(&pair1, &pair2),
            "Complex wildcard with Both direction should match Forward"
        );

        // Wildcard on one side only matches specific on other
        let pair3 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair4 = AssetPair::from_str("bitcoin:* -> ethereum:eth").unwrap();
        assert!(
            is_asset_pair_match(&pair3, &pair4),
            "Specific token should match wildcard token"
        );

        // Wildcards should not match when other fields differ
        let pair5 = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let pair6 = AssetPair::from_str("*:usdc -> ethereum:eth").unwrap();
        assert!(
            !is_asset_pair_match(&pair5, &pair6),
            "Wildcard should not match when specific fields differ"
        );
    }
}
