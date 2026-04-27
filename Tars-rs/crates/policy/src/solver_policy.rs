use crate::{
    collections::{PolicyMap, PolicySet},
    common::is_asset_id_match,
    primitives::{Fee, SourceAmount},
    DefaultPolicy, PolicyError, SolverPolicyConfig,
};
use bigdecimal::BigDecimal;
use eyre::Result;
use primitives::{Asset, AssetId, AssetPair, PairDirection};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    str::FromStr,
};

/// Unified overrides structure containing all optional route-specific overrides.
///
/// All fields are optional, allowing different routes to override only the specific
/// values they need. When searching for a value, the system will search through
/// all matching routes in precedence order until it finds one where the field is `Some`.
#[derive(Debug, Clone)]
pub struct Overrides {
    /// Fee override for the route
    pub fee: Option<Fee>,
    /// Max slippage override for the route
    pub max_slippage: Option<u64>,
    /// Confirmation target override for the route
    pub confirmation_target: Option<u64>,
    /// Source amount override for the route
    pub source_amount: Option<SourceAmount>,
}

/// Isolation rules storage.
#[derive(Debug, Clone)]
pub struct IsolationRules {
    /// Source -> allowed destinations (sorted most specific first)
    pub source_to_destination: Vec<(AssetId, Vec<AssetId>)>,
    /// Destination -> allowed sources (sorted most specific first)
    pub destination_to_source: Vec<(AssetId, Vec<AssetId>)>,
}

/// Policy manager for a single solver that enforces trading rules and fee structures.
///
/// `SolverPolicy` provides comprehensive policy enforcement for asset trading, including:
/// - Asset support validation
/// - Isolation rules that restrict which assets can trade together
/// - Blacklist/whitelist pair management
/// - Fee calculation with route-specific overrides
///
/// # Policy Validation Order
///
/// When validating a trade, the policy checks are performed in this order:
/// 1. **Asset Support**: Both source and destination must be in the supported assets list
/// 2. **Isolation Rules**: If isolation rules exist for the source, destination must be allowed
/// 3. **Blacklist/Whitelist**: Pair must not be blacklisted, unless whitelisted
///
/// # Wildcards and Specificity
///
/// Rules support wildcards (`*`) for chain or token fields. When multiple rules match,
/// more specific rules (exact matches) take precedence over wildcards.
///
#[derive(Debug, Clone)]
pub struct SolverPolicy {
    /// The default policy type (e.g., "open", "closed")
    #[allow(unused)]
    default: DefaultPolicy,
    /// Isolation rules grouped by direction.
    isolation_rules: IsolationRules,
    /// Pairs that are explicitly blocked from trading.
    /// Uses PolicySet for efficient membership testing.
    blacklist_pairs: PolicySet,
    /// Pairs that override blacklist restrictions.
    /// These pairs are allowed even if they would be blocked by blacklist rules.
    whitelist_overrides: PolicySet,
    /// Default fee structure for this solver
    default_fee: Fee,
    /// Default max slippage
    default_max_slippage: u64,
    /// Default confirmation target
    default_confirmation_target: u64,
    /// Unified route-specific overrides for fees, slippage, confirmation targets, and source amounts
    overrides: PolicyMap<Overrides>,
    /// Supported assets for this solver
    supported_assets: HashSet<AssetId>,
    /// Maximum source liquidity limit for assets
    max_limits: HashMap<AssetId, BigDecimal>,
}
impl SolverPolicy {
    /// Creates a new `SolverPolicy` from a configuration.
    ///
    /// This constructor parses and validates the policy configuration, converting
    /// string-based asset pairs and routes into efficient internal data structures.
    ///
    /// # Arguments
    ///
    /// * `policy` - The solver policy configuration containing rules and fee structures
    /// * `supported_assets` - The assets this solver supports
    ///
    /// # Returns
    ///
    /// * `Ok(SolverPolicy)` - Successfully created policy
    /// * `Err(PolicyError)` - Configuration is invalid (malformed asset IDs or pairs)
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Asset IDs in `supported_assets` cannot be parsed
    /// - Asset pairs in isolation_groups, blacklist_pairs, or whitelist_overrides are invalid
    /// - Route specifications in fee overrides are malformed
    pub fn new(
        policy: SolverPolicyConfig,
        supported_assets: Vec<String>,
    ) -> Result<Self, PolicyError> {
        let isolation_groups = policy.isolation_groups;
        let blacklist_pairs = policy.blacklist_pairs;
        let whitelist_overrides = policy.whitelist_overrides;
        let default_fee = policy.default_fee;
        let route_overrides = policy.overrides;

        let supported_assets = supported_assets
            .clone()
            .iter()
            .map(|asset| {
                AssetId::from_str(asset).map_err(|e| PolicyError::InvalidAssetId(asset.clone(), e))
            })
            .collect::<Result<HashSet<AssetId>, PolicyError>>()?;

        // Build isolation rules in a temporary HashMap first, pre-allocating capacity
        let mut temp_isolation_rules: HashMap<AssetId, Vec<AssetId>> =
            HashMap::with_capacity(isolation_groups.len());
        let mut temp_incoming_isolation_rules: HashMap<AssetId, Vec<AssetId>> =
            HashMap::with_capacity(isolation_groups.len());
        for group in isolation_groups.clone() {
            let (source, direction, destination) = AssetPair::from_str(&group)
                .map_err(|e| PolicyError::InvalidAssetPair(group.clone(), e))?
                .into();

            let bidirectional = matches!(direction, PairDirection::Both);

            // Add forward direction rule
            temp_isolation_rules
                .entry(source.clone())
                .or_insert(Vec::new())
                .push(destination.clone());
            temp_incoming_isolation_rules
                .entry(destination.clone())
                .or_insert(Vec::new())
                .push(source.clone());

            // Add reverse direction rule if bidirectional
            if bidirectional {
                temp_isolation_rules
                    .entry(destination.clone())
                    .or_insert(Vec::new())
                    .push(source.clone());
                temp_incoming_isolation_rules
                    .entry(source)
                    .or_insert(Vec::new())
                    .push(destination);
            }
        }

        // Convert HashMap to Vec and sort by specificity (most specific first)
        // This ensures the most specific rules are checked first during validation
        let mut source_to_destination: Vec<(AssetId, Vec<AssetId>)> =
            Vec::with_capacity(temp_isolation_rules.len());
        source_to_destination.extend(temp_isolation_rules.into_iter());
        source_to_destination.sort_by(|(a, _), (b, _)| compare_asset_specificity(a, b));

        let mut destination_to_source: Vec<(AssetId, Vec<AssetId>)> =
            Vec::with_capacity(temp_incoming_isolation_rules.len());
        destination_to_source.extend(temp_incoming_isolation_rules.into_iter());
        destination_to_source.sort_by(|(a, _), (b, _)| compare_asset_specificity(a, b));

        let mut blacklist_pairs_set = PolicySet::with_capacity(blacklist_pairs.len());
        for pair in blacklist_pairs.clone() {
            let asset_pair = AssetPair::from_str(&pair)
                .map_err(|e| PolicyError::InvalidAssetPair(pair.clone(), e))?;
            blacklist_pairs_set.insert(asset_pair);
        }

        let mut whitelist_overrides_set = PolicySet::with_capacity(whitelist_overrides.len());
        for pair in whitelist_overrides.clone() {
            let asset_pair = AssetPair::from_str(&pair)
                .map_err(|e| PolicyError::InvalidAssetPair(pair.clone(), e))?;
            whitelist_overrides_set.insert(asset_pair);
        }

        // Build unified overrides map from the route overrides
        // Convert each RouteOverride to Overrides and insert into PolicyMap
        // PolicyMap will handle any duplicates (though each route should only appear once)
        let mut unified_overrides = PolicyMap::with_capacity(route_overrides.len());
        for route_override in route_overrides {
            let asset_pair = AssetPair::from_str(&route_override.route)
                .map_err(|e| PolicyError::InvalidAssetPair(route_override.route.clone(), e))?;

            let overrides = Overrides {
                fee: route_override.fee,
                max_slippage: route_override.max_slippage,
                confirmation_target: route_override.confirmation_target,
                source_amount: route_override.source_amount,
            };

            unified_overrides.insert(asset_pair, overrides);
        }

        Ok(Self {
            default: policy.default,
            isolation_rules: IsolationRules {
                source_to_destination,
                destination_to_source,
            },
            blacklist_pairs: blacklist_pairs_set,
            whitelist_overrides: whitelist_overrides_set,
            default_fee,
            default_max_slippage: policy.default_max_slippage,
            default_confirmation_target: policy.default_confirmation_target,
            overrides: unified_overrides,
            max_limits: policy.max_limits,
            supported_assets,
        })
    }

    /// Retrieves the fee for a specific trading route.
    ///
    /// Returns the fee override if one exists for the route, otherwise returns
    /// the default fee. This method does not perform validation - use
    /// `validate_and_get_fee` if you need combined validation and fee retrieval.
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// The applicable fee for the route (either override or default)
    pub fn get_fee(&self, source: &AssetId, destination: &AssetId) -> Fee {
        self.overrides
            .find_map(source, destination, |overrides| overrides.fee.clone())
            .unwrap_or(self.default_fee.clone())
    }

    /// Returns a reference to the default fee structure.
    ///
    /// This is the base fee applied to all routes that don't have
    /// specific fee overrides configured.
    pub fn default_fee(&self) -> &Fee {
        &self.default_fee
    }

    /// Returns a reference to the set of supported assets for this solver.
    ///
    /// Only assets in this set can participate in trades handled by this solver.
    /// Both the source and destination assets must be in the supported set for
    /// a trade to be valid.
    pub fn supported_assets(&self) -> &HashSet<AssetId> {
        &self.supported_assets
    }

    /// Checks if an asset is supported by this solver.
    ///
    /// # Arguments
    ///
    /// * `asset` - The asset identifier to check
    ///
    /// # Returns
    ///
    /// `true` if the asset is supported, `false` otherwise
    pub fn is_asset_supported(&self, asset: &AssetId) -> bool {
        self.supported_assets.contains(asset)
    }

    /// Validates an asset pair and returns the applicable fee if the trade is allowed.
    ///
    /// This method combines validation and fee retrieval into a single operation,
    /// providing both policy compliance checking and fee information for valid trades.
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// * `Ok(Fee)` - The trade is allowed and the applicable fee is returned
    /// * `Err(...)` - The trade is blocked, with details about which rule was violated
    pub fn validate_and_get_fee(&self, source: &AssetId, destination: &AssetId) -> Result<Fee> {
        self.validate_asset_pair(source, destination)?;
        Ok(self.get_fee(source, destination))
    }

    /// Validates whether a trade between two assets is allowed according to policy rules.
    ///
    /// This method performs comprehensive validation by checking:
    /// 1. Asset support - ensures both source and destination assets are supported by the solver
    /// 2. Isolation rules - ensures the source asset can trade with the destination
    /// 3. Blacklist restrictions - ensures the pair is not explicitly blocked
    /// 4. Whitelist overrides - allows pairs that would otherwise be blacklisted
    ///
    /// The validation follows a precedence system where more specific rules
    /// override less specific ones, and whitelist overrides take precedence
    /// over blacklist restrictions.
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// * `Ok(())` - The trade is allowed according to policy rules
    /// * `Err(...)` - The trade is blocked, with details about which rule was violated
    pub fn validate_asset_pair(&self, source: &AssetId, destination: &AssetId) -> Result<()> {
        if source == destination {
            return Err(eyre::eyre!(
                "Source and destination assets must be different (got both: {})",
                source
            ));
        }
        self.are_assets_supported(source, destination)?;
        self.is_not_isolated(source, destination)?;
        self.is_not_blacklisted(source, destination)?;
        Ok(())
    }

    /// Get the max slippage for a specific trading route.
    ///
    /// Returns the max slippage override if one exists for the route, otherwise returns
    /// the default max slippage.
    pub fn get_max_slippage(&self, source: &AssetId, destination: &AssetId) -> u64 {
        self.overrides
            .find_map(source, destination, |overrides| overrides.max_slippage)
            .unwrap_or(self.default_max_slippage)
    }

    /// Gets the default max slippage.
    pub fn default_max_slippage(&self) -> u64 {
        self.default_max_slippage
    }

    /// Gets the confirmation target for a specific trading route.
    ///
    /// Returns the confirmation target override if one exists for the route, otherwise returns
    /// the default confirmation target.
    pub fn get_confirmation_target(&self, source: &AssetId, destination: &AssetId) -> u64 {
        self.overrides
            .find_map(source, destination, |overrides| {
                overrides.confirmation_target
            })
            .unwrap_or(self.default_confirmation_target)
    }

    /// Gets the default confirmation target.
    pub fn default_confirmation_target(&self) -> u64 {
        self.default_confirmation_target
    }

    /// Gets the source amount for a specific trading route.
    ///
    /// Returns the source amount override if one exists for the route, otherwise returns
    /// the default source amount.
    pub fn get_source_amount(&self, source: &Asset, destination: &Asset) -> Result<SourceAmount> {
        self.overrides
            .find_map(&source.id, &destination.id, |overrides| {
                overrides.source_amount.clone()
            })
            .map(Ok)
            .unwrap_or_else(|| self.default_source_amount(source))
    }

    /// Gets the default source amount for an asset.
    pub fn default_source_amount(&self, asset: &Asset) -> Result<SourceAmount> {
        let min_amount = BigDecimal::from_str(&asset.min_amount)
            .map_err(|e| eyre::eyre!("Invalid min amount: {}", e))?;
        let max_amount = BigDecimal::from_str(&asset.max_amount)
            .map_err(|e| eyre::eyre!("Invalid max amount: {}", e))?;
        Ok(SourceAmount {
            min: min_amount,
            max: max_amount,
        })
    }

    /// Gets the maximum source liquidity limit for an asset.
    ///
    /// Returns the maximum source liquidity limit override if one exists for the asset, otherwise returns
    /// the default maximum source liquidity limit.
    pub fn get_max_source_liquidity_limit(&self, asset: &AssetId) -> Option<BigDecimal> {
        self.max_limits.get(asset).cloned()
    }

    /// Checks if both source and destination assets are supported by this solver.
    ///
    /// This validation ensures that the solver can handle both assets involved in the trade.
    /// If either asset is not supported, the trade should be rejected.
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Both assets are supported by the solver
    /// * `Err(...)` - One or both assets are not supported, with details about which assets
    fn are_assets_supported(&self, source: &AssetId, destination: &AssetId) -> Result<()> {
        let mut unsupported_assets = Vec::new();

        if !self.is_asset_supported(source) {
            unsupported_assets.push(source.to_string());
        }

        if !self.is_asset_supported(destination) {
            unsupported_assets.push(destination.to_string());
        }

        if !unsupported_assets.is_empty() {
            return Err(eyre::eyre!(
                "Unsupported assets: [{}]. Supported assets: [{}]",
                unsupported_assets.join(", "),
                self.supported_assets
                    .iter()
                    .map(|asset| asset.to_string())
                    .collect::<Vec<String>>()
                    .join(", ")
            ));
        }

        Ok(())
    }

    /// Checks if a trade violates isolation rules.
    ///
    /// Isolation rules restrict which assets can trade with each other. When an asset
    /// has isolation rules defined, it can only trade with assets specified in those rules.
    ///
    /// The method implements a specificity-based precedence system:
    /// - More specific rules (exact matches) override less specific ones (wildcards)
    /// - Rules are matched using wildcard support for both chain and token fields
    /// - The first matching rule (most specific) determines the allowed destinations
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// * `Ok(())` - No isolation rules apply or the trade is allowed by isolation rules
    /// * `Err(...)` - The trade violates isolation rules, with details about allowed destinations
    fn is_not_isolated(&self, source: &AssetId, destination: &AssetId) -> Result<()> {
        // The first matching rule determines the allowed destinations
        // Outbound isolation: source can only trade to allowed destinations
        for (source_asset, allowed_destinations) in &self.isolation_rules.source_to_destination {
            if is_asset_id_match(source, source_asset) {
                for rule in allowed_destinations {
                    if is_asset_id_match(rule, destination) {
                        return Ok(()); // Trade is allowed
                    }
                }

                // Found matching isolation rule, but destination is not allowed
                return Err(eyre::eyre!(
                    "Source {} is isolated and can only trade with: [{}], but trying to trade to {}",
                    source,
                    allowed_destinations
                        .iter()
                        .map(|rule| rule.to_string())
                        .collect::<Vec<String>>()
                        .join(", "),
                    destination
                ));
            }
        }

        // Inbound isolation: destination can only receive from allowed sources
        for (destination_asset, allowed_sources) in &self.isolation_rules.destination_to_source {
            if is_asset_id_match(destination, destination_asset) {
                for rule in allowed_sources {
                    if is_asset_id_match(rule, source) {
                        return Ok(()); // Trade is allowed
                    }
                }

                return Err(eyre::eyre!(
                    "Destination {} is isolated and can only receive from: [{}], but trying to receive from {}",
                    destination,
                    allowed_sources
                        .iter()
                        .map(|rule| rule.to_string())
                        .collect::<Vec<String>>()
                        .join(", "),
                    source
                ));
            }
        }

        // No isolation rules apply in either direction
        Ok(())
    }

    /// Checks if a trade violates blacklist restrictions.
    ///
    /// Blacklist rules explicitly block specific asset pairs from trading. However,
    /// whitelist overrides can allow pairs that would otherwise be blacklisted.
    ///
    /// The validation logic:
    /// 1. Check if the pair is in the blacklist
    /// 2. If blacklisted, check if there's a whitelist override
    /// 3. Only block the trade if it's blacklisted AND not whitelisted
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// * `Ok(())` - The pair is not blacklisted or is whitelisted
    /// * `Err(...)` - The pair is blacklisted and not whitelisted
    fn is_not_blacklisted(&self, source: &AssetId, destination: &AssetId) -> Result<()> {
        if self.blacklist_pairs.contains(source, destination)
            && !self.whitelist_overrides.contains(source, destination)
        {
            return Err(eyre::eyre!(
                "Pair {} to {} is blacklisted",
                source,
                destination
            ));
        }
        Ok(())
    }
}

/// Compare two AssetIds for specificity ordering
/// Returns Ordering::Less if `a` is more specific than `b`
fn compare_asset_specificity(a: &AssetId, b: &AssetId) -> std::cmp::Ordering {
    let a_chain_specific = a.chain() != "*";
    let a_token_specific = a.token() != "*";
    let b_chain_specific = b.chain() != "*";
    let b_token_specific = b.token() != "*";

    // Count specificity (2 = both specific, 1 = one specific, 0 = both wildcards)
    let a_specificity = (a_chain_specific as u8) + (a_token_specific as u8);
    let b_specificity = (b_chain_specific as u8) + (b_token_specific as u8);

    match a_specificity.cmp(&b_specificity) {
        Ordering::Equal => {
            // If same level of specificity, compare lexicographically for consistency
            (a.chain(), a.token()).cmp(&(b.chain(), b.token()))
        }
        other => other.reverse(), // More specific should come first (Less)
    }
}
