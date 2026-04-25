use serde::Serialize;

/// Liquidity view for one asset held by the configured solver.
#[derive(Debug, Clone, Serialize)]
pub struct AssetLiquidity {
    pub asset: String,
    pub balance: String,
    pub virtual_balance: String,
    pub readable_balance: String,
}

/// Full liquidity snapshot returned by the public API.
#[derive(Debug, Clone, Serialize)]
pub struct SolverLiquidity {
    pub solver_id: String,
    pub liquidity: Vec<AssetLiquidity>,
}
