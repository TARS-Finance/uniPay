use crate::common::is_asset_pair_match;
use primitives::{AssetId, AssetPair, PairDirection};
use std::cmp::Ordering;

/// A wrapper around Vec that supports wildcard matching for asset pairs.
///
/// Uses `AssetPair` as keys and leverages the `is_asset_pair_match` function
/// for wildcard matching. Pairs are automatically sorted by specificity
/// (most specific first), ensuring that exact matches take precedence over
/// wildcard rules during lookups.
#[derive(Debug, Clone)]
pub struct PolicyMap<T> {
    inner: Vec<(AssetPair, T)>,
}

impl<T> PolicyMap<T> {
    /// Creates a new empty PolicyMap.
    pub fn new() -> Self {
        Self { inner: Vec::new() }
    }

    /// Creates a new empty PolicyMap with the specified capacity.
    ///
    /// The map will be able to hold at least `capacity` elements without reallocating.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Vec::with_capacity(capacity),
        }
    }

    /// Inserts an AssetPair-value pair into the policy map.
    ///
    /// The AssetPair can contain wildcards (*) for chains or tokens.
    /// Pairs are inserted in sorted order by specificity (most specific first).
    ///
    /// If an identical pair already exists (same source, destination, and direction),
    /// the insertion is skipped to prevent duplicates and avoid unnecessary lookups.
    pub fn insert(&mut self, pair: AssetPair, value: T) {
        // Check if exact pair already exists - skip if duplicate
        if self
            .inner
            .iter()
            .any(|(existing_pair, _)| are_asset_pairs_equal(existing_pair, &pair))
        {
            return; // Skip duplicate insertion
        }

        // New pair - find position based on specificity and insert
        let insert_pos = self
            .inner
            .binary_search_by(|(existing_pair, _)| {
                compare_asset_pair_specificity(existing_pair, &pair)
            })
            .unwrap_or_else(|pos| pos);

        self.inner.insert(insert_pos, (pair, value));
    }

    /// Retrieves a policy value for the given source and destination assets.
    ///
    /// This method iterates through stored policies (sorted by specificity)
    /// and returns the value from the first AssetPair that matches the query.
    /// Since pairs are sorted by specificity, the most specific match is
    /// always returned first.
    ///
    /// The matching uses `is_asset_pair_match` which supports:
    /// - Wildcard chains and tokens
    /// - Direction matching (Forward and Both)
    /// - Bidirectional pairs (Both) automatically match queries in either direction
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// Returns the first matching policy or None if no policy is found.
    pub fn get(&self, source: &AssetId, destination: &AssetId) -> Option<&T> {
        // Create a forward query pair (source -> destination)
        let query_pair = AssetPair(source.clone(), PairDirection::Forward, destination.clone());

        // Find the first matching pair (most specific due to sorting)
        for (stored_pair, value) in &self.inner {
            // Check forward direction match
            if is_asset_pair_match(&query_pair, stored_pair) {
                return Some(value);
            }

            // If stored pair is bidirectional, also check reverse direction
            if matches!(stored_pair.1, PairDirection::Both) {
                let reverse_query =
                    AssetPair(destination.clone(), PairDirection::Forward, source.clone());
                if is_asset_pair_match(&reverse_query, stored_pair) {
                    return Some(value);
                }
            }
        }

        None
    }

    /// Checks if a policy exists for the given source and destination assets.
    ///
    /// Uses the same wildcard matching logic as `get()`.
    pub fn contains_key(&self, source: &AssetId, destination: &AssetId) -> bool {
        self.get(source, destination).is_some()
    }

    /// Finds the first matching policy where the closure returns Some(value).
    ///
    /// This method iterates through all matching policies in precedence order
    /// (most specific first) and returns the first value where the closure
    /// returns `Some`. This is useful when you have a struct with optional fields
    /// and want to find the first match where a specific field is set.
    ///
    /// The matching uses `is_asset_pair_match` which supports:
    /// - Wildcard chains and tokens
    /// - Direction matching (Forward and Both)
    /// - Bidirectional pairs (Both) automatically match queries in either direction
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    /// * `f` - A closure that takes a reference to the value and returns `Option<R>`
    ///
    /// # Returns
    ///
    /// The first `Some(R)` found, or `None` if no match has a `Some` value.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use policy::collections::PolicyMap;
    /// # use primitives::{AssetId, AssetPair, PairDirection};
    /// # use std::str::FromStr;
    /// struct Overrides {
    ///     max_slippage: Option<u64>,
    /// }
    ///
    /// let mut map = PolicyMap::new();
    /// let pair = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
    /// map.insert(pair, Overrides { max_slippage: Some(100) });
    ///
    /// let source = AssetId::from_str("bitcoin:btc").unwrap();
    /// let dest = AssetId::from_str("ethereum:eth").unwrap();
    /// let result = map.find_map(&source, &dest, |overrides| overrides.max_slippage);
    /// assert_eq!(result, Some(100));
    /// ```
    pub fn find_map<F, R>(&self, source: &AssetId, destination: &AssetId, f: F) -> Option<R>
    where
        F: Fn(&T) -> Option<R>,
    {
        // Create a forward query pair (source -> destination)
        let query_pair = AssetPair(source.clone(), PairDirection::Forward, destination.clone());

        // Iterate through all pairs (sorted by specificity, most specific first)
        for (stored_pair, value) in &self.inner {
            // Check forward direction match
            if is_asset_pair_match(&query_pair, stored_pair) {
                if let Some(result) = f(value) {
                    return Some(result);
                }
            }

            // If stored pair is bidirectional, also check reverse direction
            if matches!(stored_pair.1, PairDirection::Both) {
                let reverse_query =
                    AssetPair(destination.clone(), PairDirection::Forward, source.clone());
                if is_asset_pair_match(&reverse_query, stored_pair) {
                    if let Some(result) = f(value) {
                        return Some(result);
                    }
                }
            }
        }

        None
    }
}

impl<T> Default for PolicyMap<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// A set-like structure for storing asset pairs without associated values.
///
/// `PolicySet` is a specialized version of `PolicyMap` that only tracks
/// membership (whether a pair exists or not) without storing any value.
/// It provides a cleaner API than `PolicyMap<()>` by hiding the unit type.
///
/// Like `PolicyMap`, it supports:
/// - Wildcard matching for chains and tokens
/// - Automatic specificity-based sorting
/// - Bidirectional pair matching
#[derive(Debug, Clone)]
pub struct PolicySet {
    inner: PolicyMap<()>,
}

impl PolicySet {
    /// Creates a new empty PolicySet.
    pub fn new() -> Self {
        Self {
            inner: PolicyMap::new(),
        }
    }

    /// Creates a new empty PolicySet with the specified capacity.
    ///
    /// The set will be able to hold at least `capacity` elements without reallocating.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: PolicyMap::with_capacity(capacity),
        }
    }

    /// Inserts an AssetPair into the set.
    ///
    /// The AssetPair can contain wildcards (*) for chains or tokens.
    /// Pairs are automatically sorted by specificity.
    pub fn insert(&mut self, pair: AssetPair) {
        self.inner.insert(pair, ());
    }

    /// Checks if the set contains a matching pair for the given source and destination.
    ///
    /// Uses wildcard matching and respects specificity ordering.
    /// Bidirectional pairs (Both) automatically match queries in either direction.
    pub fn contains(&self, source: &AssetId, destination: &AssetId) -> bool {
        self.inner.contains_key(source, destination)
    }
}

impl Default for PolicySet {
    fn default() -> Self {
        Self::new()
    }
}

/// Checks if two AssetPairs are exactly equal.
///
/// Returns `true` only if both pairs have identical:
/// - Source AssetId (chain and token)
/// - Direction (Forward or Both)
/// - Destination AssetId (chain and token)
///
/// This is used to detect duplicate insertions.
fn are_asset_pairs_equal(a: &AssetPair, b: &AssetPair) -> bool {
    a.0 == b.0 && a.1 == b.1 && a.2 == b.2
}

/// Compares two AssetPairs for specificity ordering.
///
/// Returns `Ordering::Less` if `a` is more specific than `b`.
///
/// Specificity is determined by comparing components separately with priority:
/// 1. Source specificity is compared first (higher priority)
/// 2. If source specificity is equal, destination specificity is compared
/// 3. If both are equal, lexicographic comparison for deterministic ordering
///
/// This ensures predictable ordering when pairs have the same total specificity
/// but different distributions (e.g., `bitcoin:btc -> *:*` vs `*:* -> ethereum:eth`)
///
/// # Examples
///
/// Ordering (most to least specific):
/// - `bitcoin:btc -> ethereum:eth` (2, 2) - Most specific
/// - `bitcoin:btc -> ethereum:*`   (2, 1) - Source-specific
/// - `bitcoin:btc -> *:*`          (2, 0) - Source-specific, wild dest
/// - `bitcoin:* -> ethereum:eth`   (1, 2) - Semi-wild source
/// - `*:* -> ethereum:eth`         (0, 2) - Wild source, dest-specific
/// - `*:* -> *:*`                  (0, 0) - Most generic
fn compare_asset_pair_specificity(a: &AssetPair, b: &AssetPair) -> Ordering {
    let a_source_specificity = calculate_asset_specificity(&a.0);
    let a_dest_specificity = calculate_asset_specificity(&a.2);
    let b_source_specificity = calculate_asset_specificity(&b.0);
    let b_dest_specificity = calculate_asset_specificity(&b.2);

    // Compare source specificity first (higher priority)
    match a_source_specificity.cmp(&b_source_specificity) {
        Ordering::Equal => {
            // If source specificity is equal, compare destination specificity
            match a_dest_specificity.cmp(&b_dest_specificity) {
                Ordering::Equal => {
                    // If both are equal, use lexicographic ordering for deterministic results
                    (a.0.to_string(), a.2.to_string()).cmp(&(b.0.to_string(), b.2.to_string()))
                }
                dest_order => dest_order.reverse(), // More specific destination first
            }
        }
        source_order => source_order.reverse(), // More specific source first
    }
}

/// Calculates the specificity score for an AssetId.
///
/// Returns a score from 0 to 2:
/// - 2: Both chain and token are specific (no wildcards)
/// - 1: Either chain or token is specific
/// - 0: Both are wildcards
fn calculate_asset_specificity(asset: &AssetId) -> u8 {
    let chain_specific = asset.chain() != "*";
    let token_specific = asset.token() != "*";
    (chain_specific as u8) + (token_specific as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn test_specificity_ordering() {
        let mut map = PolicyMap::new();

        // Insert in random order
        map.insert(AssetPair::from_str("*:* -> *:*").unwrap(), "most_generic");
        map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap(),
            "most_specific",
        );
        map.insert(
            AssetPair::from_str("*:btc -> ethereum:eth").unwrap(),
            "source_chain_wildcard",
        );
        map.insert(
            AssetPair::from_str("bitcoin:btc -> *:eth").unwrap(),
            "dest_chain_wildcard",
        );

        // Query should match most specific first
        let source = AssetId::from_str("bitcoin:btc").unwrap();
        let dest = AssetId::from_str("ethereum:eth").unwrap();

        assert_eq!(map.get(&source, &dest), Some(&"most_specific"));
    }

    #[test]
    fn test_wildcard_fallback() {
        let mut map = PolicyMap::new();

        // Only add wildcard rules
        map.insert(
            AssetPair::from_str("*:btc -> ethereum:*").unwrap(),
            "partial_wildcard",
        );
        map.insert(AssetPair::from_str("*:* -> *:*").unwrap(), "full_wildcard");

        // Query for specific pair
        let source = AssetId::from_str("bitcoin:btc").unwrap();
        let dest = AssetId::from_str("ethereum:eth").unwrap();

        // Should match the more specific wildcard rule
        assert_eq!(map.get(&source, &dest), Some(&"partial_wildcard"));
    }

    #[test]
    fn test_no_match() {
        let mut map = PolicyMap::new();

        map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap(),
            "btc_to_eth",
        );

        // Query for non-matching pair
        let source = AssetId::from_str("solana:sol").unwrap();
        let dest = AssetId::from_str("ethereum:eth").unwrap();

        assert_eq!(map.get(&source, &dest), None);
    }

    #[test]
    fn test_direction_matching() {
        let mut map = PolicyMap::new();

        // Store a bidirectional rule
        map.insert(
            AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap(),
            "bidirectional",
        );

        // Query in forward direction
        let source = AssetId::from_str("bitcoin:btc").unwrap();
        let dest = AssetId::from_str("ethereum:eth").unwrap();

        assert_eq!(map.get(&source, &dest), Some(&"bidirectional"));

        // Query in reverse direction should also match (bidirectional)
        assert_eq!(map.get(&dest, &source), Some(&"bidirectional"));
    }

    #[test]
    fn test_policy_set() {
        let mut set = PolicySet::new();

        // Insert some pairs
        set.insert(AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap());
        set.insert(AssetPair::from_str("*:usdc <-> *:usdc").unwrap());

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();
        let usdc_sol = AssetId::from_str("solana:usdc").unwrap();
        let usdc_eth = AssetId::from_str("ethereum:usdc").unwrap();
        let sol = AssetId::from_str("solana:sol").unwrap();

        // Test membership
        assert!(set.contains(&btc, &eth));
        assert!(!set.contains(&eth, &btc)); // Not bidirectional

        // Test bidirectional wildcard
        assert!(set.contains(&usdc_sol, &usdc_eth));
        assert!(set.contains(&usdc_eth, &usdc_sol)); // Bidirectional works

        // Test non-member
        assert!(!set.contains(&btc, &sol));

        let mut map = PolicyMap::new();

        // Insert in reverse order to test sorting
        map.insert(AssetPair::from_str("*:* -> *:*").unwrap(), "0_0");
        map.insert(AssetPair::from_str("*:* -> ethereum:eth").unwrap(), "0_2");
        map.insert(
            AssetPair::from_str("bitcoin:* -> ethereum:eth").unwrap(),
            "1_2",
        );
        map.insert(AssetPair::from_str("bitcoin:btc -> *:*").unwrap(), "2_0");
        map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:*").unwrap(),
            "2_1",
        );
        map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap(),
            "2_2",
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();

        // Most specific should match first
        assert_eq!(map.get(&btc, &eth), Some(&"2_2"));

        // Test partial specificity - only source matches exactly
        let dai = AssetId::from_str("ethereum:dai").unwrap();
        assert_eq!(
            map.get(&btc, &dai),
            Some(&"2_1"),
            "Should match bitcoin:btc -> ethereum:*"
        );

        // Test partial specificity - only destination matches exactly
        let sol = AssetId::from_str("solana:sol").unwrap();
        assert_eq!(
            map.get(&sol, &eth),
            Some(&"0_2"),
            "Should match *:* -> ethereum:eth"
        );
    }

    #[test]
    fn test_specificity_with_separate_source_dest_priority() {
        // Test the fixed specificity ordering with source priority
        let mut map = PolicyMap::new();

        // Insert rules with same total specificity but different distributions
        // Rule A: Specific source, wildcard destination (score: 2, 0)
        map.insert(
            AssetPair::from_str("bitcoin:btc -> *:*").unwrap(),
            "source_specific",
        );

        // Rule B: Wildcard source, specific destination (score: 0, 2)
        map.insert(
            AssetPair::from_str("*:* -> ethereum:eth").unwrap(),
            "destination_specific",
        );

        // Query for a specific pair
        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();

        // With the fix, source-specific should take precedence
        // because source specificity is compared first
        let result = map.get(&btc, &eth);
        assert_eq!(
            result,
            Some(&"source_specific"),
            "Source-specific rule should take precedence over destination-specific"
        );
    }

    #[test]
    fn test_specificity_with_fees() {
        // Real-world scenario: Fee structure with predictable precedence
        let mut fee_map = PolicyMap::new();

        // Default fee for all Bitcoin transactions
        fee_map.insert(AssetPair::from_str("bitcoin:btc -> *:*").unwrap(), 100);

        // Special fee for buying Ethereum
        fee_map.insert(AssetPair::from_str("*:* -> ethereum:eth").unwrap(), 200);

        // Most specific override
        fee_map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap(),
            150,
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();
        let usdc = AssetId::from_str("ethereum:usdc").unwrap();
        let sol = AssetId::from_str("solana:sol").unwrap();

        // Most specific wins
        assert_eq!(fee_map.get(&btc, &eth), Some(&150));

        // Bitcoin to other assets uses bitcoin rule (source-specific)
        assert_eq!(fee_map.get(&btc, &usdc), Some(&100));

        // Other assets to Ethereum uses ethereum rule (dest-specific)
        assert_eq!(fee_map.get(&sol, &eth), Some(&200));
    }

    #[test]
    fn test_source_priority_over_destination() {
        // Verify that source specificity has higher priority than destination
        let mut map = PolicyMap::new();

        // Both have total specificity of 3
        // Rule A: Fully specific source, partial destination (2, 1)
        map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:*").unwrap(),
            "specific_source",
        );

        // Rule B: Partial source, fully specific destination (1, 2)
        map.insert(
            AssetPair::from_str("bitcoin:* -> ethereum:eth").unwrap(),
            "specific_dest",
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();

        // Rule A should win because source specificity (2) > (1)
        assert_eq!(
            map.get(&btc, &eth),
            Some(&"specific_source"),
            "More specific source should take precedence even with less specific destination"
        );
    }

    #[test]
    fn test_bidirectional_reverse_with_wildcards() {
        // Test bidirectional matching in reverse direction with wildcards
        let mut map = PolicyMap::new();

        // Store bidirectional pair with wildcard source
        map.insert(
            AssetPair::from_str("bitcoin:* <-> ethereum:eth").unwrap(),
            "btc_any_to_eth",
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let wbtc = AssetId::from_str("bitcoin:wbtc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();

        // Forward direction: bitcoin:btc -> ethereum:eth
        assert_eq!(
            map.get(&btc, &eth),
            Some(&"btc_any_to_eth"),
            "Forward should match with wildcard"
        );

        // Reverse direction: ethereum:eth -> bitcoin:btc
        assert_eq!(
            map.get(&eth, &btc),
            Some(&"btc_any_to_eth"),
            "Reverse should match bidirectional pair with wildcard source"
        );

        // Reverse with different token: ethereum:eth -> bitcoin:wbtc
        assert_eq!(
            map.get(&eth, &wbtc),
            Some(&"btc_any_to_eth"),
            "Reverse should match with wildcard token"
        );
    }

    #[test]
    fn test_bidirectional_reverse_with_wildcard_destination() {
        // Test bidirectional with wildcard destination
        let mut map = PolicyMap::new();

        // Store bidirectional pair with wildcard destination
        map.insert(
            AssetPair::from_str("bitcoin:btc <-> *:usdc").unwrap(),
            "btc_to_any_usdc",
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let usdc_eth = AssetId::from_str("ethereum:usdc").unwrap();
        let usdc_sol = AssetId::from_str("solana:usdc").unwrap();

        // Forward: bitcoin:btc -> ethereum:usdc
        assert_eq!(
            map.get(&btc, &usdc_eth),
            Some(&"btc_to_any_usdc"),
            "Forward should match with wildcard destination"
        );

        // Reverse: ethereum:usdc -> bitcoin:btc
        assert_eq!(
            map.get(&usdc_eth, &btc),
            Some(&"btc_to_any_usdc"),
            "Reverse should match bidirectional pair with wildcard destination"
        );

        // Reverse with different chain: solana:usdc -> bitcoin:btc
        assert_eq!(
            map.get(&usdc_sol, &btc),
            Some(&"btc_to_any_usdc"),
            "Reverse should match with wildcard chain in destination"
        );
    }

    #[test]
    fn test_bidirectional_both_wildcards() {
        // Test bidirectional with wildcards on both sides
        let mut map = PolicyMap::new();

        map.insert(
            AssetPair::from_str("*:btc <-> *:eth").unwrap(),
            "btc_eth_any_chain",
        );

        let btc_bitcoin = AssetId::from_str("bitcoin:btc").unwrap();
        let btc_solana = AssetId::from_str("solana:btc").unwrap();
        let eth_ethereum = AssetId::from_str("ethereum:eth").unwrap();
        let eth_arbitrum = AssetId::from_str("arbitrum:eth").unwrap();

        // Forward: bitcoin:btc -> ethereum:eth
        assert_eq!(
            map.get(&btc_bitcoin, &eth_ethereum),
            Some(&"btc_eth_any_chain")
        );

        // Reverse: ethereum:eth -> bitcoin:btc
        assert_eq!(
            map.get(&eth_ethereum, &btc_bitcoin),
            Some(&"btc_eth_any_chain")
        );

        // Forward with different chains: solana:btc -> arbitrum:eth
        assert_eq!(
            map.get(&btc_solana, &eth_arbitrum),
            Some(&"btc_eth_any_chain")
        );

        // Reverse with different chains: arbitrum:eth -> solana:btc
        assert_eq!(
            map.get(&eth_arbitrum, &btc_solana),
            Some(&"btc_eth_any_chain")
        );
    }

    #[test]
    fn test_bidirectional_vs_forward_specificity() {
        // Test that forward rules don't match in reverse
        let mut map = PolicyMap::new();

        // Forward only rule
        map.insert(
            AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap(),
            "forward_only",
        );

        // Bidirectional rule (less specific)
        map.insert(
            AssetPair::from_str("bitcoin:* <-> ethereum:*").unwrap(),
            "bidirectional",
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let eth = AssetId::from_str("ethereum:eth").unwrap();

        // Forward query: should match forward_only (more specific)
        assert_eq!(map.get(&btc, &eth), Some(&"forward_only"));

        // Reverse query: should match bidirectional (forward_only doesn't work in reverse)
        assert_eq!(map.get(&eth, &btc), Some(&"bidirectional"));
    }

    #[test]
    fn test_bidirectional_no_match_reverse() {
        // Verify that non-matching pairs don't match in reverse either
        let mut map = PolicyMap::new();

        map.insert(
            AssetPair::from_str("bitcoin:btc <-> ethereum:eth").unwrap(),
            "btc_eth",
        );

        let btc = AssetId::from_str("bitcoin:btc").unwrap();
        let sol = AssetId::from_str("solana:sol").unwrap();

        // Neither forward nor reverse should match
        assert_eq!(map.get(&btc, &sol), None);
        assert_eq!(map.get(&sol, &btc), None);
    }
}
