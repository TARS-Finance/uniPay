use crate::{
    config::settings::QuoteSettings,
    error::AppError,
    liquidity::watcher::LiquidityWatcher,
    metadata::{AssetMetadata, MetadataIndex},
    pricing::service::PricingService,
    quote::{
        matcher,
        types::{QuoteRequest, QuoteResponse, QuoteRoute},
    },
    registry::{Strategy, StrategyRegistry},
};
use std::sync::Arc;

fn classify_empty_routes_error(liquidity_blocked: bool) -> Result<(), AppError> {
    if liquidity_blocked {
        Err(AppError::conflict(
            "insufficient destination liquidity for requested route",
        ))
    } else {
        Ok(())
    }
}

/// Orchestrates quote generation from metadata, pricing, strategies, and liquidity.
#[derive(Clone)]
pub struct QuoteService {
    settings: QuoteSettings,
    metadata: Arc<MetadataIndex>,
    registry: Arc<StrategyRegistry>,
    pricing: Arc<PricingService>,
    liquidity: Arc<LiquidityWatcher>,
}

impl QuoteService {
    /// Creates the quote service from the shared runtime dependencies.
    pub fn new(
        settings: QuoteSettings,
        metadata: Arc<MetadataIndex>,
        registry: Arc<StrategyRegistry>,
        pricing: Arc<PricingService>,
        liquidity: Arc<LiquidityWatcher>,
    ) -> Self {
        Self {
            settings,
            metadata,
            registry,
            pricing,
            liquidity,
        }
    }

    /// Computes all valid routes for a quote request and picks the best one.
    pub async fn quote(&self, request: QuoteRequest) -> Result<QuoteResponse, AppError> {
        if request.from_amount.is_some() == request.to_amount.is_some() {
            return Err(AppError::bad_request(
                "provide exactly one of from_amount or to_amount",
            ));
        }

        // Resolve both user-facing asset references into canonical metadata entries first.
        let input_asset = self.resolve_asset(&request.from)?;
        let output_asset = self.resolve_asset(&request.to)?;
        let order_pair = format!(
            "{}:{}::{}:{}",
            input_asset.asset.chain,
            MetadataIndex::normalize_htlc_key(&input_asset.asset).1,
            output_asset.asset.chain,
            MetadataIndex::normalize_htlc_key(&output_asset.asset).1,
        )
        .to_lowercase();

        let strategies = self
            .registry
            .strategies_for_pair(&order_pair)
            .ok_or_else(|| AppError::bad_request("pair is not supported"))?;

        // Pricing is fetched once per request and reused across all strategies for the pair.
        let input_price = self
            .pricing
            .price_for(&input_asset.asset.id.to_string())
            .await
            .ok_or_else(|| AppError::Upstream(format!("missing price for {}", request.from)))?;
        let output_price = self
            .pricing
            .price_for(&output_asset.asset.id.to_string())
            .await
            .ok_or_else(|| AppError::Upstream(format!("missing price for {}", request.to)))?;

        let slippage = request
            .slippage
            .unwrap_or_default()
            .min(self.settings.max_user_slippage_bps);

        let mut routes = Vec::new();
        let mut liquidity_blocked = false;
        for strategy in strategies.values() {
            // An explicit strategy ID narrows route selection to a single configured path.
            if let Some(strategy_id) = request.strategy_id.as_ref() {
                if strategy.id.to_lowercase() != strategy_id.to_lowercase() {
                    continue;
                }
            }

            // Exact-in and exact-out requests share the same route-building pipeline.
            let (source, destination) =
                match (request.from_amount.as_ref(), request.to_amount.as_ref()) {
                    (Some(from_amount), _) => matcher::calculate_output_amount(
                        strategy,
                        from_amount,
                        input_price,
                        output_price,
                        request.affiliate_fee,
                        slippage,
                    )?,
                    (None, Some(to_amount)) => matcher::calculate_input_amount(
                        strategy,
                        to_amount,
                        input_price,
                        output_price,
                        request.affiliate_fee,
                        slippage,
                    )?,
                    (None, None) => {
                        return Err(AppError::bad_request(
                            "either from_amount or to_amount is required",
                        ));
                    }
                };

            // Routes that cannot be paid out by the local solver are dropped before ranking.
            if !self
                .liquidity
                .can_fulfill(strategy, &destination.amount)
                .await
            {
                liquidity_blocked = true;
                continue;
            }

            routes.push(self.build_route(strategy, source, destination, slippage));
        }

        if routes.is_empty() {
            classify_empty_routes_error(liquidity_blocked)?;
        }

        // Exact-in prefers the highest destination amount, while exact-out prefers the cheapest source.
        if request.from_amount.is_some() {
            routes.sort_by(|left, right| {
                right
                    .destination
                    .amount
                    .partial_cmp(&left.destination.amount)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        } else {
            routes.sort_by(|left, right| {
                left.source
                    .amount
                    .partial_cmp(&right.source.amount)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        let best = routes.first().cloned();

        Ok(QuoteResponse {
            best,
            routes,
            input_token_price: input_price,
            output_token_price: output_price,
        })
    }

    /// Converts internal quote math results into the public route schema.
    fn build_route(
        &self,
        strategy: &Strategy,
        source: crate::quote::types::QuoteAssetView,
        destination: crate::quote::types::QuoteAssetView,
        slippage: u64,
    ) -> QuoteRoute {
        QuoteRoute {
            strategy_id: strategy.id.clone(),
            source,
            destination,
            solver_id: self.liquidity.solver_id().to_string(),
            estimated_time: self.settings.default_eta_seconds,
            slippage,
            fee: strategy.fee,
            fixed_fee: strategy.fixed_fee.clone(),
        }
    }

    /// Exposes a strategy lookup for create-order after route selection.
    pub fn strategy(&self, id: &str) -> Option<&Strategy> {
        self.registry.strategy(id)
    }

    /// Resolves either an HTLC-form asset or a canonical asset ID into metadata.
    fn resolve_asset(&self, value: &str) -> Result<&AssetMetadata, AppError> {
        let mut parts = value.splitn(2, ':');
        let chain = parts.next().unwrap_or_default();
        let rest = parts.next().unwrap_or_default();
        self.metadata
            .get_asset_by_chain_and_htlc(chain, rest)
            .or_else(|| self.metadata.get_asset_by_id(value))
            .ok_or_else(|| AppError::bad_request(format!("unknown asset: {value}")))
    }
}

#[cfg(test)]
mod tests {
    use super::classify_empty_routes_error;
    use crate::error::AppError;

    #[test]
    fn classifies_liquidity_exhaustion_as_conflict() {
        let error = classify_empty_routes_error(true)
            .expect_err("expected liquidity exhaustion to produce an error");

        match error {
            AppError::Conflict(message) => {
                assert_eq!(message, "insufficient destination liquidity for requested route")
            }
            other => panic!("expected conflict error, got {other:?}"),
        }
    }
}
