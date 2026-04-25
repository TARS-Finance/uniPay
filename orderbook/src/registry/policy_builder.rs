use crate::{
    config::{policy::PolicySettings, strategy::{StrategyAssetConfig, StrategyConfig}},
    metadata::{AssetMetadata, MetadataIndex},
};
use bigdecimal::BigDecimal;
use policy::SolverPolicy;
use std::str::FromStr;

/// Builds concrete strategy config records from local policy and metadata.
pub fn build_strategy_configs(
    policy_settings: &PolicySettings,
    metadata: &MetadataIndex,
) -> eyre::Result<Vec<StrategyConfig>> {
    let supported_assets = policy_settings
        .chains
        .values()
        .flat_map(|chain| chain.supported_assets.iter().cloned())
        .collect::<Vec<_>>();
    let policy = SolverPolicy::new(policy_settings.policy.clone(), supported_assets)
        .map_err(eyre::Report::msg)?;

    let chain_assets = policy_settings
        .chains
        .iter()
        .map(|(chain_name, chain_config)| {
            let assets = chain_config
                .supported_assets
                .iter()
                .filter_map(|asset_id| metadata.get_asset_by_id(asset_id).cloned())
                .collect::<Vec<_>>();
            (chain_name.as_str(), chain_config, assets)
        })
        .filter(|(_, _, assets)| !assets.is_empty())
        .collect::<Vec<_>>();

    let mut strategies = Vec::new();
    for (source_chain_name, source_chain_config, source_assets) in &chain_assets {
        for (dest_chain_name, dest_chain_config, dest_assets) in &chain_assets {
            if source_chain_name.eq_ignore_ascii_case(dest_chain_name) {
                continue;
            }

            for source_asset in source_assets {
                for dest_asset in dest_assets {
                    if policy
                        .validate_asset_pair(&source_asset.asset.id, &dest_asset.asset.id)
                        .is_err()
                    {
                        continue;
                    }

                    let source_amount = policy
                        .get_source_amount(&source_asset.asset, &dest_asset.asset)
                        .map_err(eyre::Report::msg)?;
                    let fee = policy.get_fee(&source_asset.asset.id, &dest_asset.asset.id);

                    strategies.push(StrategyConfig {
                        id: format!(
                            "{}:{}->{}",
                            policy_settings.solver_id,
                            source_asset.asset.id,
                            dest_asset.asset.id
                        )
                        .to_lowercase(),
                        source_chain_address: source_chain_config.order_identity().to_string(),
                        dest_chain_address: dest_chain_config.order_identity().to_string(),
                        source_chain: (*source_chain_name).to_string(),
                        dest_chain: (*dest_chain_name).to_string(),
                        source_asset: strategy_asset(source_asset),
                        dest_asset: strategy_asset(dest_asset),
                        makers: vec![policy_settings.solver_id.clone()],
                        min_amount: source_amount.min,
                        max_amount: source_amount.max,
                        min_source_timelock: source_asset.asset.min_timelock,
                        destination_timelock: dest_asset.asset.min_timelock,
                        min_source_confirmations: policy
                            .get_confirmation_target(&source_asset.asset.id, &dest_asset.asset.id),
                        fee: fee.percent_bips as u64,
                        fixed_fee: BigDecimal::from_str(&fee.fixed.to_string())?,
                        max_slippage: policy
                            .get_max_slippage(&source_asset.asset.id, &dest_asset.asset.id),
                    });
                }
            }
        }
    }

    Ok(strategies)
}

fn strategy_asset(asset: &AssetMetadata) -> StrategyAssetConfig {
    StrategyAssetConfig {
        asset: MetadataIndex::normalize_htlc_key(&asset.asset).1,
        htlc_address: asset.asset.htlc.address.clone(),
        token_address: asset.asset.token.address.clone(),
        token_id: asset.asset.token_id.clone(),
        display_symbol: asset.aggregate_symbol.clone(),
        decimals: asset.asset.decimals,
        version: asset.asset.version.clone(),
    }
}
