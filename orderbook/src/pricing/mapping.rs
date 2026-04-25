use crate::{config::settings::PricingSettings, metadata::MetadataIndex};
use std::{collections::HashMap, sync::Arc};

/// Resolves local Tars assets to canonical pricing assets used by VWMP snapshots.
#[derive(Clone)]
pub struct PricingMapping {
    by_asset_id: HashMap<String, String>,
}

impl PricingMapping {
    /// Builds the mapping from config, with metadata-based fallbacks for local development.
    pub fn new(settings: &PricingSettings, metadata: Arc<MetadataIndex>) -> Self {
        let mut by_asset_id = HashMap::new();

        for asset in &metadata.assets {
            let key = asset.asset.id.to_string().to_lowercase();
            let canonical = settings
                .asset_canonicals
                .get(&key)
                .cloned()
                .or_else(|| {
                    settings
                        .asset_canonicals
                        .get(&asset.asset.id.to_string())
                        .cloned()
                })
                .or_else(|| {
                    asset
                        .aggregate_symbol
                        .as_ref()
                        .map(|value| value.to_uppercase())
                })
                .or_else(|| {
                    asset
                        .coingecko_id
                        .as_ref()
                        .map(|value| value.to_uppercase())
                })
                .unwrap_or_else(|| asset.asset.id.to_string().to_uppercase());
            by_asset_id.insert(key, canonical);
        }

        Self { by_asset_id }
    }

    /// Returns the canonical pricing asset for a local asset ID.
    pub fn canonical_for_asset_id(&self, asset_id: &str) -> Option<&str> {
        self.by_asset_id
            .get(&asset_id.to_lowercase())
            .map(String::as_str)
    }

    /// Returns the canonical pricing asset or falls back to the normalized asset ID.
    pub fn canonical_or_asset(&self, asset_id: &str) -> String {
        self.canonical_for_asset_id(asset_id)
            .map(str::to_string)
            .unwrap_or_else(|| asset_id.to_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::settings::{MarketDataSettings, PricingSettings},
        metadata::chains::{RawAsset, RawChain, RawTokenIds},
    };
    use std::fs;
    use tars::primitives::{ContractInfo, HTLCVersion};

    fn primary_contract() -> ContractInfo {
        ContractInfo {
            address: "primary".to_string(),
            schema: Some("primary".to_string()),
        }
    }

    #[test]
    fn resolves_configured_canonical_mapping() {
        let tempdir = tempfile::tempdir().unwrap();
        let path = tempdir.path().join("chain.json");
        let chains = vec![RawChain {
            chain: "bitcoin_testnet".to_string(),
            id: "bitcoin".to_string(),
            icon: "icon".to_string(),
            explorer_url: "explorer".to_string(),
            confirmation_target: 1,
            source_timelock: "12".to_string(),
            destination_timelock: "6".to_string(),
            supported_htlc_schemas: vec!["primary".to_string()],
            supported_token_schemas: vec!["primary".to_string()],
            assets: vec![RawAsset {
                id: "bitcoin_testnet:btc".to_string(),
                name: "Bitcoin".to_string(),
                chain: "bitcoin_testnet".to_string(),
                icon: "icon".to_string(),
                htlc: Some(primary_contract()),
                token: Some(primary_contract()),
                decimals: 8,
                min_amount: "1".to_string(),
                max_amount: "100".to_string(),
                chain_icon: "icon".to_string(),
                chain_id: None,
                chain_type: "bitcoin".to_string(),
                version: Some(HTLCVersion::V2),
                explorer_url: "explorer".to_string(),
                min_timelock: 12,
                token_ids: Some(RawTokenIds {
                    coingecko: Some("bitcoin".to_string()),
                    aggregate: Some("BTC".to_string()),
                    cmc: Some("1".to_string()),
                }),
                solver: "solver".to_string(),
            }],
        }];

        fs::write(&path, serde_json::to_string(&chains).unwrap()).unwrap();
        let metadata = Arc::new(MetadataIndex::load(path.to_str().unwrap()).unwrap());
        let settings = PricingSettings {
            refresh_interval_secs: 30,
            coingecko_api_url: "http://localhost".to_string(),
            coingecko_api_key: None,
            static_prices: HashMap::new(),
            asset_canonicals: HashMap::from([(
                "bitcoin_testnet:btc".to_string(),
                "BTC".to_string(),
            )]),
            market_data: MarketDataSettings::default(),
        };

        let mapping = PricingMapping::new(&settings, metadata);
        assert_eq!(
            mapping.canonical_for_asset_id("bitcoin_testnet:btc"),
            Some("BTC")
        );
        assert_eq!(
            mapping.canonical_for_asset_id("BITCOIN_TESTNET:BTC"),
            Some("BTC")
        );
    }
}
