/// HTTP request handlers.
pub mod handlers;
/// Shared JSON response wrappers.
pub mod response;
/// Axum route registration.
pub mod routes;
/// Request and response types for server endpoints.
pub mod types;

use crate::AppState;
use eyre::Result;
use std::{net::SocketAddr, sync::Arc};
use tokio::net::TcpListener;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

/// Starts the Axum server and binds the configured HTTP routes.
pub async fn serve(state: Arc<AppState>) -> Result<()> {
    let router = routes::router(state.clone())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state.clone());

    let address: SocketAddr = state.settings.addr.parse()?;
    let listener = TcpListener::bind(address).await?;
    tracing::info!("listening on {}", address);
    axum::serve(listener, router).await?;
    Ok(())
}
