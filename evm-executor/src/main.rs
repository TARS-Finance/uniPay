use alloy::{
    hex::FromHex,
    network::EthereumWallet,
    primitives::{Address, FixedBytes},
    providers::ProviderBuilder,
    signers::local::PrivateKeySigner,
};
use executor::Executor;
use eyre::{Result, eyre};
use moka::future::Cache;
use orders::PendingOrdersProvider;
use reqwest::Url;
use settings::ChainSettings;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tars::{
    evm::{
        Multicall3::Multicall3Instance, executor::UnipayActionExecutor, htlc::UnipayHTLC,
        primitives::UnipayHandlerType, traits::UnipayActionHandler, tx_handler::PendingTxHandler,
    },
    fiat::FiatProvider,
    orderbook::OrderMapper,
    utils::setup_tracing_with_webhook,
};

mod executor;
mod orders;
mod settings;

const SETTINGS_FILE: &str = "Settings.toml";
const CACHE_TTL: u64 = 3600; // Cache time to live in seconds
const MAX_CACHE_SIZE: u64 = 1000; // Maximum cache size

#[tokio::main]
async fn main() {
    // Load application settings
    let settings = match settings::Settings::try_from_toml(SETTINGS_FILE) {
        Ok(settings) => settings,
        Err(e) => {
            eprintln!("Failed to load settings: {}", e);
            std::process::exit(1);
        }
    };

    // Initialize tracing/logging
    setup_logging(&settings.discord_webhook_url);

    // Initialize core services
    let (fiat_provider, orders_provider, cache) = match setup_core_services(&settings).await {
        Ok(services) => services,
        Err(e) => {
            tracing::error!("Failed to setup core services: {}", e);
            return;
        }
    };

    // Create wallet and signer
    let (wallet, signer) = match create_wallet_and_signer(&settings.private_key) {
        Ok((wallet, signer)) => (wallet, signer),
        Err(e) => {
            tracing::error!("Failed to create wallet and signer: {}", e);
            return;
        }
    };

    // Create executors for each chain
    let executors = create_executors(
        &settings.chains,
        fiat_provider,
        orders_provider,
        cache,
        wallet,
        signer,
    )
    .await;

    if executors.is_empty() {
        tracing::error!("No executors created successfully. Exiting.");
        return;
    }

    tracing::info!("Starting {} executor(s)", executors.len());

    // Start all executors
    run_executors(executors).await;
    tracing::info!("All executors exited");
}

/// Sets up logging with optional Discord webhook integration
fn setup_logging(discord_webhook_url: &Option<String>) {
    match discord_webhook_url {
        Some(webhook_url) => {
            if let Err(e) =
                setup_tracing_with_webhook(webhook_url, "EVM Executor", tracing::Level::ERROR, None)
            {
                eprintln!("Failed to setup tracing with webhook: {}", e);
                tracing_subscriber::fmt().pretty().init();
            }
        }
        None => tracing_subscriber::fmt().pretty().init(),
    }
}

/// Sets up core services: order mapper, orders provider, and cache
async fn setup_core_services(
    settings: &settings::Settings,
) -> Result<(
    FiatProvider,
    PendingOrdersProvider,
    Arc<Cache<String, bool>>,
)> {
    // Parse pending orders URL
    let orders_provider_url = Url::parse(&settings.pending_orders_url)
        .map_err(|e| eyre!("Failed to parse pending orders URL: {}", e))?;

    let orders_provider = PendingOrdersProvider::new(orders_provider_url);

    let fiat_url = settings.fiat_provider_url.trim_end_matches("/");

    // Create fiat provider
    let fiat_provider = FiatProvider::new(&fiat_url, None)
        .map_err(|e| eyre!("Failed to create fiat provider: {}", e))?;

    // Create cache
    let cache = Arc::new(
        Cache::builder()
            .time_to_live(Duration::from_secs(CACHE_TTL))
            .max_capacity(MAX_CACHE_SIZE)
            .build(),
    );

    Ok((fiat_provider, orders_provider, cache))
}

/// Creates wallet and signer from private key
fn create_wallet_and_signer(private_key: &str) -> Result<(EthereumWallet, PrivateKeySigner)> {
    let private_key_bytes = FixedBytes::from_hex(private_key)
        .map_err(|e| eyre!("Failed to parse private key hex: {}", e))?;

    let signer = PrivateKeySigner::from_bytes(&private_key_bytes)
        .map_err(|e| eyre!("Failed to create signer: {}", e))?;

    let wallet = EthereumWallet::from(signer.clone());

    Ok((wallet, signer))
}

/// Creates executors for all configured chains
async fn create_executors(
    chain_configs: &[ChainSettings],
    fiat_provider: FiatProvider,
    orders_provider: PendingOrdersProvider,
    cache: Arc<Cache<String, bool>>,
    wallet: EthereumWallet,
    signer: PrivateKeySigner,
) -> Vec<Executor> {
    let mut executors = Vec::with_capacity(chain_configs.len());

    for chain_config in chain_configs {
        let order_mapper = OrderMapper::builder(fiat_provider.clone())
            .add_supported_chain(chain_config.chain_identifier.clone())
            .build();
        match create_executor_for_chain(
            chain_config.clone(),
            order_mapper,
            orders_provider.clone(),
            Arc::clone(&cache),
            wallet.clone(),
            signer.clone(),
        )
        .await
        {
            Ok(executor) => {
                tracing::info!(
                    "Created executor for chain: {}",
                    chain_config.chain_identifier
                );
                executors.push(executor);
            }
            Err(e) => {
                tracing::error!(
                    "Failed to create executor for chain {}: {}",
                    chain_config.chain_identifier,
                    e
                );
            }
        }
    }

    executors
}

/// Runs all executors concurrently
async fn run_executors(executors: Vec<Executor>) {
    let executor_handles: Vec<_> = executors
        .into_iter()
        .map(|mut executor| {
            tokio::spawn(async move {
                executor.run().await;
            })
        })
        .collect();

    // Wait for all executors to complete
    for handle in executor_handles {
        if let Err(e) = handle.await {
            tracing::error!("Executor task failed: {}", e);
        }
    }
}

/// Creates an `Executor` instance from the provided chain configuration
async fn create_executor_for_chain(
    config: ChainSettings,
    order_mapper: OrderMapper,
    order_provider: PendingOrdersProvider,
    cache: Arc<Cache<String, bool>>,
    wallet: EthereumWallet,
    signer: PrivateKeySigner,
) -> Result<Executor> {
    // Parse RPC URL and create provider
    let rpc_url = Url::parse(&config.rpc_url).map_err(|e| {
        eyre!(
            "Failed to parse RPC URL for chain {}: {}",
            config.chain_identifier,
            e
        )
    })?;

    let provider = ProviderBuilder::new()
        .disable_recommended_fillers()
        .with_gas_estimation()
        .with_simple_nonce_management()
        .fetch_chain_id()
        .wallet(wallet)
        .connect_http(rpc_url);

    // Parse multicall address and create contract instance
    let multicall_address = Address::from_hex(&config.multicall_address).map_err(|e| {
        eyre!(
            "Failed to parse multicall address for {}: {}",
            config.chain_identifier,
            e
        )
    })?;

    let multicall_contract = Arc::new(Multicall3Instance::new(multicall_address, provider.clone()));

    // Create Unipay HTLC instance
    let garden_htlc = UnipayHTLC::new(signer.clone(), provider.clone());

    let action_handlers: HashMap<UnipayHandlerType, Arc<dyn UnipayActionHandler>> =
        HashMap::from([(
            UnipayHandlerType::HTLC,
            Arc::new(garden_htlc.clone()) as Arc<dyn UnipayActionHandler>,
        )]);

    let action_executor = UnipayActionExecutor::new(multicall_contract, action_handlers);

    let pending_tx_handler = PendingTxHandler::new(
        Duration::from_millis(config.transaction_timeout),
        provider.clone(),
        config.chain_identifier.clone(),
        action_executor.clone(),
    );

    Ok(Executor::builder()
        .polling_interval(config.polling_interval)
        .orders_provider(order_provider)
        .order_mapper(order_mapper)
        .actions_executor(action_executor)
        .provider(provider)
        .pending_tx_handler(pending_tx_handler)
        .signer_addr(signer.address().to_string().to_lowercase())
        .settings(config)
        .cache(cache)
        .build())
}
