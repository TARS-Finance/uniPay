use std::sync::Arc;
use tars::orderbook::{OrderbookProvider, primitives::OrderQueryFilters, traits::Orderbook};

/// Thin wrapper around the Unipay-compatible orderbook read queries.
#[derive(Clone)]
pub struct ReadApiService {
    orderbook: Arc<OrderbookProvider>,
}

impl ReadApiService {
    /// Creates the read API facade from the shared orderbook provider.
    pub fn new(orderbook: Arc<OrderbookProvider>) -> Self {
        Self { orderbook }
    }

    /// Returns one fully populated matched order, if it exists.
    pub async fn get_order(
        &self,
        id: &str,
    ) -> Result<
        Option<tars::orderbook::primitives::MatchedOrderVerbose>,
        tars::orderbook::errors::OrderbookError,
    > {
        self.orderbook.get_matched_order(id).await
    }

    /// Lists matched orders using the existing paginated filter model.
    pub async fn list_orders(
        &self,
        filters: OrderQueryFilters,
    ) -> Result<
        tars::orderbook::primitives::PaginatedData<
            tars::orderbook::primitives::MatchedOrderVerbose,
        >,
        tars::orderbook::errors::OrderbookError,
    > {
        self.orderbook.get_all_matched_orders(filters).await
    }
}
