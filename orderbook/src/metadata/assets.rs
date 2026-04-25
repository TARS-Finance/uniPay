use crate::metadata::chains::{RawAsset, RawChain};
use std::{collections::HashMap, fs};
use tars::primitives::{Asset, AssetId, Chain, ChainType, ContractInfo, HTLCVersion};

/// Asset plus the extra metadata needed by pricing and quote selection.
#[derive(Debug, Clone)]
pub struct AssetMetadata {
    pub asset: Asset,
    pub name: String,
    pub coingecko_id: Option<String>,
    pub aggregate_symbol: Option<String>,
    pub cmc_id: Option<String>,
}

/// Precomputed metadata indexes used throughout the service.
#[derive(Debug, Clone)]
pub struct MetadataIndex {
    pub chains: Vec<Chain>,
    pub raw_chains: Vec<RawChain>,
    pub assets: Vec<AssetMetadata>,
    pub assets_by_id: HashMap<String, AssetMetadata>,
    pub assets_by_htlc: HashMap<(String, String), AssetMetadata>,
}

impl MetadataIndex {
    /// Loads `chain.json` and builds lookups by asset ID and HTLC address.
    pub fn load(path: &str) -> eyre::Result<Self> {
        let contents = fs::read_to_string(path)?;
        let raw_chains: Vec<RawChain> = serde_json::from_str(&contents)?;
        let stored_raw = raw_chains.clone();

        let mut chains = Vec::with_capacity(raw_chains.len());
        let mut assets = Vec::new();
        let mut assets_by_id = HashMap::new();
        let mut assets_by_htlc = HashMap::new();

        for raw_chain in raw_chains {
            let mut chain_assets = Vec::with_capacity(raw_chain.assets.len());
            for raw_asset in raw_chain.assets.clone() {
                // Store each asset once and index it by both its canonical ID and HTLC identity.
                let metadata = Self::convert_asset(&raw_asset)?;
                let htlc_key = Self::normalize_htlc_key(&metadata.asset);
                chain_assets.push(metadata.asset.clone());
                assets_by_id.insert(
                    metadata.asset.id.to_string().to_lowercase(),
                    metadata.clone(),
                );
                assets_by_htlc.insert(htlc_key, metadata.clone());
                assets.push(metadata);
            }

            chains.push(Chain {
                chain: raw_chain.chain,
                id: raw_chain.id,
                icon: raw_chain.icon,
                explorer_url: raw_chain.explorer_url,
                confirmation_target: raw_chain.confirmation_target,
                source_timelock: raw_chain.source_timelock,
                destination_timelock: raw_chain.destination_timelock,
                supported_htlc_schemas: raw_chain.supported_htlc_schemas,
                supported_token_schemas: raw_chain.supported_token_schemas,
                assets: chain_assets,
            });
        }

        Ok(Self {
            chains,
            raw_chains: stored_raw,
            assets,
            assets_by_id,
            assets_by_htlc,
        })
    }

    /// Returns an asset using its canonical `chain:symbol`-style identifier.
    pub fn get_asset_by_id(&self, id: &str) -> Option<&AssetMetadata> {
        self.assets_by_id.get(&id.to_lowercase())
    }

    /// Returns an asset using the chain name and HTLC address key used by strategies.
    pub fn get_asset_by_chain_and_htlc(&self, chain: &str, htlc: &str) -> Option<&AssetMetadata> {
        self.assets_by_htlc
            .get(&(chain.to_string(), htlc.to_lowercase()))
    }

    /// Normalizes the lookup key used to match strategy assets back to metadata.
    pub fn normalize_htlc_key(asset: &Asset) -> (String, String) {
        let address = if asset.chain.contains("solana") && asset.token.address != "primary" {
            format!("{}_{}", asset.htlc.address, asset.token.address)
        } else {
            asset.htlc.address.clone()
        };

        (asset.chain.clone(), address.to_lowercase())
    }

    /// Converts the raw JSON schema into the `tars-rs` asset model used at runtime.
    fn convert_asset(raw: &RawAsset) -> eyre::Result<AssetMetadata> {
        let chain_type = match raw.chain_type.to_lowercase().as_str() {
            "bitcoin" => ChainType::Bitcoin,
            "solana" => ChainType::Solana,
            "starknet" => ChainType::Starknet,
            "sui" => ChainType::Sui,
            _ => ChainType::Evm,
        };

        let asset = Asset {
            id: raw.id.parse::<AssetId>().map_err(eyre::Report::msg)?,
            chain: raw.chain.clone(),
            icon: raw.icon.clone(),
            htlc: raw.htlc.clone().unwrap_or_else(primary_contract),
            token: raw.token.clone().unwrap_or_else(primary_contract),
            decimals: raw.decimals,
            min_amount: raw.min_amount.clone(),
            max_amount: raw.max_amount.clone(),
            chain_id: raw.chain_id.clone(),
            chain_icon: raw.chain_icon.clone(),
            chain_type,
            explorer_url: raw.explorer_url.clone(),
            price: None,
            version: raw.version.clone().unwrap_or(HTLCVersion::V1),
            min_timelock: raw.min_timelock,
            token_id: raw
                .token_ids
                .as_ref()
                .and_then(|ids| ids.coingecko.clone().or_else(|| ids.aggregate.clone()))
                .unwrap_or_else(|| raw.id.to_string()),
            solver: raw.solver.clone(),
        };

        Ok(AssetMetadata {
            asset,
            name: raw.name.clone(),
            coingecko_id: raw.token_ids.as_ref().and_then(|ids| ids.coingecko.clone()),
            aggregate_symbol: raw.token_ids.as_ref().and_then(|ids| ids.aggregate.clone()),
            cmc_id: raw.token_ids.as_ref().and_then(|ids| ids.cmc.clone()),
        })
    }
}

/// Creates a synthetic "primary" contract entry for native assets.
fn primary_contract() -> ContractInfo {
    ContractInfo {
        address: "primary".to_string(),
        schema: Some("primary".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::chains::{RawAsset, RawChain, RawTokenIds};

    #[test]
    fn loads_chain_metadata_and_normalizes_lookup_keys() {
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

        std::fs::write(&path, serde_json::to_string(&chains).unwrap()).unwrap();
        let metadata = MetadataIndex::load(path.to_str().unwrap()).unwrap();

        assert!(metadata.get_asset_by_id("BITCOIN_TESTNET:BTC").is_some());
        assert!(
            metadata
                .get_asset_by_chain_and_htlc("bitcoin_testnet", "primary")
                .is_some()
        );
    }
}
