mod api;
mod htlc;
mod initiator;
mod orders;
mod redeemer;
mod refunder;
mod settings;

use std::sync::Arc;

use alloy::{
    network::EthereumWallet,
    providers::{Provider, ProviderBuilder},
    signers::local::PrivateKeySigner,
};
use axum;
use orderbook::OrderbookProvider;
use reqwest::Url;
use tars::{fiat::FiatProvider, orderbook::OrderMapper};
use tracing::info;
use tracing_subscriber::EnvFilter;

use api::ApiState;
use initiator::InitiatorService;
use orders::PendingOrdersProvider;
use redeemer::RedeemerService;
use refunder::RefunderService;
use settings::Settings;

const CONFIG_FILE: &str = "config.toml";
const API_BIND: &str = "0.0.0.0:7777";

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let config_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| CONFIG_FILE.to_string());

    let mut settings = Settings::load(&config_path)?;

    let private_key = std::env::var("PRIVATE_KEY")
        .map_err(|_| eyre::eyre!("PRIVATE_KEY env var not set"))?;

    let signer: PrivateKeySigner = private_key
        .parse()
        .map_err(|e| eyre::eyre!("Invalid PRIVATE_KEY: {e}"))?;

    let executor_address = signer.address();
    info!(%executor_address, chain = %settings.initia.chain_name, "Starting Initia executor");

    let wallet = EthereumWallet::from(signer);
    let provider = Arc::new(
        ProviderBuilder::new()
            .wallet(wallet)
            .connect_http(settings.initia.rpc_url.parse()?),
    );

    let onchain_chain_id = provider.get_chain_id().await
        .map_err(|e| eyre::eyre!("Failed to fetch eth_chainId from {}: {e}", settings.initia.rpc_url))?;
    if onchain_chain_id != settings.initia.chain_id {
        tracing::warn!(
            configured = settings.initia.chain_id,
            onchain = onchain_chain_id,
            "Configured chain_id differs from on-chain eth_chainId; using on-chain value for HTLC orderID computation"
        );
        settings.initia.chain_id = onchain_chain_id;
    }

    let orderbook = Arc::new(
        OrderbookProvider::from_db_url(&settings.database.db_url)
            .await
            .map_err(|e| eyre::eyre!("Failed to connect to orderbook DB: {e}"))?,
    );

    let solver_orders_url = Url::parse(&settings.solver_orders_url)
        .map_err(|e| eyre::eyre!("Invalid solver_orders_url: {e}"))?;
    let orders_provider = PendingOrdersProvider::new(solver_orders_url);

    let fiat_provider = FiatProvider::new(&settings.fiat_provider_url, None)
        .map_err(|e| eyre::eyre!("Failed to create FiatProvider: {e}"))?;

    let order_mapper = Arc::new(
        OrderMapper::builder(fiat_provider)
            .add_supported_chain(settings.initia.chain_name.clone())
            .build(),
    );

    let initiator = InitiatorService::new(
        provider.clone(),
        orderbook.clone(),
        orders_provider.clone(),
        order_mapper.clone(),
        &settings,
        executor_address,
    )?;

    let redeemer = RedeemerService::new(
        provider.clone(),
        orderbook.clone(),
        orders_provider.clone(),
        order_mapper.clone(),
        &settings,
        executor_address,
    )?;

    let refunder = RefunderService::new(
        provider.clone(),
        orderbook.clone(),
        orders_provider,
        order_mapper.clone(),
        &settings,
        executor_address,
    )?;

    let api_state = ApiState::new(
        provider.clone(),
        orderbook.clone(),
        &settings,
    )?;
    let api_router = api_state.router();
    let listener = tokio::net::TcpListener::bind(API_BIND).await?;
    info!(addr = API_BIND, "API server listening");

    tokio::select! {
        _ = tokio::spawn(async move { initiator.run().await }) => {},
        _ = tokio::spawn(async move { redeemer.run().await }) => {},
        _ = tokio::spawn(async move { refunder.run().await }) => {},
        _ = axum::serve(listener, api_router) => {},
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
    }

    Ok(())
}
