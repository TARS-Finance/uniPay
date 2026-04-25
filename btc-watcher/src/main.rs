mod adapters;
mod core;
mod settings;
use crate::{
    adapters::{AddressScreenerClient, BitcoinRPCClient, FixedStatusScreener},
    core::{AddressScreener, RPCClient},
};
use adapters::{GardenBitcoinIndexer, GardenSwapStore, SwapCache, ZmqListener};
use bitcoin::{Block, Network, Transaction};
use core::{BlockchainIndexer, Cache, Swap, SwapStore, watch};
use tars::{
    bitcoin::{BITCOIN_REGTEST, BITCOIN_TESTNET},
    primitives::BITCOIN,
    utils::{OtelTracingConfig, TracingParams, setup_tracing as garden_setup_tracing},
};
use settings::Settings;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::Level;
use vault::ConfigType;

const CONFIG_FILE: &str = "Settings.toml";
const MAX_TXS_CHANNEL_BUFFER: usize = 1024;
const MAX_BLOCKS_CHANNEL_BUFFER: usize = 256;

/// Setup tracing for logs and webhook
fn setup_tracing(otel_settings: &Option<OtelTracingConfig>) -> eyre::Result<()> {
    let params = TracingParams {
        service_name: String::from("Bitcoin Watcher"),
        level: Level::INFO,
        otel_config: otel_settings.clone(),
        discord_webhook_url: None,
    };

    garden_setup_tracing(&params).map_err(|e| eyre::eyre!("Failed to setup tracing: {e}"))?;
    Ok(())
}

fn get_bitcoin_network(chain: &str) -> Network {
    match chain {
        BITCOIN_REGTEST => Network::Regtest,
        BITCOIN_TESTNET => Network::Testnet4,
        BITCOIN => Network::Bitcoin,
        _ => panic!("Unknown chain: {}", chain),
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> eyre::Result<()> {
    let settings: Settings = vault::get_config(CONFIG_FILE, ConfigType::Toml)
        .await
        .map_err(|e| eyre::eyre!("Failed to load settings: {e}"))?;

    setup_tracing(&settings.otel_opts)?;

    // Initialize components
    let swap_store: Arc<dyn SwapStore + Send + Sync> = Arc::new(
        GardenSwapStore::from_db_url(&settings.db_url, settings.deadline_buffer_secs).await?,
    );

    let swap_cache: Arc<dyn Cache<String, Swap> + Send + Sync> = Arc::new(SwapCache::new());
    tracing::info!(
        chain = %settings.chain.chain,
        "Starting Bitcoin Watcher"
    );

    let screener: Arc<dyn AddressScreener + Send + Sync> = match settings.screener_url {
        Some(url) => Arc::new(AddressScreenerClient::new(url)),
        None => Arc::new(FixedStatusScreener::new(false)),
    };

    let rpc_client: Arc<dyn RPCClient + Send + Sync> = Arc::new(BitcoinRPCClient::new(
        settings.rpc.url,
        settings.rpc.username,
        settings.rpc.password,
    ));

    let network = get_bitcoin_network(&settings.chain.chain);

    // Create channels for ZMQ listener -> processors
    let (tx_sender, tx_receiver) = mpsc::channel::<Transaction>(MAX_TXS_CHANNEL_BUFFER);
    let (block_sender, block_receiver) = mpsc::channel::<Block>(MAX_BLOCKS_CHANNEL_BUFFER);

    // Create and spawn ZMQ listener
    let zmq_listener = ZmqListener::new(settings.zmq, tx_sender, block_sender);
    tokio::spawn(async move {
        zmq_listener.listen().await;
    });

    // Create blockchain indexer
    let indexer: Arc<dyn BlockchainIndexer + Send + Sync> =
        Arc::new(GardenBitcoinIndexer::new(settings.indexer_url.clone())?);

    // Start the watcher (runs indefinitely)
    watch(
        tx_receiver,
        block_receiver,
        network,
        settings.chain,
        swap_store,
        swap_cache,
        indexer,
        screener,
        rpc_client,
        settings.indexer_url,
    );

    // Keep main thread alive
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down...");

    Ok(())
}
