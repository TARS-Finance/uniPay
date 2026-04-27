use std::{collections::HashMap, sync::Arc, time::Duration};

use api::primitives::{Response, Status};
use arc_swap::ArcSwap;
use eyre::{Context, Result};
use primitives::AssetId;
use reqwest::{Client, Url};
use tracing::info;

use crate::{primitives::Fee, SolverInfo, SolverPolicy, SolverPolicyConfig};

/// Default refresh interval in seconds
const DEFAULT_REFRESH_INTERVAL_SECS: u64 = 30;

/// Default solver aggregator timeout in seconds
const SOLVER_AGGREGATOR_TIMEOUT_SECS: u64 = 10;

/// Policy implementation that fetches and manages policies for multiple solvers.
///
/// The `Policy` struct provides a centralized way to manage solver policies from a solver aggregator.
/// It fetches solver configurations including supported assets, fee structures, and validation
/// rules. It also supports automatic background refresh of policies at configurable intervals.
///
/// This implementation is thread-safe and uses lock-free atomic operations (`ArcSwap`) to
/// enable concurrent reads while policies are being updated, ensuring zero contention between
/// readers and the refresh task.
///
/// # Examples
///
/// ## Basic usage
///
/// ```no_run
/// use policy::Policy;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// // Create a policy instance with default refresh interval
/// let policy = Policy::new("http://localhost:4466", None).await?;
///
/// // Policies are now available for validation
/// println!("Loaded policies from: {}", policy.solver_aggregator_url());
/// # Ok(())
/// # }
/// ```
///
/// ## With background refresh
///
/// ```no_run
/// use policy::Policy;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let mut policy = Policy::new("http://localhost:4466", Some(60)).await?;
///
/// // Spawn the refresh loop in a background task
/// tokio::spawn(async move {
///     policy.start().await;
/// });
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Policy {
    /// The base URL of the solver aggregator service
    solver_aggregator_url: Url,
    /// Policies for all configured solvers keyed by solver id.
    /// Uses `ArcSwap` to provide lock-free atomic updates and concurrent read access.
    solvers: ArcSwap<HashMap<String, SolverPolicy>>,
    /// Interval for refreshing the solvers info
    refresh_interval_secs: u64,
}

impl Policy {
    /// Creates a new Policy instance by fetching policy configuration from the solver aggregator.
    ///
    /// This method will make an HTTP request to the solver aggregator's solvers endpoint
    /// and load the configuration into memory.
    ///
    /// # Arguments
    ///
    /// * `solver_aggregator_url` - The base URL of the solver aggregator service
    /// * `refresh_interval_secs` - Optional interval in seconds for refreshing solver policies.
    ///   Defaults to 30 seconds if not provided.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The solver aggregator URL is invalid
    /// - The solvers endpoint is unreachable
    /// - The solvers response is malformed or indicates an error
    /// - The solvers data is missing or invalid
    ///
    /// # Examples
    ///
    /// ```rust
    /// use policy::Policy;
    ///
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// // Use default refresh interval (30 seconds)
    /// let policy = Policy::new("http://localhost:4466", None).await?;
    ///
    /// // Use custom refresh interval (60 seconds)
    /// let policy = Policy::new("http://localhost:4466", Some(60)).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(
        solver_aggregator_url: &str,
        refresh_interval_secs: Option<u64>,
    ) -> Result<Self> {
        let solver_aggregator_url =
            Url::parse(solver_aggregator_url).context("Invalid solver aggregator URL format")?;

        let solvers = Self::get_solver_policies(&solver_aggregator_url).await?;

        Ok(Self {
            solver_aggregator_url,
            solvers: ArcSwap::from_pointee(solvers),
            refresh_interval_secs: refresh_interval_secs.unwrap_or(DEFAULT_REFRESH_INTERVAL_SECS),
        })
    }

    /// Starts a background refresh loop that continuously updates solver policies.
    ///
    /// This method runs indefinitely, periodically fetching fresh solver policies from the
    /// solver aggregator at the configured refresh interval. If a fetch fails, the error is logged
    /// and the loop waits for the refresh interval before retrying to avoid hammering
    /// the solver aggregator endpoint.
    ///
    /// Policy updates are performed atomically using `ArcSwap`, ensuring that concurrent
    /// readers always see a consistent snapshot and are never blocked during updates.
    ///
    /// # Note
    ///
    /// This method will block the current task. It should typically be spawned
    /// in a separate tokio task to run concurrently with other operations.
    pub async fn start(&self) {
        info!("Starting policy refresh");
        loop {
            match Self::get_solver_policies(&self.solver_aggregator_url).await {
                Ok(solvers) => self.solvers.store(Arc::new(solvers)),
                Err(e) => {
                    tracing::error!("Failed to fetch policy configuration: {}", e);
                }
            };
            tokio::time::sleep(Duration::from_secs(self.refresh_interval_secs)).await;
        }
    }

    /// Fetches solver information from the solver aggregator and constructs SolverPolicy instances.
    ///
    /// This internal helper method retrieves solver information from the solver aggregator and
    /// converts it into a HashMap of solver policies. If a solver policy fails to be
    /// created, an error is logged and that solver is skipped.
    ///
    /// # Arguments
    ///
    /// * `solver_aggregator_url` - The base URL of the solver aggregator service
    ///
    /// # Returns
    ///
    /// A HashMap mapping solver IDs to their respective SolverPolicy instances.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - Failed to fetch solver information from the solver aggregator
    async fn get_solver_policies(
        solver_aggregator_url: &Url,
    ) -> Result<HashMap<String, SolverPolicy>> {
        let solvers_info = match Self::fetch_solvers_info(&solver_aggregator_url).await {
            Ok(solvers_info) => solvers_info,
            Err(e) => return Err(eyre::eyre!("Failed to fetch policy configuration: {}", e)),
        };

        let mut solvers = HashMap::new();

        for solver in solvers_info {
            let policy_config = SolverPolicyConfig::from(solver.clone());
            let supported_assets = solver
                .chains
                .iter()
                .flat_map(|chain| chain.assets.clone())
                .collect();
            let policy = match SolverPolicy::new(policy_config, supported_assets) {
                Ok(policy) => policy,
                Err(e) => {
                    tracing::error!("Failed to create solver policy: {}", e);
                    continue;
                }
            };
            solvers.insert(solver.id, policy);
        }

        Ok(solvers)
    }

    /// Fetches solver information from the solver aggregator's solvers endpoint.
    ///
    /// This internal helper method makes an HTTP request to the solver aggregator's solvers
    /// endpoint and returns a list of solver information including their supported
    /// assets, fee structures, and other configuration details.
    ///
    /// # Arguments
    ///
    /// * `solver_aggregator_url` - The base URL of the solver aggregator service
    ///
    /// # Returns
    ///
    /// A vector of SolverInfo containing configuration for all solvers.
    async fn fetch_solvers_info(solver_aggregator_url: &Url) -> Result<Vec<SolverInfo>> {
        let solvers_endpoint = solver_aggregator_url
            .join("solvers")
            .context("Failed to construct solver aggregator endpoint URL")?;

        let client = Client::builder()
            .timeout(Duration::from_secs(SOLVER_AGGREGATOR_TIMEOUT_SECS))
            .build()
            .context("Failed to build HTTP client")?;

        let response = client
            .get(solvers_endpoint.as_str())
            .send()
            .await
            .context("Failed to send request to solver aggregator endpoint")?;

        if !response.status().is_success() {
            return Err(eyre::eyre!(
                "Solver aggregator endpoint returned error status: {}",
                response.status()
            ));
        }

        let solvers_info_response: Response<Vec<SolverInfo>> = response
            .json()
            .await
            .context("Failed to parse solvers info response as JSON")?;

        match solvers_info_response.status {
            Status::Ok => solvers_info_response.result.ok_or_else(|| {
                eyre::eyre!("Solvers info response is missing data despite Ok status")
            }),
            Status::Error => {
                let error_msg = solvers_info_response
                    .error
                    .unwrap_or_else(|| "Unknown error".to_string());
                Err(eyre::eyre!(
                    "Solver aggregator endpoint returned error: {}",
                    error_msg
                ))
            }
        }
    }

    /// Returns a reference to the solver aggregator URL.
    ///
    /// This is the base URL that was used to fetch the policy configuration.
    pub fn solver_aggregator_url(&self) -> &Url {
        &self.solver_aggregator_url
    }

    /// Returns an Arc-backed clone of all solver policies.
    ///
    /// This provides access to a consistent snapshot of all solver policies loaded from
    /// the solver aggregator. The returned `Arc` can be safely shared across threads and will
    /// remain valid even if the policies are refreshed in the background.
    ///
    /// Each `SolverPolicy` can be used to validate trades and retrieve fees.
    pub fn solvers(&self) -> Arc<HashMap<String, SolverPolicy>> {
        self.solvers.load_full()
    }

    /// Returns a HashMap of solver IDs to their fees for solvers that support the given asset pair.
    ///
    /// This method iterates through all configured solvers and checks which ones support
    /// trading between the given source and destination assets. For each solver that supports
    /// the pair, it returns the applicable fee (including any route-specific overrides).
    ///
    /// The method uses a consistent snapshot of the current policies, ensuring that all
    /// validations are performed against the same policy state.
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    ///
    /// # Returns
    ///
    /// A HashMap mapping solver IDs to their fees for the given trading pair.
    /// Only solvers that support and allow this pair will be included in the result.
    pub fn get_solvers_for_pair(
        &self,
        source: &AssetId,
        destination: &AssetId,
    ) -> HashMap<String, Fee> {
        let mut result = HashMap::new();

        let solvers = self.solvers.load();

        for (solver_id, solver_policy) in solvers.iter() {
            // Try to validate and get the fee for this solver
            if let Ok(fee) = solver_policy.validate_and_get_fee(source, destination) {
                result.insert(solver_id.clone(), fee);
            }
            // If validation fails, we simply skip this solver
        }

        result
    }

    /// Validates a route for a specific solver and returns its fee.
    ///
    /// This method performs validation for the specified solver, checking that the solver exists in the policy configuration and that the route is valid.
    /// - That the solver exists in the policy configuration
    /// - That both assets are supported by the solver
    /// - That the pair is not blacklisted
    ///
    /// If validation succeeds, it returns the applicable fee (including any route-specific overrides).
    ///
    /// The method uses a consistent snapshot of the current policies for validation.
    ///
    /// # Arguments
    ///
    /// * `source` - The source asset identifier
    /// * `destination` - The destination asset identifier
    /// * `solver_id` - The unique identifier of the solver to validate against
    ///
    /// # Returns
    ///
    /// * `Ok(Fee)` - The trade is allowed and the applicable fee is returned
    /// * `Err(...)` - An error occurred, either:
    ///   - The solver ID was not found
    ///   - The route validation failed (unsupported assets, isolation rules, blacklist, etc.)
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The solver ID does not exist in the policy configuration
    /// - The route fails validation according to the solver's policy rules
    pub fn validate_route_and_get_fee(
        &self,
        source: &AssetId,
        destination: &AssetId,
        solver_id: &str,
    ) -> Result<Fee> {
        let solvers = self.solvers.load();
        let solver_policy = solvers
            .get(solver_id)
            .ok_or_else(|| eyre::eyre!("Solver with ID '{}' not found", solver_id))?;

        solver_policy.validate_and_get_fee(source, destination)
    }
}
