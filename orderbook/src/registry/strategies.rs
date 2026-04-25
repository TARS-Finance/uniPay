use crate::{
    config::strategy::{StrategyAssetConfig, StrategyConfig},
    metadata::MetadataIndex,
};
use bigdecimal::BigDecimal;
use serde::Serialize;
use std::{collections::HashMap, fs};
use tars::primitives::HTLCVersion;

/// Runtime copy of the asset fields needed during quoting and order creation.
#[derive(Debug, Clone, Serialize)]
pub struct StrategyAsset {
    pub asset: String,
    pub htlc_address: String,
    pub token_address: String,
    pub token_id: String,
    pub display_symbol: Option<String>,
    pub decimals: u8,
    pub version: HTLCVersion,
}

/// Quoteable route definition loaded from the strategy file.
#[derive(Debug, Clone, Serialize)]
pub struct Strategy {
    pub id: String,
    pub source_chain_address: String,
    pub dest_chain_address: String,
    pub source_chain: String,
    pub dest_chain: String,
    pub source_asset: StrategyAsset,
    pub dest_asset: StrategyAsset,
    pub makers: Vec<String>,
    pub min_amount: BigDecimal,
    pub max_amount: BigDecimal,
    pub min_source_timelock: u64,
    pub destination_timelock: u64,
    pub min_source_confirmations: u64,
    pub fee: u64,
    pub fixed_fee: BigDecimal,
    pub max_slippage: u64,
}

impl Strategy {
    /// Returns the normalized pair key used for route lookup.
    pub fn order_pair(&self) -> String {
        format!(
            "{}:{}::{}:{}",
            self.source_chain, self.source_asset.asset, self.dest_chain, self.dest_asset.asset
        )
        .to_lowercase()
    }

    /// Returns the source asset identifier in `chain:asset` form.
    pub fn source_symbol(&self) -> String {
        format!("{}:{}", self.source_chain, self.source_asset.asset).to_lowercase()
    }

    /// Returns the destination asset identifier in `chain:asset` form.
    pub fn destination_symbol(&self) -> String {
        format!("{}:{}", self.dest_chain, self.dest_asset.asset).to_lowercase()
    }
}

/// In-memory strategy registry keyed by both pair and strategy ID.
#[derive(Debug, Clone)]
pub struct StrategyRegistry {
    strategies_by_pair: HashMap<String, HashMap<String, Strategy>>,
    strategies_by_id: HashMap<String, Strategy>,
}

impl StrategyRegistry {
    /// Loads strategy config and builds the pair and ID indexes used by quotes.
    pub fn load(path: &str, metadata: &MetadataIndex) -> eyre::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let configs: Vec<StrategyConfig> = serde_json::from_str(&contents)?;
        Self::from_configs(configs, metadata)
    }

    /// Builds the registry from already-loaded strategy config records.
    pub fn from_configs(
        configs: Vec<StrategyConfig>,
        metadata: &MetadataIndex,
    ) -> eyre::Result<Self> {
        let mut strategies_by_pair = HashMap::<String, HashMap<String, Strategy>>::new();
        let mut strategies_by_id = HashMap::new();

        for config in configs {
            // Each strategy is stored twice so requests can resolve by pair or explicit ID.
            let strategy = Self::convert_strategy(config, metadata)?;
            let pair = strategy.order_pair();
            strategies_by_pair
                .entry(pair)
                .or_default()
                .insert(strategy.id.to_lowercase(), strategy.clone());
            strategies_by_id.insert(strategy.id.to_lowercase(), strategy);
        }

        Ok(Self {
            strategies_by_pair,
            strategies_by_id,
        })
    }

    /// Returns all strategies that can quote a given pair.
    pub fn strategies_for_pair(&self, pair: &str) -> Option<&HashMap<String, Strategy>> {
        self.strategies_by_pair.get(&pair.to_lowercase())
    }

    /// Returns a single strategy by its configured identifier.
    pub fn strategy(&self, id: &str) -> Option<&Strategy> {
        self.strategies_by_id.get(&id.to_lowercase())
    }

    /// Exposes the full strategy map for API responses.
    pub fn all_strategies(&self) -> &HashMap<String, Strategy> {
        &self.strategies_by_id
    }

    /// Iterates over the pair index used to build `/pairs`.
    pub fn pairs(&self) -> impl Iterator<Item = (&String, &HashMap<String, Strategy>)> {
        self.strategies_by_pair.iter()
    }

    /// Converts one strategy config record into the runtime strategy model.
    fn convert_strategy(
        config: StrategyConfig,
        metadata: &MetadataIndex,
    ) -> eyre::Result<Strategy> {
        let source_asset =
            Self::convert_asset(config.source_asset, &config.source_chain, metadata)?;
        let dest_asset = Self::convert_asset(config.dest_asset, &config.dest_chain, metadata)?;

        Ok(Strategy {
            id: config.id,
            source_chain_address: config.source_chain_address,
            dest_chain_address: config.dest_chain_address,
            source_chain: config.source_chain,
            dest_chain: config.dest_chain,
            source_asset,
            dest_asset,
            makers: config.makers,
            min_amount: config.min_amount,
            max_amount: config.max_amount,
            min_source_timelock: config.min_source_timelock,
            destination_timelock: config.destination_timelock,
            min_source_confirmations: config.min_source_confirmations,
            fee: config.fee,
            fixed_fee: config.fixed_fee,
            max_slippage: config.max_slippage,
        })
    }

    /// Normalizes one strategy asset while validating it against loaded metadata.
    fn convert_asset(
        config: StrategyAssetConfig,
        chain: &str,
        metadata: &MetadataIndex,
    ) -> eyre::Result<StrategyAsset> {
        let display_symbol = if config.display_symbol.is_none() {
            metadata
                .get_asset_by_chain_and_htlc(chain, &config.asset)
                .and_then(|m| m.aggregate_symbol.clone())
        } else {
            config.display_symbol
        };

        if display_symbol.is_none() && config.asset != "primary" {
            tracing::warn!(
                strategy_chain = chain,
                strategy_asset = %config.asset,
                "strategy asset was not found directly in metadata by HTLC address"
            );
        }

        Ok(StrategyAsset {
            asset: config.asset,
            htlc_address: config.htlc_address,
            token_address: config.token_address,
            token_id: config.token_id,
            display_symbol,
            decimals: config.decimals,
            version: config.version,
        })
    }
}
