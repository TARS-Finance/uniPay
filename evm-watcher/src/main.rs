use crate::{
    onchain_orders::GardenOnChainOrderProvider,
    stream_event::garden_htlc,
    swaps::GardenSwapStore,
    watcher::{ChainConfig, ContractAddresses, HTLCEventWatcher, MultiChainSwapsProvider},
};
use alloy::{primitives::Address, providers::ProviderBuilder};
use tars::orderbook::primitives::SingleSwap;
use moka::future::Cache;
use std::{str::FromStr, sync::Arc, time::Duration};
use tokio::{task::JoinSet, time::interval};

mod block_range;
mod onchain_orders;
mod prepare_event;
mod primitives;
mod settings;
mod stream_event;
mod swaps;
mod testutils;
mod validate_event;
mod validate_swap;
mod watcher;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt::init();

    let settings = settings::Settings::from_toml("Settings.toml");
    let cache: Cache<String, Vec<SingleSwap>> = Cache::builder().build();

    let supported_assets = settings
        .chains
        .iter()
        .flat_map(|chain| chain.supported_assets.values())
        .flatten()
        .cloned()
        .collect();

    let pending_swaps_cache = Arc::new(cache);
    let swap_store = Arc::new(
        GardenSwapStore::from_db_url(
            &settings.db_url,
            settings.ignore_chains,
            settings.deadline_buffer,
            Some(supported_assets),
        )
        .await
        .map_err(|e| eyre::eyre!("Failed to create swap store: {}", e))?,
    );

    let multiwatcher = MultiChainSwapsProvider::new(
        pending_swaps_cache.clone(),
        swap_store.clone(),
        settings.multiwatcher_polling_interval,
    );

    let mut watchers = Vec::new();
    for chain in settings.chains {
        let blockchain_provider = Arc::new(
            ProviderBuilder::new()
                .connect(&chain.rpc_url)
                .await
                .expect(&format!(
                    "failed to create blockchain provider for {}",
                    chain.name
                )),
        );

        let v2_assets = chain
            .supported_assets
            .get("v2")
            .unwrap_or(&vec![])
            .iter()
            .map(|s| Address::from_str(s).expect("Invalid address format"))
            .collect();
        let v3_assets = chain
            .supported_assets
            .get("v3")
            .unwrap_or(&vec![])
            .iter()
            .map(|s| Address::from_str(s).expect("Invalid address format"))
            .collect();

        let contract_addresses = ContractAddresses::new(v2_assets, v3_assets);
        let htlc_event_provider = garden_htlc::EventProvider::new(
            blockchain_provider.clone(),
            Some(chain.max_block_span),
            None,
        );

        let chain_config = ChainConfig::new(
            chain.name,
            interval(Duration::from_secs(chain.polling_interval)),
            chain.max_block_span,
            contract_addresses,
        );

        let onchain_order_provider = Arc::new(
            GardenOnChainOrderProvider::new(&chain.multicall_address, blockchain_provider.clone())
                .await
                .expect("Failed to create onchain order provider"),
        );

        let watcher = HTLCEventWatcher::new(
            multiwatcher.clone(),
            swap_store.clone(),
            onchain_order_provider.clone(),
            Arc::new(htlc_event_provider),
            blockchain_provider,
            chain_config,
        );
        watchers.push(watcher);
    }

    let mut join_set = JoinSet::new();
    for mut watcher in watchers {
        join_set.spawn(async move {
            let _ = watcher.start().await;
        });
    }

    while let Some(_) = join_set.join_next().await {}
    Ok(())
}
