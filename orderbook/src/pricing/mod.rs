/// Aggregator workers that feed canonical prices into the market-data service.
pub mod aggregators;
/// Market-data computation helpers such as VWMP and volatility.
pub mod computation;
/// Domain types for canonical assets, market state, and snapshots.
pub mod exchanges;
/// Canonical asset mapping between local assets and pricing identities.
pub mod mapping;
/// Background price refresh service and cache.
pub mod service;
/// Public pricing response types.
pub mod types;
