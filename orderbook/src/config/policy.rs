use crate::config::solver::{ChainConfig, SolverSettings};
use policy::SolverPolicyConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const DEFAULT_POLLING_INTERVAL_MS: u64 = 5_000;
const DEFAULT_CACHE_TTL_MS: u64 = 10_000;

/// Trimmed local policy configuration used for single-solver development.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PolicySettings {
    pub solver_id: String,
    pub solver_name: String,
    pub chains: HashMap<String, ChainConfig>,
    pub policy: SolverPolicyConfig,
}

impl PolicySettings {
    /// Reads and deserializes the policy settings file.
    pub fn from_toml(path: &str) -> eyre::Result<Self> {
        let config = config::Config::builder()
            .add_source(config::File::with_name(path))
            .build()?;
        Ok(config.try_deserialize()?)
    }

    /// Converts the local policy file into the runtime liquidity watcher settings.
    pub fn to_solver_settings(&self) -> SolverSettings {
        SolverSettings {
            solver_id: self.solver_id.clone(),
            solver_name: self.solver_name.clone(),
            polling_interval_ms: DEFAULT_POLLING_INTERVAL_MS,
            cache_ttl_ms: DEFAULT_CACHE_TTL_MS,
            chains: self.chains.clone(),
        }
    }
}
