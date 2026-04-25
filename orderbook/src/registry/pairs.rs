use crate::registry::StrategyRegistry;
use serde::Serialize;

/// Public summary of a supported order pair and the strategies behind it.
#[derive(Debug, Clone, Serialize)]
pub struct PairDescriptor {
    pub order_pair: String,
    pub strategies: Vec<String>,
}

/// Derives a stable list of pair descriptors from the loaded strategy registry.
pub fn derive_pairs(registry: &StrategyRegistry) -> Vec<PairDescriptor> {
    let mut pairs = registry
        .pairs()
        .map(|(pair, strategies)| PairDescriptor {
            order_pair: pair.clone(),
            strategies: strategies.keys().cloned().collect(),
        })
        .collect::<Vec<_>>();
    pairs.sort_by(|left, right| left.order_pair.cmp(&right.order_pair));
    pairs
}
