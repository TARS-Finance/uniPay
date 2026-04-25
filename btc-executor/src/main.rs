use std::sync::Arc;
use std::time::Duration;

use btc_executor::executor::Executor;
use btc_executor::infrastructure::chain::bitcoin::BitcoinActionExecutor;
use btc_executor::infrastructure::chain::bitcoin::clients::{BitcoindRpcClient, ElectrsClient};
use btc_executor::infrastructure::chain::bitcoin::fee_providers::{
    ElectrsFeeRateEstimator, FeeRateEstimator,
};
use btc_executor::infrastructure::chain::bitcoin::wallet::{
    BitcoinWalletRunner, WalletConfig, WalletRequestSubmitter,
};
use btc_executor::infrastructure::keys::BitcoinWallet;
use btc_executor::infrastructure::persistence::{
    PgBitcoinWalletStore, connect_pool, database_schema,
};
use btc_executor::orders::PendingOrdersProvider;
use btc_executor::settings::{BitcoinSettings, Settings};
use moka::future::Cache;
use tars::fiat::FiatProvider;
use tars::orderbook::OrderMapper;
use tracing_subscriber::EnvFilter;

const SETTINGS_FILE: &str = "Settings.toml";
const CACHE_TTL_SECS: u64 = 3600;
const MAX_CACHE_SIZE: u64 = 1000;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("btc-executor failed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> eyre::Result<()> {
    init_tracing();

    let settings = Settings::try_from_toml(SETTINGS_FILE)?;
    let network = parse_network(&settings.bitcoin.network)?;
    let wallet = Arc::new(BitcoinWallet::from_private_key(
        &settings.executor_btc_private_key()?,
        network,
    )?);
    let electrs = Arc::new(ElectrsClient::new(settings.bitcoin.electrs_url.clone()));
    let bitcoind = Arc::new(BitcoindRpcClient::new(
        settings.bitcoin.bitcoind_url.clone(),
        settings.bitcoin.bitcoind_user.clone(),
        settings.bitcoin.bitcoind_pass.clone(),
    ));
    let fee_estimator: Arc<dyn FeeRateEstimator> =
        Arc::new(ElectrsFeeRateEstimator::new(Arc::clone(&electrs)));
    let schema = database_schema(&settings.bitcoin.chain_identifier);
    let pool = connect_pool(&settings.bitcoin.database_url, &schema, 5).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let store = Arc::new(PgBitcoinWalletStore::new(pool));
    let (wallet_runner, wallet_handle) = BitcoinWalletRunner::new(
        Arc::clone(&wallet),
        store,
        Arc::clone(&electrs),
        bitcoind,
        fee_estimator,
        network,
        wallet_config(&settings.bitcoin),
        None,
    )
    .await?;
    tokio::spawn(async move {
        wallet_runner.run().await;
    });

    let submitter: Arc<dyn WalletRequestSubmitter> = wallet_handle;
    let action_executor = Arc::new(BitcoinActionExecutor::new(
        wallet,
        submitter,
        Arc::clone(&electrs),
        network,
    ));
    let fiat_provider = FiatProvider::new(settings.fiat_provider_url.trim_end_matches('/'), None)?;
    let order_mapper = OrderMapper::builder(fiat_provider)
        .add_supported_chain(settings.bitcoin.chain_identifier.clone())
        .build();
    let orders_provider =
        PendingOrdersProvider::new(reqwest::Url::parse(&settings.pending_orders_url)?);
    let cache = Arc::new(
        Cache::builder()
            .time_to_live(Duration::from_secs(CACHE_TTL_SECS))
            .max_capacity(MAX_CACHE_SIZE)
            .build(),
    );

    let mut executor = Executor::new(
        settings.polling_interval_ms,
        settings.bitcoin.chain_identifier.clone(),
        orders_provider,
        order_mapper,
        action_executor,
        cache,
    );
    executor.run().await;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

fn parse_network(network: &str) -> eyre::Result<bitcoin::Network> {
    match network {
        "bitcoin" | "mainnet" => Ok(bitcoin::Network::Bitcoin),
        "testnet" | "bitcoin_testnet" => Ok(bitcoin::Network::Testnet),
        "signet" | "bitcoin_signet" => Ok(bitcoin::Network::Signet),
        "regtest" | "bitcoin_regtest" => Ok(bitcoin::Network::Regtest),
        other => Err(eyre::eyre!("unsupported bitcoin network {other}")),
    }
}

fn wallet_config(settings: &BitcoinSettings) -> WalletConfig {
    WalletConfig {
        tick_interval_secs: settings.batcher_interval_secs,
        min_fee_rate: settings.default_fee_rate,
        max_fee_rate: settings
            .default_fee_rate
            .max(WalletConfig::default().max_fee_rate),
        ..WalletConfig::default()
    }
}
