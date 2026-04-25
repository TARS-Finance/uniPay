use crate::{AppState, server::handlers};
use axum::{
    Router,
    routing::{get, post},
};
use std::sync::Arc;

/// Registers the unified quote, order, and metadata endpoints.
pub fn router(_state: Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(handlers::health))
        .route("/fiat", get(handlers::fiat))
        .route("/quote", get(handlers::quote))
        .route(
            "/orders",
            post(handlers::create_order).get(handlers::list_orders),
        )
        .route("/orders/:id", get(handlers::get_order))
        .route("/liquidity", get(handlers::liquidity))
        .route("/strategies", get(handlers::strategies))
        .route("/chains", get(handlers::chains))
        .route("/pairs", get(handlers::pairs))
}
