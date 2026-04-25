use crate::{
    AppState,
    error::AppError,
    metadata::{AssetMetadata, MetadataIndex},
    orders::types::CreateOrderRequest,
    pricing::service::PricingService,
    quote::types::QuoteRequest,
    registry::pairs::derive_pairs,
    server::{
        response::{self, success},
        types::{UsdOrderPairParams, UsdOrderPairResponse},
    },
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
};
use std::fmt;
use std::sync::Arc;
use tars::orderbook::primitives::OrderQueryFilters;

/// Lightweight readiness endpoint used by deployments and local smoke tests.
pub async fn health() -> Json<crate::server::response::ApiResponse<serde_json::Value>> {
    success(serde_json::json!({ "status": "ok" }))
}

/// Returns route candidates for a requested swap.
pub async fn quote(
    State(state): State<Arc<AppState>>,
    Query(request): Query<QuoteRequest>,
) -> Result<Json<crate::server::response::ApiResponse<crate::quote::types::QuoteResponse>>, AppError>
{
    let response = state.quote_service.quote(request).await?;
    Ok(success(response))
}

/// Returns the USD prices for both sides of a quote-style `order_pair`.
pub async fn fiat(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UsdOrderPairParams>,
) -> Response {
    match fiat_prices_for_order_pair(&params.order_pair, state.metadata.as_ref(), state.pricing.as_ref())
        .await
    {
        Ok((input_token_price, output_token_price)) => {
            response::legacy_success(UsdOrderPairResponse {
                input_token_price,
                output_token_price,
            })
        }
        Err(FiatEndpointError::BadRequest(message)) => {
            response::legacy_error(StatusCode::BAD_REQUEST, message)
        }
        Err(FiatEndpointError::PriceUnavailable(message)) => response::legacy_error(
            StatusCode::BAD_REQUEST,
            format!("failed to get fiat values: {message}"),
        ),
    }
}

/// Prices and persists a matched order in one request.
pub async fn create_order(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateOrderRequest>,
) -> Result<
    Json<crate::server::response::ApiResponse<tars::orderbook::primitives::MatchedOrderVerbose>>,
    AppError,
> {
    let response = state.order_service.create_order(request).await?;
    Ok(success(response))
}

/// Fetches a single persisted order by create ID or swap ID.
pub async fn get_order(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<
    Json<crate::server::response::ApiResponse<tars::orderbook::primitives::MatchedOrderVerbose>>,
    AppError,
> {
    let order = state
        .read_api
        .get_order(&id)
        .await?
        .ok_or_else(|| AppError::not_found(format!("order not found: {id}")))?;
    Ok(success(order))
}

/// Lists orders from the shared orderbook using the existing Unipay filters.
pub async fn list_orders(
    State(state): State<Arc<AppState>>,
    Query(filters): Query<OrderQueryFilters>,
) -> Result<
    Json<
        crate::server::response::ApiResponse<
            tars::orderbook::primitives::PaginatedData<
                tars::orderbook::primitives::MatchedOrderVerbose,
            >,
        >,
    >,
    AppError,
> {
    let orders = state.read_api.list_orders(filters).await?;
    Ok(success(orders))
}

/// Exposes the current in-memory solver liquidity snapshot.
pub async fn liquidity(
    State(state): State<Arc<AppState>>,
) -> Result<
    Json<crate::server::response::ApiResponse<crate::liquidity::primitives::SolverLiquidity>>,
    AppError,
> {
    Ok(success(state.liquidity.all().await))
}

/// Returns the loaded strategy registry keyed by strategy ID.
pub async fn strategies(
    State(state): State<Arc<AppState>>,
) -> Result<
    Json<
        crate::server::response::ApiResponse<
            std::collections::HashMap<String, crate::registry::Strategy>,
        >,
    >,
    AppError,
> {
    Ok(success(state.registry.all_strategies().clone()))
}

/// Returns the raw chain and asset registry loaded from `chain.json`.
pub async fn chains(
    State(state): State<Arc<AppState>>,
) -> Json<crate::server::response::ApiResponse<Vec<crate::metadata::chains::RawChain>>> {
    success(state.metadata.raw_chains.clone())
}

/// Returns the supported order pairs derived from loaded strategies.
pub async fn pairs(
    State(state): State<Arc<AppState>>,
) -> Result<
    Json<crate::server::response::ApiResponse<Vec<crate::registry::pairs::PairDescriptor>>>,
    AppError,
> {
    Ok(success(derive_pairs(&state.registry)))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FiatEndpointError {
    BadRequest(String),
    PriceUnavailable(String),
}

impl fmt::Display for FiatEndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadRequest(message) | Self::PriceUnavailable(message) => {
                write!(f, "{message}")
            }
        }
    }
}

async fn fiat_prices_for_order_pair(
    order_pair: &str,
    metadata: &MetadataIndex,
    pricing: &PricingService,
) -> Result<(f64, f64), FiatEndpointError> {
    let (source_pair, destination_pair) = split_order_pair(order_pair)?;
    let input_asset = resolve_pair_asset(source_pair, "Source", metadata)?;
    let output_asset = resolve_pair_asset(destination_pair, "Destination", metadata)?;

    let input_token_price = pricing
        .price_for(&input_asset.asset.id.to_string())
        .await
        .ok_or_else(|| {
            FiatEndpointError::PriceUnavailable(format!(
                "missing price for {}",
                input_asset.asset.id
            ))
        })?;
    let output_token_price = pricing
        .price_for(&output_asset.asset.id.to_string())
        .await
        .ok_or_else(|| {
            FiatEndpointError::PriceUnavailable(format!(
                "missing price for {}",
                output_asset.asset.id
            ))
        })?;

    Ok((input_token_price, output_token_price))
}

fn split_order_pair(order_pair: &str) -> Result<(&str, &str), FiatEndpointError> {
    let mut parts = order_pair.split("::");
    let source_pair = parts.next().unwrap_or_default();
    let destination_pair = parts.next().unwrap_or_default();

    if source_pair.is_empty() || destination_pair.is_empty() || parts.next().is_some() {
        return Err(FiatEndpointError::BadRequest(
            "Invalid order pair format, expected source_chain:source_asset::dest_chain:dest_asset"
                .to_string(),
        ));
    }

    Ok((source_pair, destination_pair))
}

fn resolve_pair_asset<'a>(
    pair: &'a str,
    label: &str,
    metadata: &'a MetadataIndex,
) -> Result<&'a AssetMetadata, FiatEndpointError> {
    let mut parts = pair.splitn(2, ':');
    let chain = parts.next().unwrap_or_default();
    let asset = parts.next().unwrap_or_default();

    if chain.is_empty() || asset.is_empty() {
        return Err(FiatEndpointError::BadRequest(
            "Invalid order pair format, expected source_chain:source_asset::dest_chain:dest_asset"
                .to_string(),
        ));
    }

    metadata
        .get_asset_by_chain_and_htlc(&chain.to_lowercase(), asset)
        .ok_or_else(|| {
            FiatEndpointError::BadRequest(format!(
                "{label} asset pair : {pair}, cannot be found in supported asset pairs"
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::fiat_prices_for_order_pair;
    use crate::{
        config::settings::{MarketDataSettings, PricingSettings},
        metadata::{MetadataIndex, chains::{RawAsset, RawChain, RawTokenIds}},
        pricing::service::PricingService,
    };
    use rust_decimal::Decimal;
    use std::{collections::HashMap, fs, sync::Arc};
    use tars::primitives::{ContractInfo, HTLCVersion};

    fn contract(address: &str, schema: &str) -> ContractInfo {
        ContractInfo {
            address: address.to_string(),
            schema: Some(schema.to_string()),
        }
    }

    fn write_chain_json() -> (tempfile::TempDir, Arc<MetadataIndex>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chain.json");
        let chains = vec![
            RawChain {
                chain: "base_sepolia".to_string(),
                id: "evm:84532".to_string(),
                icon: "icon".to_string(),
                explorer_url: "explorer".to_string(),
                confirmation_target: 1,
                source_timelock: "3600".to_string(),
                destination_timelock: "3600".to_string(),
                supported_htlc_schemas: vec!["evm:htlc_erc20".to_string()],
                supported_token_schemas: vec!["evm:erc20".to_string()],
                assets: vec![RawAsset {
                    id: "base_sepolia:usdc".to_string(),
                    name: "USD Coin".to_string(),
                    chain: "base_sepolia".to_string(),
                    icon: "icon".to_string(),
                    htlc: Some(contract("0xbasehtlc", "evm:htlc_erc20")),
                    token: Some(contract("0xbasetoken", "evm:erc20")),
                    decimals: 6,
                    min_amount: "1".to_string(),
                    max_amount: "1000000".to_string(),
                    chain_icon: "icon".to_string(),
                    chain_id: Some("84532".to_string()),
                    chain_type: "evm".to_string(),
                    version: Some(HTLCVersion::V3),
                    explorer_url: "explorer".to_string(),
                    min_timelock: 3600,
                    token_ids: Some(RawTokenIds {
                        coingecko: Some("usd-coin".to_string()),
                        aggregate: Some("USDC".to_string()),
                        cmc: Some("3408".to_string()),
                    }),
                    solver: "solver".to_string(),
                }],
            },
            RawChain {
                chain: "bitcoin_testnet".to_string(),
                id: "bitcoin".to_string(),
                icon: "icon".to_string(),
                explorer_url: "explorer".to_string(),
                confirmation_target: 1,
                source_timelock: "12".to_string(),
                destination_timelock: "12".to_string(),
                supported_htlc_schemas: vec!["primary".to_string()],
                supported_token_schemas: vec!["primary".to_string()],
                assets: vec![RawAsset {
                    id: "bitcoin_testnet:btc".to_string(),
                    name: "Bitcoin".to_string(),
                    chain: "bitcoin_testnet".to_string(),
                    icon: "icon".to_string(),
                    htlc: Some(contract("primary", "primary")),
                    token: Some(contract("primary", "primary")),
                    decimals: 8,
                    min_amount: "1".to_string(),
                    max_amount: "1000000".to_string(),
                    chain_icon: "icon".to_string(),
                    chain_id: None,
                    chain_type: "bitcoin".to_string(),
                    version: Some(HTLCVersion::V3),
                    explorer_url: "explorer".to_string(),
                    min_timelock: 12,
                    token_ids: Some(RawTokenIds {
                        coingecko: Some("bitcoin".to_string()),
                        aggregate: Some("BTC".to_string()),
                        cmc: Some("1".to_string()),
                    }),
                    solver: "solver".to_string(),
                }],
            },
        ];

        fs::write(&path, serde_json::to_string(&chains).unwrap()).unwrap();
        let metadata = Arc::new(MetadataIndex::load(path.to_str().unwrap()).unwrap());
        (dir, metadata)
    }

    async fn pricing_service(metadata: Arc<MetadataIndex>) -> Arc<PricingService> {
        let pricing = Arc::new(PricingService::new(
            PricingSettings {
                refresh_interval_secs: 30,
                coingecko_api_url: "http://localhost".to_string(),
                coingecko_api_key: None,
                static_prices: HashMap::new(),
                asset_canonicals: HashMap::new(),
                market_data: MarketDataSettings::default(),
            },
            metadata,
        ));

        pricing
            .ingest_aggregator_price("test", "USDC", Decimal::ONE)
            .await;
        pricing
            .ingest_aggregator_price("test", "BTC", Decimal::from(100_000_u64))
            .await;
        pricing
    }

    #[tokio::test]
    async fn returns_input_and_output_prices_for_a_valid_order_pair() {
        let (_dir, metadata) = write_chain_json();
        let pricing = pricing_service(metadata.clone()).await;

        let prices = fiat_prices_for_order_pair(
            "base_sepolia:0xbasehtlc::bitcoin_testnet:primary",
            metadata.as_ref(),
            pricing.as_ref(),
        )
        .await
        .expect("expected fiat prices");

        assert_eq!(prices, (1.0, 100_000.0));
    }

    #[tokio::test]
    async fn rejects_order_pairs_with_an_invalid_format() {
        let (_dir, metadata) = write_chain_json();
        let pricing = pricing_service(metadata.clone()).await;

        let error = fiat_prices_for_order_pair("base_sepolia:0xbasehtlc", metadata.as_ref(), pricing.as_ref())
            .await
            .expect_err("expected invalid format error");

        assert_eq!(
            error.to_string(),
            "Invalid order pair format, expected source_chain:source_asset::dest_chain:dest_asset"
        );
    }

    #[tokio::test]
    async fn rejects_unknown_source_assets_with_the_reference_message() {
        let (_dir, metadata) = write_chain_json();
        let pricing = pricing_service(metadata.clone()).await;

        let error = fiat_prices_for_order_pair(
            "base_sepolia:0xmissing::bitcoin_testnet:primary",
            metadata.as_ref(),
            pricing.as_ref(),
        )
        .await
        .expect_err("expected missing source asset error");

        assert_eq!(
            error.to_string(),
            "Source asset pair : base_sepolia:0xmissing, cannot be found in supported asset pairs"
        );
    }
}
