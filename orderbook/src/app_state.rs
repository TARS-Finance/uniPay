use crate::{
    config::settings::Settings, liquidity::watcher::LiquidityWatcher, metadata::MetadataIndex,
    orders::service::OrderService, pricing::service::PricingService, quote::service::QuoteService,
    read_api::service::ReadApiService, registry::StrategyRegistry,
};
use std::sync::Arc;
use tars::orderbook::OrderbookProvider;

/// Holds the long-lived services shared by all HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub settings: Settings,
    pub metadata: Arc<MetadataIndex>,
    pub registry: Arc<StrategyRegistry>,
    pub pricing: Arc<PricingService>,
    pub liquidity: Arc<LiquidityWatcher>,
    pub quote_service: Arc<QuoteService>,
    pub order_service: Arc<OrderService>,
    pub read_api: Arc<ReadApiService>,
    pub orderbook: Arc<OrderbookProvider>,
}

impl AppState {
    /// Builds the shared state object once startup wiring is complete.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        settings: Settings,
        metadata: Arc<MetadataIndex>,
        registry: Arc<StrategyRegistry>,
        pricing: Arc<PricingService>,
        liquidity: Arc<LiquidityWatcher>,
        quote_service: Arc<QuoteService>,
        order_service: Arc<OrderService>,
        read_api: Arc<ReadApiService>,
        orderbook: Arc<OrderbookProvider>,
    ) -> Self {
        Self {
            settings,
            metadata,
            registry,
            pricing,
            liquidity,
            quote_service,
            order_service,
            read_api,
            orderbook,
        }
    }
}
