//! Async driver for the Bitcoin wallet batching runtime.
//!
//! The runner owns the event loop that accepts queued [`WalletRequest`] values,
//! rebuilds runtime state from persistence on startup, chooses batch plans,
//! builds transactions, persists them, broadcasts them, and handles
//! confirmations or missing-lineage reconciliation.
//!
//! Execution outline:
//!
//! ```text
//! submit() / submit_and_wait()
//!   -> store.enqueue(dedupe_key)
//!   -> runner receives RunnerCommand::Submit
//!   -> next tick observes chain + handles confirmations/reconciliation
//!   -> planner chooses Fresh / Rbf / Chained
//!   -> builder creates tx using current wallet + lineage context
//!   -> store.persist_broadcast(...)
//!   -> broadcaster submits to bitcoind
//!   -> runtime records new live lineage head
//!   -> waiters resolve once a concrete txid exists
//! ```
//!
//! Concrete example:
//!
//! ```text
//! req_a submitted first          -> pending.free = [req_a]
//! tick builds tx_1               -> lineage L(head=tx_1, requests=[req_a])
//! req_b submitted while tx_1 live
//! tick may choose Rbf(L, [req_b])
//!   -> tx_2 replaces tx_1
//!   -> req_a history = [tx_1, tx_2]
//!   -> req_b history = [tx_2]
//! tx_2 confirms
//!   -> confirmation handler marks winner and may requeue orphaned work
//! ```
//!
//! Chained example:
//!
//! ```text
//! tx_2 confirms and creates change output C
//! req_c is requeued as pending.anchored(anchor=C)
//! next tick may choose Chained(anchor=C, [req_c])
//!
//! if no lineage already owns C:
//!   create new lineage M whose first tx spends C
//!
//! if an anchor-owned lineage already exists:
//!   build an anchor-aware RBF replacement inside that lineage instead
//! ```
//!
//! The file is easiest to read in this order:
//!
//! 1. `BitcoinWalletRunnerHandle` explains how callers enqueue and wait.
//! 2. `process_tick` explains the runtime loop and phase ordering.
//! 3. `execute_action` / `submit_built_lineage` explain submission paths.
//! 4. `handle_confirmation` / `handle_missing_lineage` explain repair paths.
//! 5. `BitcoinWalletChainObserver` explains how tx state is observed.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bitcoin::{
    consensus::encode::{deserialize, serialize_hex},
    OutPoint, ScriptBuf, Transaction, Txid,
};
use tokio::sync::{mpsc, oneshot, Mutex};

use super::store::ResolvePendingWalletRequestResult;
use super::{
    extract_chain_anchor, partition_confirmed_lineage, persist_and_submit_broadcast,
    reconcile_missing_lineage, recover_wallet_runtime, BroadcastAcceptance,
    BroadcastPersistenceKind, BroadcastPersistencePlan, BroadcastSubmissionResult, ChainAnchor,
    ConfirmationPersistencePlan, CoverUtxo, LineageId, LiveLineage, PendingLineageConfirmation,
    PendingWalletRequest, PlannedBatchAction, PlannerCostEvaluator, ReconciledLineageState,
    WalletBatchBroadcaster, WalletConfig, WalletRequest, WalletRuntimeState, WalletStore,
    WalletTxObserver,
};
use crate::errors::{ChainError, ExecutorError};
use crate::timestamp::Timestamp;
use crate::infrastructure::chain::bitcoin::clients::{
    BitcoinClientError, BitcoindRpcClient, ElectrsClient, EsploraTx,
};
use crate::infrastructure::chain::bitcoin::fee_providers::{FeeLevel, FeeRateEstimator};
use crate::infrastructure::chain::bitcoin::tx_builder::builder::WalletBuildOptions;
use crate::infrastructure::chain::bitcoin::tx_builder::{
    BitcoinTxBuilder, RbfFeeContext, TxBuilderError,
};
use crate::infrastructure::chain::bitcoin::wallet::runtime::PlannedActionExecutor;
use crate::infrastructure::keys::BitcoinWallet;

const SUBMISSION_TIMEOUT_SECS: u64 = 5 * 60;
const RETRY_ATTEMPTS: usize = 3;
const RETRY_DELAY_MS: u64 = 250;

/// Notification emitted after a batch submission is accepted for tracking by
/// external consumers such as the fee handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubmittedWalletBatch {
    /// Stable lineage identity after the submission has been accepted.
    pub lineage_id: LineageId,
    /// Concrete txid chosen for the submitted batch head.
    pub txid: Txid,
    /// Every logical request represented by the submitted tx.
    pub request_keys: Vec<String>,
    /// Previous head txid if this submission replaced an older lineage head.
    pub replaces: Option<Txid>,
}

/// Write-side API exposed to the rest of the application for interacting with
/// the wallet runner.
#[async_trait]
pub trait WalletRequestSubmitter: Send + Sync {
    /// Enqueue a request and return once it is durably pending.
    async fn submit(&self, request: WalletRequest) -> Result<(), ExecutorError>;
    /// Enqueue a request and wait until a txid is known for it.
    async fn submit_and_wait(&self, request: WalletRequest) -> Result<Txid, ExecutorError>;
    /// Resolve a dedupe key that may still be pending or may already have been
    /// attached to a submitted lineage.
    async fn resolve_pending(
        &self,
        dedupe_key: &str,
    ) -> Result<ResolvePendingRequestResult, ExecutorError>;
}

/// Outcome of asking the runner to resolve a pending dedupe key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvePendingRequestResult {
    /// The request never made it into a submitted batch and was cancelled/dropped locally.
    CancelledPending,
    /// The request had already crossed the submission boundary.
    AlreadySubmitted(Option<Txid>),
}

enum RunnerCommand {
    Submit(Box<WalletRequest>),
    ResolvePending {
        dedupe_key: String,
        response_tx: oneshot::Sender<Result<ResolvePendingRequestResult, ExecutorError>>,
    },
}

/// Cloneable handle used by application code to talk to the single runner task.
#[derive(Clone)]
pub struct BitcoinWalletRunnerHandle {
    /// Persistence namespace for this wallet/network pair.
    scope: String,
    /// Shared store used for enqueue/idempotency checks before talking to the runner task.
    store: Arc<dyn WalletStore>,
    /// Command channel into the single owning runner task.
    request_tx: mpsc::Sender<RunnerCommand>,
    /// Per-dedupe-key waiters used by `submit_and_wait`.
    waiters: Arc<Mutex<HashMap<String, Vec<oneshot::Sender<Txid>>>>>,
}

#[async_trait]
impl WalletRequestSubmitter for BitcoinWalletRunnerHandle {
    async fn submit(&self, request: WalletRequest) -> Result<(), ExecutorError> {
        self.enqueue_request(request, None).await.map(|_| ())
    }

    async fn submit_and_wait(&self, request: WalletRequest) -> Result<Txid, ExecutorError> {
        let _dedupe_key = request.dedupe_key().to_string();
        let (waiter_tx, waiter_rx) = oneshot::channel();

        if let Some(txid) = self.enqueue_request(request, Some(waiter_tx)).await? {
            return Ok(txid);
        }

        tokio::time::timeout(Duration::from_secs(SUBMISSION_TIMEOUT_SECS), waiter_rx)
            .await
            .map_err(|_| ExecutorError::Chain(ChainError::TxTimeout))?
            .map_err(|_| {
                ExecutorError::Chain(ChainError::WorkerChannel(
                    "bitcoin wallet runner waiter dropped".into(),
                ))
            })
    }

    async fn resolve_pending(
        &self,
        dedupe_key: &str,
    ) -> Result<ResolvePendingRequestResult, ExecutorError> {
        let (response_tx, response_rx) = oneshot::channel();
        self.request_tx
            .send(RunnerCommand::ResolvePending {
                dedupe_key: dedupe_key.to_string(),
                response_tx,
            })
            .await
            .map_err(|_| {
                ExecutorError::Chain(ChainError::WorkerChannel(
                    "bitcoin wallet runner channel closed".into(),
                ))
            })?;
        response_rx.await.map_err(|_| {
            ExecutorError::Chain(ChainError::WorkerChannel(
                "bitcoin wallet runner resolve response dropped".into(),
            ))
        })?
    }
}

impl BitcoinWalletRunnerHandle {
    /// Enqueue a request and optionally register a waiter for the eventual txid.
    async fn enqueue_request(
        &self,
        request: WalletRequest,
        waiter: Option<oneshot::Sender<Txid>>,
    ) -> Result<Option<Txid>, ExecutorError> {
        let dedupe_key = request.dedupe_key().to_string();

        match self.store.enqueue(&self.scope, &request).await? {
            super::EnqueueWalletRequestResult::EnqueuedPending => {
                // A freshly pending request has no txid yet; wake the runner so
                // a later batch build can pick it up.
                if let Some(waiter) = waiter {
                    self.waiters
                        .lock()
                        .await
                        .entry(dedupe_key)
                        .or_default()
                        .push(waiter);
                }
                self.request_tx
                    .send(RunnerCommand::Submit(Box::new(request)))
                    .await
                    .map_err(|_| {
                        ExecutorError::Chain(ChainError::WorkerChannel(
                            "bitcoin wallet runner channel closed".into(),
                        ))
                    })?;
                Ok(None)
            },
            super::EnqueueWalletRequestResult::AlreadyPending => {
                // Multiple callers may wait on the same logical request. They
                // all get completed once the eventual batch txid is known.
                if let Some(waiter) = waiter {
                    self.waiters
                        .lock()
                        .await
                        .entry(dedupe_key)
                        .or_default()
                        .push(waiter);
                }
                Ok(None)
            },
            super::EnqueueWalletRequestResult::AlreadyInflight { txid, .. }
            | super::EnqueueWalletRequestResult::AlreadyConfirmed { txid, .. } => {
                // The logical request is already attached to a transaction, so
                // return the known txid immediately instead of re-enqueueing.
                Ok(Some(txid))
            },
            super::EnqueueWalletRequestResult::AlreadyDropped => Err(ExecutorError::Domain(format!(
                "wallet request {} is already dropped",
                request.dedupe_key()
            ))),
        }
    }
}

/// Owning task that drives the Bitcoin wallet state machine and chain I/O.
pub struct BitcoinWalletRunner {
    /// Persistence namespace for this wallet/network pair.
    scope: String,
    wallet: Arc<BitcoinWallet>,
    store: Arc<dyn WalletStore>,
    tx_builder: BitcoinTxBuilder,
    electrs: Arc<ElectrsClient>,
    bitcoind: Arc<BitcoindRpcClient>,
    fee_estimator: Arc<dyn FeeRateEstimator>,
    config: WalletConfig,
    request_rx: mpsc::Receiver<RunnerCommand>,
    /// Direct `submit_and_wait` callers waiting for a concrete submitted txid.
    waiters: Arc<Mutex<HashMap<String, Vec<oneshot::Sender<Txid>>>>>,
    /// Optional downstream notification stream for accepted submissions.
    submitted_tx: Option<mpsc::Sender<SubmittedWalletBatch>>,
    runtime: WalletRuntimeState,
    /// Confirmations discovered earlier that still need durable handling.
    confirmations_pending: Vec<PendingLineageConfirmation>,
}

impl BitcoinWalletRunner {
    #[allow(clippy::too_many_arguments)]
    /// Construct a runner and recover persisted runtime state for its scope.
    ///
    /// Startup does real recovery work, not just wiring:
    ///
    /// - derive the wallet/network persistence scope,
    /// - load current block height,
    /// - probe persisted inflight heads,
    /// - return any confirmations that must be handled before the first tick.
    pub async fn new(
        wallet: Arc<BitcoinWallet>,
        store: Arc<dyn WalletStore>,
        electrs: Arc<ElectrsClient>,
        bitcoind: Arc<BitcoindRpcClient>,
        fee_estimator: Arc<dyn FeeRateEstimator>,
        network: bitcoin::Network,
        config: WalletConfig,
        submitted_tx: Option<mpsc::Sender<SubmittedWalletBatch>>,
    ) -> Result<(Self, Arc<BitcoinWalletRunnerHandle>), ExecutorError> {
        let scope = wallet_scope(network, wallet.as_ref());
        let waiters = Arc::new(Mutex::new(HashMap::new()));
        let (request_tx, request_rx) = mpsc::channel(1024);
        let current_height = current_height(electrs.as_ref()).await?;
        let observer = BitcoinWalletChainObserver::new(
            Arc::clone(&electrs),
            Arc::clone(&bitcoind),
            wallet.address().clone(),
        );
        let recovered =
            recover_wallet_runtime(store.as_ref(), &observer, &scope, current_height).await?;
        tracing::info!(
            scope = %scope,
            current_height,
            live_lineages = recovered.runtime.live_lineages.len(),
            pending_free = recovered.runtime.pending.free.len(),
            pending_anchored = recovered.runtime.pending.anchored.len(),
            confirmations_pending = recovered.confirmations_pending.len(),
            "bitcoin wallet runner recovered runtime state",
        );

        Ok((
            Self {
                scope: scope.clone(),
                wallet: Arc::clone(&wallet),
                store: Arc::clone(&store),
                tx_builder: BitcoinTxBuilder::new(wallet, electrs.clone(), network),
                electrs,
                bitcoind,
                fee_estimator,
                config,
                request_rx,
                waiters: Arc::clone(&waiters),
                submitted_tx,
                runtime: recovered.runtime,
                confirmations_pending: recovered.confirmations_pending,
            },
            Arc::new(BitcoinWalletRunnerHandle {
                scope,
                store,
                request_tx,
                waiters,
            }),
        ))
    }

    /// Run the main wallet event loop indefinitely.
    ///
    /// `incoming` is the short-lived pre-persistence queue. Requests stay here
    /// only until the next `process_tick`, after which they either become part
    /// of runtime pending state or are turned into submitted batches.
    pub async fn run(mut self) {
        let mut interval =
            tokio::time::interval(Duration::from_secs(self.config.tick_interval_secs));
        let mut incoming = Vec::new();

        loop {
            tokio::select! {
                maybe_request = self.request_rx.recv() => {
                    match maybe_request {
                        Some(RunnerCommand::Submit(request)) => {
                            tracing::info!(
                                scope = %self.scope,
                                dedupe_key = request.dedupe_key(),
                                "bitcoin wallet runner received request",
                            );
                            incoming.push(PendingWalletRequest {
                                request: *request,
                                chain_anchor: None,
                                created_at: Timestamp::now(),
                            });
                        }
                        Some(RunnerCommand::ResolvePending { dedupe_key, response_tx }) => {
                            // Resolution requests bypass the normal waiting path
                            // and ask "did this deduped logical request ever cross
                            // the durable submission boundary?"
                            let result = self.resolve_pending_request(&mut incoming, &dedupe_key).await;
                            let _ = response_tx.send(result);
                        }
                        // If every handle disappears, the runner has no more writers.
                        None => return,
                    }
                }
                _ = interval.tick() => {
                    // Every tick tries to converge chain state first, then plan
                    // and execute the next batch wave from the resulting runtime.
                    if let Err(error) = self.process_tick(&mut incoming).await {
                        tracing::error!(scope = %self.scope, %error, "bitcoin wallet runner tick failed");
                    }
                }
            }
        }
    }

    /// Resolve one dedupe key when a caller wants a definite "submitted or not?"
    /// answer instead of waiting for the normal batch lifecycle.
    ///
    /// Primary consumer: the coordinator HTLC worker's Bitcoin async submit /
    /// reclaim observation path. If address polling times out, it asks the
    /// runner whether the logical request was cancelled, already attached to a
    /// batch tx, or otherwise no longer pending.
    async fn resolve_pending_request(
        &mut self,
        incoming: &mut Vec<PendingWalletRequest>,
        dedupe_key: &str,
    ) -> Result<ResolvePendingRequestResult, ExecutorError> {
        // Remove any not-yet-planned copies first so store resolution reflects
        // only durable/persisted state instead of transient in-memory queues.
        incoming.retain(|request| request.request.dedupe_key() != dedupe_key);
        self.runtime
            .pending
            .free
            .retain(|request| request.request.dedupe_key() != dedupe_key);
        self.runtime
            .pending
            .anchored
            .retain(|request| request.request.dedupe_key() != dedupe_key);

        let result = self.store.resolve_pending(&self.scope, dedupe_key).await?;
        self.waiters.lock().await.remove(dedupe_key);

        Ok(match result {
            ResolvePendingWalletRequestResult::CancelledPending => {
                ResolvePendingRequestResult::CancelledPending
            },
            ResolvePendingWalletRequestResult::AlreadySubmitted { txid } => {
                ResolvePendingRequestResult::AlreadySubmitted(Some(txid))
            },
        })
    }

    /// Execute one full runner cycle.
    ///
    /// Phase order matters:
    ///
    /// 1. finish pending confirmations from earlier observations/startup
    /// 2. observe live lineage heads
    /// 3. reconcile heads that stayed missing beyond the threshold
    /// 4. preview housekeeping-adjusted runtime for planner costing
    /// 5. run the synchronous planner tick
    /// 6. execute the chosen batch actions
    ///
    /// This is the core "why" of the runner: planning only happens after the
    /// runner has converged live state as far as chain observation allows.
    ///
    /// Example:
    ///
    /// ```text
    /// tick starts with:
    ///   live_lineages = [L(head=tx_2)]
    ///   confirmations_pending = []
    ///   incoming = [req_c]
    ///
    /// if tx_2 confirms during observation:
    ///   handle_confirmation may requeue orphaned work first
    ///   planner then sees the post-confirmation pending state
    ///   req_c can be priced against the updated reality, not stale mempool state
    /// ```
    async fn process_tick(
        &mut self,
        incoming: &mut Vec<PendingWalletRequest>,
    ) -> Result<(), ExecutorError> {
        let current_height = current_height(self.electrs.as_ref()).await?;
        tracing::debug!(
            scope = %self.scope,
            current_height,
            incoming = incoming.len(),
            live_lineages = self.runtime.live_lineages.len(),
            pending_free = self.runtime.pending.free.len(),
            pending_anchored = self.runtime.pending.anchored.len(),
            confirmations_pending = self.confirmations_pending.len(),
            "bitcoin wallet runner tick started",
        );
        let observer = BitcoinWalletChainObserver::new(
            Arc::clone(&self.electrs),
            Arc::clone(&self.bitcoind),
            self.wallet.address().clone(),
        );

        // First finish any confirmations discovered earlier so planner inputs
        // reflect the latest durable lineage winners.
        let mut pending_confirmations = std::mem::take(&mut self.confirmations_pending);
        while let Some(confirmation) = pending_confirmations.pop() {
            if let Err(error) = self
                .handle_confirmation(confirmation.clone(), &observer, current_height)
                .await
            {
                tracing::warn!(
                    scope = %self.scope,
                    lineage_id = %confirmation.lineage.lineage_id,
                    confirmed_txid = %confirmation.confirmed_txid,
                    %error,
                    "bitcoin wallet runner confirmation handling failed; retrying later",
                );
                self.confirmations_pending.push(confirmation);
            }
        }

        let observation = self.runtime.observe_live_lineages(&observer).await;
        // Confirmations discovered during this observation sweep are handled on
        // the next loop iteration so the current sweep stays one-way/read-only.
        self.confirmations_pending
            .extend(observation.confirmations_pending.clone());

        // Missing heads are tolerated for a few polls to absorb Electrs/bitcoind
        // lag before expensive reconciliation rewrites lineage membership.
        let reconciliation_targets = self
            .runtime
            .live_lineages
            .keys()
            .copied()
            .filter(|lineage_id| {
                self.runtime.missing_observations(lineage_id) >= self.config.missing_batch_threshold
            })
            .collect::<Vec<_>>();

        for lineage_id in reconciliation_targets {
            let Some(lineage) = self.runtime.live_lineages.remove(&lineage_id) else {
                continue;
            };
            self.runtime.reset_missing_observations(&lineage_id);
            if let Err(error) = self
                .handle_missing_lineage(lineage.clone(), &observer, current_height)
                .await
            {
                tracing::warn!(
                    scope = %self.scope,
                    lineage_id = %lineage_id,
                    %error,
                    "bitcoin wallet runner reconciliation failed; restoring live lineage",
                );
                self.runtime.live_lineages.insert(lineage_id, lineage);
            }
        }

        let now = Timestamp::now();
        // Cost preparation is done against a preview that already accounts for
        // expiry/TTL housekeeping, so planner decisions match the real tick.
        let preview_runtime =
            self.preview_runtime_after_housekeeping(incoming.clone(), current_height, now);
        let evaluator = self.prepare_planner_evaluator(&preview_runtime).await;
        let mut executor = CollectingExecutor::default();
        self.runtime.tick(
            std::mem::take(incoming),
            current_height,
            now,
            &self.config,
            &evaluator,
            &mut executor,
        );
        // At this point runtime pending/live state is already updated
        // synchronously. What remains is the async execution of the chosen work.

        if !executor.actions.is_empty() {
            tracing::info!(
                scope = %self.scope,
                action_count = executor.actions.len(),
                "bitcoin wallet runner planned actions",
            );
        }

        for action in executor.actions {
            if let Err(error) = self.execute_action(action).await {
                tracing::warn!(scope = %self.scope, %error, "bitcoin wallet runner action execution failed");
            }
        }

        Ok(())
    }

    /// Build the planner preview state without mutating the real runtime.
    ///
    /// The preview exists so expensive cost calculations see the same pending
    /// set that the subsequent synchronous `runtime.tick()` will see after
    /// anchor expiry and TTL cleanup.
    ///
    /// It intentionally does not run observation or confirmation logic. Those
    /// must already have happened before previewing, otherwise planner prices
    /// would be computed against stale lineage state.
    fn preview_runtime_after_housekeeping(
        &self,
        incoming: Vec<PendingWalletRequest>,
        current_height: u64,
        now: Timestamp,
    ) -> WalletRuntimeState {
        let mut preview = self.runtime.clone();
        preview.current_height = current_height;
        for request in incoming {
            // Preview applies the same pending partitioning rules as the real
            // tick, but without mutating live runtime state yet.
            if request.chain_anchor.is_some() {
                preview.pending.anchored.push(request);
            } else {
                preview.pending.free.push(request);
            }
        }

        let required_confirmations = self.config.chain_anchor_confirmations;
        let mut anchored = Vec::new();
        for mut request in preview.pending.anchored.drain(..) {
            let Some(anchor) = request.chain_anchor.as_ref() else {
                request.chain_anchor = None;
                preview.pending.free.push(request);
                continue;
            };

            let confirmations = current_height
                .saturating_sub(anchor.confirmed_height)
                .saturating_add(1);
            if confirmations >= required_confirmations {
                // Preview mirrors the real tick: once the anchor is old enough,
                // the request becomes ordinary free work for planning purposes.
                request.chain_anchor = None;
                preview.pending.free.push(request);
            } else {
                anchored.push(request);
            }
        }
        preview.pending.anchored = anchored;

        let ttl = time::Duration::seconds(self.config.max_pending_ttl_secs as i64);
        // The planner should not price requests that will be dropped moments
        // later by the real tick.
        preview
            .pending
            .free
            .retain(|request| request.created_at.0 + ttl >= now.0);
        preview
            .pending
            .anchored
            .retain(|request| request.created_at.0 + ttl >= now.0);

        preview
    }

    /// Precompute every planner cost candidate that may be needed this tick.
    ///
    /// The planner itself stays pure/synchronous. This helper does the async
    /// builder and fee-estimation work first, then packages the answers into
    /// `PreparedPlannerEvaluator`.
    async fn prepare_planner_evaluator(
        &mut self,
        preview_runtime: &WalletRuntimeState,
    ) -> PreparedPlannerEvaluator {
        let mut evaluator = PreparedPlannerEvaluator::default();
        let fee_rate = market_fee_rate(self.fee_estimator.as_ref(), &self.config).await;
        tracing::debug!(
            scope = %self.scope,
            fee_rate,
            pending_free = preview_runtime.pending.free.len(),
            pending_anchored = preview_runtime.pending.anchored.len(),
            live_lineages = preview_runtime.live_lineages.len(),
            "bitcoin wallet runner preparing planner evaluator",
        );

        if !preview_runtime.pending.free.is_empty() {
            let free_requests = preview_runtime.pending.free.clone();
            let request_keys = sorted_request_key(&free_requests);
            // RBF is disabled at the planner layer: free requests always take
            // the Fresh path, so we only price the standalone candidate here.
            match self
                .cost_fresh(&free_requests, fee_rate, preview_runtime)
                .await
            {
                Ok(cost) => {
                    tracing::debug!(
                        scope = %self.scope,
                        request_keys = %request_keys,
                        fee_rate,
                        planner_cost = cost,
                        "bitcoin wallet runner prepared fresh planner cost",
                    );
                    evaluator.fresh.insert(request_keys.clone(), cost);
                },
                Err(error) => {
                    tracing::info!(
                        scope = %self.scope,
                        request_keys = %request_keys,
                        fee_rate,
                        %error,
                        "bitcoin wallet runner could not prepare fresh planner cost",
                    );
                },
            }
        }

        let anchored_groups = group_anchored_requests(&preview_runtime.pending.anchored);
        for (chain_anchor, requests) in anchored_groups {
            // Anchored requests are mandatory groups. The planner only decides
            // whether they extend an existing lineage or start a new chained one.
            let existing_lineage = preview_runtime
                .live_lineages
                .values()
                .filter(|lineage| lineage.chain_anchor.as_ref() == Some(&chain_anchor))
                .min_by_key(|lineage| lineage.lineage_id.to_string());
            if let Ok(cost) = self
                .cost_chained(
                    existing_lineage,
                    &chain_anchor,
                    &requests,
                    fee_rate,
                    preview_runtime,
                )
                .await
            {
                tracing::debug!(
                    scope = %self.scope,
                    existing_lineage_id = ?existing_lineage.map(|lineage| lineage.lineage_id),
                    anchor_outpoint = %chain_anchor.change_outpoint,
                    request_keys = %sorted_request_key(&requests),
                    fee_rate,
                    planner_cost = cost,
                    "bitcoin wallet runner prepared chained planner cost",
                );
                evaluator.chained.insert(
                    (
                        existing_lineage.map(|lineage| lineage.lineage_id),
                        sorted_request_key(&requests),
                    ),
                    cost,
                );
            }

            if !preview_runtime.pending.free.is_empty() {
                let mut mixed = requests.clone();
                mixed.extend(preview_runtime.pending.free.clone());
                // This prices the "absorb all free work into this chained group"
                // alternative. The planner later picks at most one such mix-in.
                let existing_lineage = preview_runtime
                    .live_lineages
                    .values()
                    .filter(|lineage| lineage.chain_anchor.as_ref() == Some(&chain_anchor))
                    .min_by_key(|lineage| lineage.lineage_id.to_string());
                if let Ok(cost) = self
                    .cost_chained(
                        existing_lineage,
                        &chain_anchor,
                        &mixed,
                        fee_rate,
                        preview_runtime,
                    )
                    .await
                {
                    tracing::debug!(
                        scope = %self.scope,
                        existing_lineage_id = ?existing_lineage.map(|lineage| lineage.lineage_id),
                        anchor_outpoint = %chain_anchor.change_outpoint,
                        request_keys = %sorted_request_key(&mixed),
                        fee_rate,
                        planner_cost = cost,
                        "bitcoin wallet runner prepared chained planner cost for mixed group",
                    );
                    evaluator.chained.insert(
                        (
                            existing_lineage.map(|lineage| lineage.lineage_id),
                            sorted_request_key(&mixed),
                        ),
                        cost,
                    );
                }
            }
        }

        evaluator
    }

    /// Price a brand-new lineage that contains exactly `requests`.
    async fn cost_fresh(
        &mut self,
        requests: &[PendingWalletRequest],
        fee_rate: f64,
        runtime: &WalletRuntimeState,
    ) -> Result<u64, TxBuilderError> {
        // Fresh costing deliberately clears carried-forward cover inputs so the
        // estimate reflects a brand-new lineage with no mempool ancestry.
        self.tx_builder.clear_inflight_cover_utxos();
        let build = self
            .tx_builder
            .build_wallet_tx_with_options(
                &wallet_requests(requests),
                fee_rate,
                WalletBuildOptions {
                    ignored_cover_outpoints: ignored_cover_outpoints(runtime, None, None),
                    min_change_value: self.config.min_change_value,
                    ..WalletBuildOptions::default()
                },
            )
            .await?;
        tracing::debug!(
            scope = %self.scope,
            request_keys = %sorted_request_key(requests),
            fee_rate,
            txid = %build.tx.compute_txid(),
            fee_paid_sats = build.fee_paid_sats,
            cover_utxo_count = build.cover_utxos.len(),
            "bitcoin wallet runner computed fresh build cost inputs",
        );
        planner_cost_from_build(&build, None)
    }

    /// Price a batch that must spend from a confirmed `chain_anchor`.
    ///
    /// There are two cases:
    ///
    /// - `existing_lineage == None`: first descendant batch from that anchor
    /// - `existing_lineage == Some(..)`: anchor-aware RBF of an existing child lineage
    async fn cost_chained(
        &mut self,
        existing_lineage: Option<&LiveLineage>,
        chain_anchor: &ChainAnchor,
        requests: &[PendingWalletRequest],
        fee_rate: f64,
        runtime: &WalletRuntimeState,
    ) -> Result<u64, TxBuilderError> {
        let chain_prevout = Some(CoverUtxo {
            outpoint: chain_anchor.change_outpoint,
            value: chain_anchor.change_value,
            script_pubkey: chain_anchor.change_script_pubkey.clone(),
        });
        let mut merged = existing_lineage
            .map(LiveLineage::wallet_requests)
            .unwrap_or_default();
        // Chained pricing always includes the anchor prevout explicitly, and
        // may also extend an existing anchor-owning lineage via RBF.
        merged.extend(wallet_requests(requests));
        if let Some(lineage) = existing_lineage {
            let Some(rbf_context) =
                fetch_rbf_context(self.bitcoind.as_ref(), lineage.head_txid).await
            else {
                return Err(TxBuilderError::Client("rbf context unavailable".into()));
            };
            let previous_total_fee = rbf_context.previous_total_fee;
            tracing::debug!(
                scope = %self.scope,
                lineage_id = %lineage.lineage_id,
                head_txid = %lineage.head_txid,
                anchor_outpoint = %chain_anchor.change_outpoint,
                previous_fee_rate = rbf_context.previous_fee_rate,
                previous_total_fee = rbf_context.previous_total_fee,
                descendant_fee = rbf_context.descendant_fee,
                carried_cover_utxo_count = lineage.cover_utxos.len(),
                "bitcoin wallet runner loaded chained-rbf context for planner cost",
            );
            self.tx_builder
                .set_inflight_cover_utxos(lineage.cover_utxos.clone());
            let build = self
                .tx_builder
                .build_wallet_tx_with_rbf_and_options(
                    &merged,
                    fee_rate,
                    rbf_context,
                    WalletBuildOptions {
                        lineage_prevout: chain_prevout,
                        min_change_value: self.config.min_change_value,
                        ignored_cover_outpoints: ignored_cover_outpoints(
                            runtime,
                            Some(lineage.lineage_id),
                            Some(chain_anchor),
                        ),
                    },
                )
                .await?;
            tracing::debug!(
                scope = %self.scope,
                lineage_id = %lineage.lineage_id,
                head_txid = %lineage.head_txid,
                anchor_outpoint = %chain_anchor.change_outpoint,
                request_keys = %sorted_request_key(requests),
                fee_rate,
                txid = %build.tx.compute_txid(),
                fee_paid_sats = build.fee_paid_sats,
                previous_total_fee,
                fee_delta = build.fee_paid_sats.saturating_sub(previous_total_fee),
                cover_utxo_count = build.cover_utxos.len(),
                "bitcoin wallet runner computed chained-rbf build cost inputs",
            );
            Ok(planner_cost_from_build(&build, Some(previous_total_fee))?)
        } else {
            self.tx_builder.clear_inflight_cover_utxos();
            let build = self
                .tx_builder
                .build_wallet_tx_with_options(
                    &merged,
                    fee_rate,
                    WalletBuildOptions {
                        lineage_prevout: chain_prevout,
                        min_change_value: self.config.min_change_value,
                        ignored_cover_outpoints: ignored_cover_outpoints(
                            runtime,
                            None,
                            Some(chain_anchor),
                        ),
                    },
                )
                .await?;
            tracing::debug!(
                scope = %self.scope,
                anchor_outpoint = %chain_anchor.change_outpoint,
                request_keys = %sorted_request_key(requests),
                fee_rate,
                txid = %build.tx.compute_txid(),
                fee_paid_sats = build.fee_paid_sats,
                cover_utxo_count = build.cover_utxos.len(),
                "bitcoin wallet runner computed chained-fresh build cost inputs",
            );
            Ok(planner_cost_from_build(&build, None)?)
        }
    }

    /// Turn one planner decision into a built transaction plus a persisted
    /// submission attempt.
    ///
    /// This is where abstract planner actions become concrete tx builds.
    ///
    /// The runtime has already decided *which* action should happen. This
    /// method decides *how to build it* and then delegates the persist/broadcast
    /// boundary to `submit_built_lineage`.
    async fn execute_action(&mut self, action: PlannedBatchAction) -> Result<(), ExecutorError> {
        let fee_rate = market_fee_rate(self.fee_estimator.as_ref(), &self.config).await;
        match action {
            PlannedBatchAction::Fresh { requests } => {
                tracing::info!(
                    scope = %self.scope,
                    request_keys = %sorted_request_key(&requests),
                    fee_rate,
                    "bitcoin wallet runner executing fresh batch",
                );
                self.tx_builder.clear_inflight_cover_utxos();
                let build = self
                    .tx_builder
                    .build_wallet_tx_with_options(
                        &wallet_requests(&requests),
                        fee_rate,
                        WalletBuildOptions {
                            ignored_cover_outpoints: ignored_cover_outpoints(
                                &self.runtime,
                                None,
                                None,
                            ),
                            min_change_value: self.config.min_change_value,
                            ..WalletBuildOptions::default()
                        },
                    )
                    .await
                    .map_err(map_builder_error)?;
                let lineage_id = LineageId::new();
                // Fresh creates a brand-new lineage with no prior head txid.
                self.submit_built_lineage(
                    BroadcastPersistenceKind::Fresh,
                    lineage_id,
                    None,
                    None,
                    Vec::new(),
                    requests,
                    build.tx,
                    build.cover_utxos,
                )
                .await
            },
            PlannedBatchAction::Rbf {
                lineage_id,
                requests,
            } => {
                tracing::info!(
                    scope = %self.scope,
                    lineage_id = %lineage_id,
                    request_keys = %sorted_request_key(&requests),
                    fee_rate,
                    "bitcoin wallet runner executing rbf batch",
                );
                let lineage = self
                    .runtime
                    .live_lineages
                    .get(&lineage_id)
                    .cloned()
                    .ok_or_else(|| {
                        ExecutorError::Domain(format!("missing live lineage {lineage_id} for rbf"))
                    })?;
                let rbf_context = fetch_rbf_context(self.bitcoind.as_ref(), lineage.head_txid)
                    .await
                    .ok_or_else(|| {
                        ExecutorError::Chain(ChainError::Rpc(format!(
                            "rbf context unavailable for {}",
                            lineage.head_txid
                        )))
                    })?;
                self.tx_builder
                    .set_inflight_cover_utxos(lineage.cover_utxos.clone());
                let mut merged = lineage.wallet_requests();
                // Replacement batches preserve prior surviving requests and add
                // any newly selected pending work into the same lineage.
                merged.extend(wallet_requests(&requests));
                let build = self
                    .tx_builder
                    .build_wallet_tx_with_rbf_and_options(
                        &merged,
                        fee_rate,
                        rbf_context,
                        WalletBuildOptions {
                            lineage_prevout: lineage.derived_lineage_prevout(),
                            min_change_value: self.config.min_change_value,
                            ignored_cover_outpoints: ignored_cover_outpoints(
                                &self.runtime,
                                Some(lineage.lineage_id),
                                lineage.chain_anchor.as_ref(),
                            ),
                        },
                    )
                    .await
                    .map_err(map_builder_error)?;
                self.submit_built_lineage(
                    BroadcastPersistenceKind::Rbf,
                    lineage_id,
                    Some(lineage.head_txid),
                    lineage.chain_anchor.clone(),
                    lineage.requests,
                    requests,
                    build.tx,
                    build.cover_utxos,
                )
                .await
            },
            PlannedBatchAction::Chained {
                existing_lineage_id,
                chain_anchor,
                requests,
            } => {
                tracing::info!(
                    scope = %self.scope,
                    existing_lineage_id = ?existing_lineage_id,
                    anchor_outpoint = %chain_anchor.change_outpoint,
                    request_keys = %sorted_request_key(&requests),
                    fee_rate,
                    "bitcoin wallet runner executing chained batch",
                );
                let chain_prevout = CoverUtxo {
                    outpoint: chain_anchor.change_outpoint,
                    value: chain_anchor.change_value,
                    script_pubkey: chain_anchor.change_script_pubkey.clone(),
                };

                if let Some(lineage_id) = existing_lineage_id {
                    // Existing chained lineage: treat this like an anchor-aware
                    // RBF so descendants keep the same confirmed parent output.
                    let lineage = self
                        .runtime
                        .live_lineages
                        .get(&lineage_id)
                        .cloned()
                        .ok_or_else(|| {
                            ExecutorError::Domain(format!(
                                "missing chained lineage {lineage_id} for rbf"
                            ))
                        })?;
                    let rbf_context = fetch_rbf_context(self.bitcoind.as_ref(), lineage.head_txid)
                        .await
                        .ok_or_else(|| {
                            ExecutorError::Chain(ChainError::Rpc(format!(
                                "rbf context unavailable for {}",
                                lineage.head_txid
                            )))
                        })?;
                    self.tx_builder
                        .set_inflight_cover_utxos(lineage.cover_utxos.clone());
                    let mut merged = lineage.wallet_requests();
                    merged.extend(wallet_requests(&requests));
                    let build = self
                        .tx_builder
                        .build_wallet_tx_with_rbf_and_options(
                            &merged,
                            fee_rate,
                            rbf_context,
                            WalletBuildOptions {
                                lineage_prevout: Some(chain_prevout),
                                min_change_value: self.config.min_change_value,
                                ignored_cover_outpoints: ignored_cover_outpoints(
                                    &self.runtime,
                                    Some(lineage.lineage_id),
                                    Some(&chain_anchor),
                                ),
                            },
                        )
                        .await
                        .map_err(map_builder_error)?;
                    self.submit_built_lineage(
                        BroadcastPersistenceKind::Chained,
                        lineage_id,
                        Some(lineage.head_txid),
                        Some(chain_anchor),
                        lineage.requests,
                        requests,
                        build.tx,
                        build.cover_utxos,
                    )
                    .await
                } else {
                    // No lineage owns this anchor yet, so this is the first
                    // descendant batch spending that confirmed change output.
                    self.tx_builder.clear_inflight_cover_utxos();
                    let lineage_id = LineageId::new();
                    let build = self
                        .tx_builder
                        .build_wallet_tx_with_options(
                            &wallet_requests(&requests),
                            fee_rate,
                            WalletBuildOptions {
                                lineage_prevout: Some(chain_prevout),
                                min_change_value: self.config.min_change_value,
                                ignored_cover_outpoints: ignored_cover_outpoints(
                                    &self.runtime,
                                    None,
                                    Some(&chain_anchor),
                                ),
                            },
                        )
                        .await
                        .map_err(map_builder_error)?;
                    self.submit_built_lineage(
                        BroadcastPersistenceKind::Chained,
                        lineage_id,
                        None,
                        Some(chain_anchor),
                        Vec::new(),
                        requests,
                        build.tx,
                        build.cover_utxos,
                    )
                    .await
                }
            },
        }
    }

    #[allow(clippy::too_many_arguments)]
    /// Persist, broadcast, and adopt one newly built lineage head.
    ///
    /// Example RBF transition:
    ///
    /// ```text
    /// existing lineage L:
    ///   head      = tx_1
    ///   requests  = [req_a]
    ///
    /// new build:
    ///   tx_2 contains [req_a, req_b]
    ///
    /// result on acceptance:
    ///   live_lineages[L].head_txid = tx_2
    ///   req_a.txid_history = [tx_1, tx_2]
    ///   req_b.txid_history = [tx_2]
    /// ```
    ///
    /// The key invariant is that persistence happens before network submission.
    /// Once this method receives `Accepted` or `Ambiguous`, runtime memory is
    /// advanced as though the new head exists, because recovery can reconstruct
    /// that same view from the stored receipt if the process crashes.
    async fn submit_built_lineage(
        &mut self,
        kind: BroadcastPersistenceKind,
        lineage_id: LineageId,
        replaces: Option<Txid>,
        chain_anchor: Option<ChainAnchor>,
        existing_requests: Vec<super::LiveLineageRequest>,
        new_requests: Vec<PendingWalletRequest>,
        tx: Transaction,
        cover_utxos: Vec<CoverUtxo>,
    ) -> Result<(), ExecutorError> {
        let txid = tx.compute_txid();
        let included_request_keys = existing_requests
            .iter()
            .map(|request| request.request.dedupe_key().to_string())
            .chain(
                new_requests
                    .iter()
                    .map(|request| request.request.dedupe_key().to_string()),
            )
            .collect::<Vec<_>>();
        tracing::info!(
            scope = %self.scope,
            lineage_id = %lineage_id,
            txid = %txid,
            replaces = ?replaces,
            kind = ?kind,
            request_count = included_request_keys.len(),
            "bitcoin wallet runner built batch transaction",
        );
        // The persistence plan records the candidate head before broadcast so a
        // crash after network acceptance can still be recovered deterministically.
        let plan = BroadcastPersistencePlan {
            kind,
            lineage_id,
            txid,
            raw_tx_hex: serialize_hex(&tx),
            included_request_keys: included_request_keys.clone(),
            dropped_request_keys: Vec::new(),
        };
        let broadcaster = BitcoinWalletBroadcaster::new(Arc::clone(&self.bitcoind));
        let result =
            persist_and_submit_broadcast(self.store.as_ref(), &broadcaster, &self.scope, &plan)
                .await?;

        match result {
            BroadcastSubmissionResult::Accepted(_) | BroadcastSubmissionResult::Ambiguous(_) => {
                // From the runner's perspective, "ambiguous" still means the tx
                // may already be on its way to the network, so runtime state and
                // waiters must move forward as if the submission happened.
                tracing::info!(
                    scope = %self.scope,
                    lineage_id = %lineage_id,
                    txid = %txid,
                    replaces = ?replaces,
                    request_count = included_request_keys.len(),
                    "bitcoin wallet runner accepted batch submission",
                );
                for request in &new_requests {
                    // Newly selected pending requests are no longer eligible
                    // for planning because they now belong to the submitted head.
                    remove_pending_request(&mut self.runtime, request.request.dedupe_key());
                }

                let mut requests = existing_requests;
                let txid_history_append = txid;
                for request in &mut requests {
                    // Existing lineage members record the new head so later
                    // confirmation/reconciliation can tell which requests survived.
                    if request.txid_history.last().copied() != Some(txid_history_append) {
                        request.txid_history.push(txid_history_append);
                    }
                }
                requests.extend(new_requests.into_iter().map(|pending| {
                    // New arrivals enter the lineage with a one-element history
                    // because this accepted head is the first tx that contains them.
                    super::LiveLineageRequest {
                        request: pending.request,
                        txid_history: vec![txid],
                        created_at: pending.created_at,
                    }
                }));

                let all_txids = match self.runtime.live_lineages.get(&lineage_id) {
                    Some(existing) if existing.all_txids.last().copied() != Some(txid) => {
                        // Replacements append to lineage-wide history so later
                        // missing-head reconciliation can scan every sibling.
                        let mut all_txids = existing.all_txids.clone();
                        all_txids.push(txid);
                        all_txids
                    },
                    Some(existing) => existing.all_txids.clone(),
                    None => vec![txid],
                };

                self.runtime.live_lineages.insert(
                    lineage_id,
                    LiveLineage {
                        lineage_id,
                        head_txid: txid,
                        all_txids,
                        requests,
                        cover_utxos,
                        chain_anchor,
                    },
                );
                self.runtime.reset_missing_observations(&lineage_id);
                // Emit the batch txid to direct waiters and to any downstream
                // consumers such as fee-accounting handlers.
                self.notify_submission(SubmittedWalletBatch {
                    lineage_id,
                    txid,
                    request_keys: included_request_keys,
                    replaces,
                })
                .await;
            },
            BroadcastSubmissionResult::Rejected(_) => {
                // Rejected submissions were already reverted inside
                // `persist_and_submit_broadcast`, so runtime state stays unchanged here.
                tracing::warn!(
                    scope = %self.scope,
                    lineage_id = %lineage_id,
                    txid = %txid,
                    "bitcoin wallet runner broadcast rejected",
                );
            },
        }

        Ok(())
    }

    /// Publish the concrete txid chosen for a submitted batch.
    ///
    /// This resolves direct `submit_and_wait` callers and also notifies
    /// downstream consumers such as fee-accounting code.
    async fn notify_submission(&self, batch: SubmittedWalletBatch) {
        tracing::info!(
            scope = %self.scope,
            lineage_id = %batch.lineage_id,
            txid = %batch.txid,
            replaces = ?batch.replaces,
            request_count = batch.request_keys.len(),
            "bitcoin wallet runner emitted submission event",
        );
        let mut waiters = self.waiters.lock().await;
        // Resolve every caller waiting on the deduped logical request set with
        // the concrete txid chosen by the wallet batcher.
        for dedupe_key in &batch.request_keys {
            if let Some(entries) = waiters.remove(dedupe_key) {
                for entry in entries {
                    let _ = entry.send(batch.txid);
                }
            }
        }
        drop(waiters);

        if let Some(submitted_tx) = &self.submitted_tx {
            let _ = submitted_tx.send(batch).await;
        }
    }

    /// Finalize a lineage whose winning tx is now confirmed.
    ///
    /// If some requests from the lineage did not survive into the confirmed
    /// winner, they are requeued. When the winner's change is still young
    /// enough, those requeued requests may carry a fresh `ChainAnchor`.
    ///
    /// Example:
    ///
    /// ```text
    /// lineage history:
    ///   tx_1 = [req_a]
    ///   tx_2 = [req_a, req_b]
    /// confirmed winner = tx_1
    ///
    /// outcome:
    ///   req_a -> confirmed
    ///   req_b -> pending or pending(anchor=C)
    /// ```
    async fn handle_confirmation(
        &mut self,
        confirmation: PendingLineageConfirmation,
        observer: &BitcoinWalletChainObserver,
        current_height: u64,
    ) -> Result<(), ExecutorError> {
        let confirmed_tx = observer.load_tx(confirmation.confirmed_txid).await?;
        let confirmed_height = confirmed_tx.status.block_height.ok_or_else(|| {
            ExecutorError::Domain(format!(
                "confirmed tx {} missing block height",
                confirmation.confirmed_txid
            ))
        })?;
        let partition =
            partition_confirmed_lineage(&confirmation.lineage, confirmation.confirmed_txid);
        let confirmations = current_height
            .saturating_sub(confirmed_height)
            .saturating_add(1);
        // A fresh confirmation may still produce usable wallet change for any
        // orphaned requests. Once it is old enough, the special chaining
        // constraint is dropped and those requests return as ordinary pending work.
        let chain_anchor = if partition.orphaned_requests.is_empty()
            || confirmations >= self.config.chain_anchor_confirmations
        {
            None
        } else {
            Some(extract_chain_anchor(
                confirmation.confirmed_txid,
                confirmed_height,
                &confirmed_tx,
                &self.wallet.address().script_pubkey(),
            )?)
        };
        tracing::info!(
            scope = %self.scope,
            lineage_id = %confirmation.lineage.lineage_id,
            confirmed_txid = %confirmation.confirmed_txid,
            confirmed_requests = partition.confirmed_requests.len(),
            orphaned_requests = partition.orphaned_requests.len(),
            anchor_created = chain_anchor.is_some(),
            "bitcoin wallet runner handling confirmed lineage",
        );

        let plan = ConfirmationPersistencePlan {
            lineage_id: confirmation.lineage.lineage_id,
            confirmed_txid: confirmation.confirmed_txid,
            confirmed_request_keys: partition
                .confirmed_requests
                .iter()
                .map(|request| request.dedupe_key().to_string())
                .collect(),
            orphaned_request_keys: partition
                .orphaned_requests
                .iter()
                .map(|request| request.dedupe_key().to_string())
                .collect(),
            chain_anchor: chain_anchor.clone(),
        };
        // Persist the winner/orphan split before mutating in-memory pending
        // state so restart recovery sees the same decision.
        self.store.persist_confirmation(&self.scope, &plan).await?;

        for orphan in partition.orphaned_requests {
            let created_at = confirmation
                .lineage
                .requests
                .iter()
                .find(|request| request.request.dedupe_key() == orphan.dedupe_key())
                .map(|request| request.created_at)
                .expect("orphan request must exist");
            let pending = PendingWalletRequest {
                request: orphan,
                chain_anchor: chain_anchor.clone(),
                created_at,
            };
            // Requeue orphaned work into the right pending bucket for the next
            // planner tick. The logical request survives; only its lineage membership changes.
            if pending.chain_anchor.is_some() {
                self.runtime.pending.anchored.push(pending);
            } else {
                self.runtime.pending.free.push(pending);
            }
        }

        Ok(())
    }

    /// Reconcile a lineage whose latest head stayed missing long enough that the
    /// runner no longer trusts it to reappear.
    ///
    /// The persistence helper decides whether:
    ///
    /// - an older sibling actually confirmed,
    /// - an older sibling is still live in mempool,
    /// - or nothing survived and every request must be requeued.
    ///
    /// Example:
    ///
    /// ```text
    /// all_txids = [tx_1, tx_2, tx_3]
    /// current head = tx_3
    ///
    /// if tx_3 is missing and tx_2 is still in mempool:
    ///   keep lineage live under tx_2
    ///   requeue only requests that never made it into tx_2
    ///
    /// if nothing survives:
    ///   delete live lineage state
    ///   requeue every logical request
    /// ```
    async fn handle_missing_lineage(
        &mut self,
        lineage: LiveLineage,
        observer: &BitcoinWalletChainObserver,
        current_height: u64,
    ) -> Result<(), ExecutorError> {
        tracing::info!(
            scope = %self.scope,
            lineage_id = %lineage.lineage_id,
            head_txid = %lineage.head_txid,
            "bitcoin wallet runner reconciling missing lineage",
        );
        let result = reconcile_missing_lineage(
            self.store.as_ref(),
            observer,
            &self.scope,
            &lineage,
            current_height,
            self.config.chain_anchor_confirmations,
            &self.wallet.address().script_pubkey(),
        )
        .await?;

        match result.state {
            ReconciledLineageState::Confirmed { .. } | ReconciledLineageState::NoSurvivor => {
                tracing::info!(
                    scope = %self.scope,
                    lineage_id = %lineage.lineage_id,
                    requeued_requests = result.requeued_requests.len(),
                    state = ?result.state,
                    "bitcoin wallet runner finished reconciliation",
                );
                for request in result.requeued_requests {
                    // These requests no longer belong to any live lineage, so
                    // they return to normal pending planning.
                    if request.chain_anchor.is_some() {
                        self.runtime.pending.anchored.push(request);
                    } else {
                        self.runtime.pending.free.push(request);
                    }
                }
            },
            ReconciledLineageState::InMempool { surviving_txid } => {
                tracing::info!(
                    scope = %self.scope,
                    lineage_id = %lineage.lineage_id,
                    surviving_txid = %surviving_txid,
                    requeued_requests = result.requeued_requests.len(),
                    "bitcoin wallet runner adopted surviving mempool lineage",
                );
                for request in &result.requeued_requests {
                    // Requests absent from the surviving sibling are requeued
                    // even though the lineage itself stays live under that sibling.
                    if request.chain_anchor.is_some() {
                        self.runtime.pending.anchored.push(request.clone());
                    } else {
                        self.runtime.pending.free.push(request.clone());
                    }
                }

                let mut survivor_requests = lineage
                    .requests
                    .iter()
                    // Survivors are exactly the requests whose history already
                    // records the adopted sibling txid.
                    .filter(|request| request.txid_history.contains(&surviving_txid))
                    .cloned()
                    .collect::<Vec<_>>();
                survivor_requests.sort_by_key(|request| request.request.dedupe_key().to_string());

                let cover_utxos = observer
                    .load_wallet_funding_inputs(surviving_txid)
                    .await?
                    .into_iter()
                    .filter(|utxo| {
                        lineage
                            .chain_anchor
                            .as_ref()
                            .map(|anchor| anchor.change_outpoint != utxo.outpoint)
                            .unwrap_or(true)
                    })
                    .collect();

                // Adopt the surviving sibling as the live head so future polls
                // and possible RBF attempts continue from the real winner.
                self.runtime.live_lineages.insert(
                    lineage.lineage_id,
                    LiveLineage {
                        lineage_id: lineage.lineage_id,
                        head_txid: surviving_txid,
                        all_txids: lineage.all_txids,
                        requests: survivor_requests,
                        cover_utxos,
                        chain_anchor: lineage.chain_anchor,
                    },
                );
            },
        }

        Ok(())
    }
}

/// Minimal executor that just records planner output for later async execution.
///
/// `WalletRuntimeState::tick` stays synchronous; this collector lets the runner
/// capture the chosen actions and execute them once the state transition ends.
#[derive(Default)]
struct CollectingExecutor {
    actions: Vec<PlannedBatchAction>,
}

impl PlannedActionExecutor for CollectingExecutor {
    fn execute(&mut self, action: PlannedBatchAction) {
        self.actions.push(action);
    }
}

/// Precomputed async cost answers supplied to the pure planner.
///
/// Keys are normalized request-key sets so planner lookups remain stable across
/// restarts and in-memory ordering differences.
#[derive(Default)]
struct PreparedPlannerEvaluator {
    fresh: HashMap<String, u64>,
    rbf: HashMap<(LineageId, String), u64>,
    chained: HashMap<(Option<LineageId>, String), u64>,
}

impl PlannerCostEvaluator for PreparedPlannerEvaluator {
    fn fresh_cost(&self, requests: &[PendingWalletRequest]) -> Option<u64> {
        self.fresh.get(&sorted_request_key(requests)).copied()
    }

    fn rbf_cost(&self, lineage: &LiveLineage, requests: &[PendingWalletRequest]) -> Option<u64> {
        self.rbf
            .get(&(lineage.lineage_id, sorted_request_key(requests)))
            .copied()
    }

    fn chained_cost(
        &self,
        _chain_anchor: &ChainAnchor,
        existing_lineage: Option<&LiveLineage>,
        requests: &[PendingWalletRequest],
    ) -> Option<u64> {
        self.chained
            .get(&(
                existing_lineage.map(|lineage| lineage.lineage_id),
                sorted_request_key(requests),
            ))
            .copied()
    }
}

struct BitcoinWalletBroadcaster {
    bitcoind: Arc<BitcoindRpcClient>,
}

impl BitcoinWalletBroadcaster {
    /// Create the concrete submission adapter used by `submit_built_lineage`.
    fn new(bitcoind: Arc<BitcoindRpcClient>) -> Self {
        Self { bitcoind }
    }
}

#[async_trait]
impl WalletBatchBroadcaster for BitcoinWalletBroadcaster {
    async fn broadcast(
        &self,
        _scope: &str,
        receipt: &super::PersistedBatchReceipt,
    ) -> Result<BroadcastAcceptance, ExecutorError> {
        match self
            .bitcoind
            .send_raw_transaction(&receipt.raw_tx_hex)
            .await
        {
            Ok(_) => Ok(BroadcastAcceptance::Accepted),
            // Some nodes reply with "already known" if a previous attempt or
            // peer announcement won the race. For the runner this still means
            // the submission should be treated as accepted.
            Err(BitcoinClientError::Rpc(message)) if is_known_submission(message.as_str()) => {
                Ok(BroadcastAcceptance::Accepted)
            },
            Err(BitcoinClientError::Rpc(message)) if is_rejected_submission(message.as_str()) => {
                tracing::warn!(
                    txid = %receipt.txid,
                    error = %message,
                    "bitcoin wallet broadcaster rejected transaction"
                );
                Ok(BroadcastAcceptance::Rejected)
            },
            // Unknown RPC errors are treated conservatively as rejected here.
            // The persisted receipt still preserves the pre-broadcast boundary.
            Err(error) => {
                tracing::warn!(
                    txid = %receipt.txid,
                    error = %error,
                    "bitcoin wallet broadcaster treating unknown broadcast error as rejected"
                );
                Ok(BroadcastAcceptance::Rejected)
            },
        }
    }
}

/// Chain observer used by the runner for confirmation and mempool state.
///
/// The observer prefers Electrs for lightweight status/raw tx reads, but falls
/// back to bitcoind when Electrs is stale or missing mempool-side detail.
pub struct BitcoinWalletChainObserver {
    electrs: Arc<ElectrsClient>,
    bitcoind: Arc<BitcoindRpcClient>,
    wallet_address: bitcoin::Address,
}

impl BitcoinWalletChainObserver {
    /// Create an observer that resolves tx state from Electrs first and falls
    /// back to bitcoind when Electrs is stale or lacks raw prevout detail.
    pub fn new(
        electrs: Arc<ElectrsClient>,
        bitcoind: Arc<BitcoindRpcClient>,
        wallet_address: bitcoin::Address,
    ) -> Self {
        Self {
            electrs,
            bitcoind,
            wallet_address,
        }
    }
}

#[async_trait]
impl WalletTxObserver for BitcoinWalletChainObserver {
    async fn observe_tx(&self, txid: Txid) -> Result<super::ObservedTxState, ExecutorError> {
        let txid = txid.to_string();
        match self.electrs.get_tx_status(&txid).await {
            Ok(status) if status.confirmed => Ok(super::ObservedTxState::Confirmed),
            Ok(_) => match self.bitcoind.get_mempool_entry(&txid).await {
                Ok(_) => Ok(super::ObservedTxState::InMempool),
                Err(err) if err.is_tx_not_found() => Ok(super::ObservedTxState::Missing),
                Err(err) => Err(ExecutorError::Chain(ChainError::Rpc(err.to_string()))),
            },
            Err(electrs_err) => match self.bitcoind.get_mempool_entry(&txid).await {
                // Electrs can lag behind mempool reality, so bitcoind acts as
                // the second opinion before the runner declares a tx missing.
                Ok(_) => Ok(super::ObservedTxState::InMempool),
                Err(bitcoind_err)
                    if electrs_err.is_tx_not_found() && bitcoind_err.is_tx_not_found() =>
                {
                    Ok(super::ObservedTxState::Missing)
                },
                Err(bitcoind_err) => {
                    Err(ExecutorError::Chain(ChainError::Rpc(bitcoind_err.to_string())))
                },
            },
        }
    }

    async fn load_wallet_funding_inputs(&self, txid: Txid) -> Result<Vec<CoverUtxo>, ExecutorError> {
        let wallet_script_pubkey = self.wallet_address.script_pubkey();
        let wallet_address = self.wallet_address.to_string();
        match self.load_tx(txid).await {
            // Electrs is preferred because prevout information arrives already
            // decoded in Esplora shape, which is cheaper than raw-tx traversal.
            Ok(tx) => extract_wallet_funding_inputs_from_esplora(
                tx,
                &wallet_script_pubkey,
                &wallet_address,
            ),
            Err(err) => {
                tracing::debug!(
                    %txid,
                    %err,
                    "bitcoin wallet observer failed to load tx from electrs, falling back to bitcoind raw transaction",
                );
                self.load_wallet_funding_inputs_from_bitcoind(
                    txid,
                    &wallet_script_pubkey,
                    &wallet_address,
                )
                .await
            },
        }
    }

    async fn load_tx(&self, txid: Txid) -> Result<EsploraTx, ExecutorError> {
        self.electrs
            .get_tx(&txid.to_string())
            .await
            .map_err(|err| ExecutorError::Chain(ChainError::Rpc(err.to_string())))
    }
}

impl BitcoinWalletChainObserver {
    /// Fall back to bitcoind raw transactions when Electrs cannot provide the
    /// wallet-owned funding inputs for a lineage head.
    ///
    /// This path is slower because it has to fetch the tx and then each
    /// previous tx needed to inspect the consumed outputs, but it preserves
    /// recovery when Electrs lacks mempool prevout detail.
    async fn load_wallet_funding_inputs_from_bitcoind(
        &self,
        txid: Txid,
        wallet_script_pubkey: &ScriptBuf,
        _wallet_address: &str,
    ) -> Result<Vec<CoverUtxo>, ExecutorError> {
        let raw_hex = self
            .bitcoind
            .get_raw_transaction_hex(&txid.to_string())
            .await
            .map_err(|err| ExecutorError::Chain(ChainError::Rpc(err.to_string())))?;
        let tx: Transaction = deserialize(&hex::decode(raw_hex).map_err(|err| {
            ExecutorError::Chain(ChainError::DecodeFailed(format!(
                "invalid bitcoind raw tx hex: {err}"
            )))
        })?)
        .map_err(|err| {
            ExecutorError::Chain(ChainError::DecodeFailed(format!(
                "invalid bitcoind raw tx bytes: {err}"
            )))
        })?;

        let mut funding_inputs = Vec::new();
        for input in tx.input {
            // Raw transaction responses do not inline prevout bodies, so the
            // observer fetches the previous transaction to recover the spent output.
            let prev_tx_hex = self
                .bitcoind
                .get_raw_transaction_hex(&input.previous_output.txid.to_string())
                .await
                .map_err(|err| ExecutorError::Chain(ChainError::Rpc(err.to_string())))?;
            let prev_tx: Transaction = deserialize(&hex::decode(prev_tx_hex).map_err(|err| {
                ExecutorError::Chain(ChainError::DecodeFailed(format!(
                    "invalid bitcoind prev tx hex: {err}"
                )))
            })?)
            .map_err(|err| {
                ExecutorError::Chain(ChainError::DecodeFailed(format!(
                    "invalid bitcoind prev tx bytes: {err}"
                )))
            })?;
            let Some(prev_output) = prev_tx.output.get(input.previous_output.vout as usize) else {
                return Err(ExecutorError::Chain(ChainError::DecodeFailed(format!(
                    "prev tx {} missing vout {}",
                    input.previous_output.txid, input.previous_output.vout
                ))));
            };
            let script_matches = &prev_output.script_pubkey == wallet_script_pubkey;
            // This fallback matches only on script bytes because the raw
            // transaction format does not expose the Esplora address string.
            if script_matches {
                funding_inputs.push(CoverUtxo {
                    outpoint: input.previous_output,
                    value: prev_output.value.to_sat(),
                    script_pubkey: prev_output.script_pubkey.clone(),
                });
            }
        }

        Ok(funding_inputs)
    }
}

/// Extract wallet-owned funding inputs from an Esplora transaction response.
///
/// The observer accepts either an exact script match or an address match
/// because Electrs responses can omit one representation in edge cases, but the
/// runtime still needs a stable view of fee-cover inputs for RBF carry-forward.
fn extract_wallet_funding_inputs_from_esplora(
    tx: EsploraTx,
    wallet_script_pubkey: &ScriptBuf,
    wallet_address: &str,
) -> Result<Vec<CoverUtxo>, ExecutorError> {
    tx.vin
        .into_iter()
        .filter_map(|vin| {
            let prevout = vin.prevout.clone()?;
            Some((vin, prevout))
        })
        .filter_map(|(vin, prevout)| {
            // Esplora may omit either the decoded address or a clean script
            // representation in edge cases, so the observer accepts either.
            let script_matches = hex::decode(&prevout.scriptpubkey)
                .ok()
                .map(ScriptBuf::from_bytes)
                .map(|script_pubkey| script_pubkey == *wallet_script_pubkey)
                .unwrap_or(false);
            let address_matches = prevout
                .scriptpubkey_address
                .as_deref()
                .map(|address| address == wallet_address)
                .unwrap_or(false);
            (script_matches || address_matches).then_some((vin, prevout))
        })
        .map(|(vin, prevout)| {
            let input_txid = vin.txid.parse().map_err(|err| {
                ExecutorError::Chain(ChainError::DecodeFailed(format!(
                    "invalid txid in electrs vin: {err}"
                )))
            })?;
            let script_pubkey =
                ScriptBuf::from_bytes(hex::decode(&prevout.scriptpubkey).map_err(|err| {
                    ExecutorError::Chain(ChainError::DecodeFailed(format!(
                        "invalid prevout script hex: {err}"
                    )))
                })?);
            Ok(CoverUtxo {
                outpoint: OutPoint {
                    txid: input_txid,
                    vout: vin.vout,
                },
                value: prevout.value,
                script_pubkey,
            })
        })
        .collect()
}

/// Derive the persistence scope for one executor wallet on one Bitcoin network.
///
/// This isolates:
///
/// - one executor key from another on the same network,
/// - and the same executor key across mainnet/testnet/regtest.
pub fn wallet_scope(network: bitcoin::Network, wallet: &BitcoinWallet) -> String {
    format!(
        "bitcoin_{}:{}",
        network.to_core_arg(),
        wallet.x_only_pubkey()
    )
}

/// Strip runtime metadata and return just the logical wallet requests.
fn wallet_requests(requests: &[PendingWalletRequest]) -> Vec<WalletRequest> {
    requests
        .iter()
        .map(|request| request.request.clone())
        .collect()
}

/// Deterministic identifier for one logical request set.
///
/// Sorting ensures the same request bundle maps to the same planner-cache key
/// even if pending-order changes across ticks or restarts.
fn sorted_request_key(requests: &[PendingWalletRequest]) -> String {
    let mut keys = requests
        .iter()
        .map(|request| request.request.dedupe_key().to_string())
        .collect::<Vec<_>>();
    keys.sort();
    keys.join("|")
}

/// Group anchored requests by exact anchor identity.
///
/// Runner-side grouping mirrors planner-side grouping so cost preparation and
/// execution reason about the same anchor-bound bundles.
fn group_anchored_requests(
    requests: &[PendingWalletRequest],
) -> Vec<(ChainAnchor, Vec<PendingWalletRequest>)> {
    let mut groups = Vec::<(ChainAnchor, Vec<PendingWalletRequest>)>::new();
    for request in requests {
        let Some(anchor) = request.chain_anchor.clone() else {
            continue;
        };
        // Runner-side grouping mirrors planner grouping so anchored cost
        // preparation and action execution talk about the same request bundles.
        if let Some((_, grouped)) = groups.iter_mut().find(|(existing, _)| *existing == anchor) {
            grouped.push(request.clone());
        } else {
            groups.push((anchor, vec![request.clone()]));
        }
    }
    groups
}

fn remove_pending_request(runtime: &mut WalletRuntimeState, dedupe_key: &str) {
    // Submission removes the logical request from whichever pending bucket it
    // was in so the next tick doesn't try to plan it again.
    runtime
        .pending
        .free
        .retain(|request| request.request.dedupe_key() != dedupe_key);
    runtime
        .pending
        .anchored
        .retain(|request| request.request.dedupe_key() != dedupe_key);
}

fn ignored_cover_outpoints(
    runtime: &WalletRuntimeState,
    current_lineage_id: Option<LineageId>,
    active_anchor: Option<&ChainAnchor>,
) -> HashSet<OutPoint> {
    let mut ignored = HashSet::new();

    for lineage in runtime.live_lineages.values() {
        if Some(lineage.lineage_id) == current_lineage_id {
            continue;
        }
        // Cover inputs and chain anchors already reserved by other live
        // lineages must not be stolen by the build being priced/executed.
        for utxo in &lineage.cover_utxos {
            ignored.insert(utxo.outpoint);
        }
        if let Some(anchor) = &lineage.chain_anchor {
            ignored.insert(anchor.change_outpoint);
        }
    }

    for request in &runtime.pending.anchored {
        let Some(anchor) = request.chain_anchor.as_ref() else {
            continue;
        };
        if active_anchor == Some(anchor) {
            continue;
        }
        // Pending anchored requests reserve their anchor change output until a
        // batch for that same anchor is being considered.
        ignored.insert(anchor.change_outpoint);
    }

    ignored
}

/// Current chain height helper with a short bounded retry policy.
async fn current_height(electrs: &ElectrsClient) -> Result<u64, ExecutorError> {
    retry_with_delay(
        || electrs.get_block_height(),
        RETRY_ATTEMPTS,
        Duration::from_millis(RETRY_DELAY_MS),
        "failed to fetch bitcoin block height",
    )
    .await
}

/// Retry a `u64`-producing Bitcoin infrastructure call with fixed delay.
///
/// This is intentionally narrow: it smooths transient RPC/HTTP failures, not
/// long outages or stateful recovery.
async fn retry_with_delay<F, Fut>(
    mut fetch: F,
    attempts: usize,
    retry_delay: Duration,
    error_message: &str,
) -> Result<u64, ExecutorError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<u64, BitcoinClientError>>,
{
    let attempts = attempts.max(1);
    let mut last_error = None;

    for attempt in 0..attempts {
        match fetch().await {
            Ok(height) => return Ok(height),
            Err(err) => {
                // Height fetch failures are usually transient infrastructure
                // issues, so the runner gives them a short bounded retry loop.
                last_error = Some(err);
                if attempt + 1 < attempts {
                    tokio::time::sleep(retry_delay).await;
                }
            },
        }
    }

    let error = last_error.expect("retry loop must record at least one error");
    Err(ExecutorError::Chain(ChainError::Rpc(format!(
        "{error_message} after {attempts} attempts: {error}"
    ))))
}

/// Choose the market fee rate used for this tick's builds.
///
/// The runner currently prefers the "half hour" target and then clamps it into
/// the configured min/max safety rails.
async fn market_fee_rate(fee_estimator: &dyn FeeRateEstimator, config: &WalletConfig) -> f64 {
    match fee_estimator.get_fee_estimates().await {
        Ok(estimate) => {
            let selected = FeeLevel::HalfHour.from_estimate(&estimate);
            let clamped = selected.clamp(config.min_fee_rate, config.max_fee_rate);
            tracing::debug!(
                fastest_fee = estimate.fastest_fee,
                half_hour_fee = estimate.half_hour_fee,
                hour_fee = estimate.hour_fee,
                economy_fee = estimate.economy_fee,
                minimum_fee = estimate.minimum_fee,
                selected_fee = selected,
                clamped_fee = clamped,
                min_fee_rate = config.min_fee_rate,
                max_fee_rate = config.max_fee_rate,
                "bitcoin wallet runner selected market fee rate",
            );
            clamped
        },
        Err(error) => {
            // Falling back to the configured minimum keeps the runner making
            // progress even when fee providers are temporarily unavailable.
            tracing::warn!(
                min_fee_rate = config.min_fee_rate,
                max_fee_rate = config.max_fee_rate,
                %error,
                "bitcoin wallet runner failed to fetch fee estimate, falling back to min fee rate",
            );
            config.min_fee_rate
        },
    }
}

/// Load the fee context needed to price or build an RBF replacement.
///
/// Returns `None` when the current head is no longer known to bitcoind's
/// mempool view, meaning the runner should stop treating it as safely RBF-able.
async fn fetch_rbf_context(bitcoind: &BitcoindRpcClient, txid: Txid) -> Option<RbfFeeContext> {
    match bitcoind.get_rbf_tx_fee_info(&txid.to_string()).await {
        Ok(info) => Some(RbfFeeContext {
            previous_fee_rate: info.tx_fee_rate,
            previous_total_fee: info.total_fee,
            descendant_fee: info.descendant_fee,
        }),
        // If the previous head is already gone from bitcoind's mempool view,
        // the runner cannot safely price or build an RBF replacement.
        Err(err) if err.is_tx_not_found() => None,
        Err(_) => None,
    }
}

/// Normalize builder failures into the runner's chain-error surface.
fn map_builder_error(error: TxBuilderError) -> ExecutorError {
    ExecutorError::Chain(ChainError::Other(format!(
        "bitcoin wallet builder: {error}"
    )))
}

/// Convert a built transaction into the planner's cost metric.
///
/// - fresh/chained-new plans pay their full absolute fee
/// - replacements pay only the incremental fee above the previous head
fn planner_cost_from_build(
    build: &crate::infrastructure::chain::bitcoin::tx_builder::BuildTxReceipt,
    previous_total_fee: Option<u64>,
) -> Result<u64, TxBuilderError> {
    match previous_total_fee {
        Some(previous_total_fee) => {
            // Always submit standalone txs - ignore RBF fee validation
            let fee_delta = build.fee_paid_sats.saturating_sub(previous_total_fee);
            // let fee_delta = build
            //     .fee_paid_sats
            //     .checked_sub(previous_total_fee)
            //     .ok_or_else(|| {
            //         TxBuilderError::Client(format!(
            //             "replacement fee {} does not exceed previous fee {}",
            //             build.fee_paid_sats, previous_total_fee
            //         ))
            //     })?;
            tracing::debug!(
                fee_paid_sats = build.fee_paid_sats,
                previous_total_fee,
                fee_delta,
                "bitcoin wallet runner computed replacement planner cost",
            );
            Ok(fee_delta)
        },
        None => {
            tracing::debug!(
                fee_paid_sats = build.fee_paid_sats,
                "bitcoin wallet runner computed fresh planner cost",
            );
            Ok(build.fee_paid_sats)
        },
    }
}

/// True when bitcoind reports the raw tx is already known and should therefore
/// be treated as successfully submitted from the runner's perspective.
fn is_known_submission(message: &str) -> bool {
    message.contains("already in block chain")
        || message.contains("txn-already-known")
        || message.contains("txn-already-in-mempool")
}

/// True when bitcoind reports a hard submission failure the runner should not
/// classify as accepted/ambiguous.
fn is_rejected_submission(message: &str) -> bool {
    message.contains("insufficient fee")
        || message.contains("missing inputs")
        || message.contains("too-long-mempool-chain")
        || message.contains("non-BIP68-final")
        || message.contains("bad-txns")
        || message.contains("mandatory-script-verify-flag")
}

#[cfg(test)]
mod tests {
    #![allow(clippy::type_complexity)]

    use std::collections::VecDeque;
    use std::sync::Mutex as StdMutex;

    use async_trait::async_trait;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, Txid};
    use tokio::sync::mpsc;
    use uuid::Uuid;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::infrastructure::chain::bitcoin::clients::BitcoinClientError;
    use crate::infrastructure::chain::bitcoin::tx_builder::BuildTxReceipt;
    use crate::infrastructure::chain::bitcoin::wallet::{
        ConfirmedLineageHead, EnqueueWalletRequestResult, ObservedTxState, PersistedBatchReceipt,
        ReconciliationPersistencePlan, RestoredWalletState,
    };

    fn regtest_address(seed: u8) -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn send_request(key: &str) -> WalletRequest {
        WalletRequest::send(key, regtest_address(9), 25_000).expect("request")
    }

    fn test_bitcoind(server: &MockServer) -> Arc<BitcoindRpcClient> {
        Arc::new(BitcoindRpcClient::new(
            server.uri(),
            "admin1".into(),
            "123".into(),
        ))
    }

    fn dummy_tx() -> Transaction {
        Transaction {
            version: bitcoin::transaction::Version(2),
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: Vec::new(),
            output: Vec::new(),
        }
    }

    fn test_observer(
        electrs: Arc<ElectrsClient>,
        bitcoind: Arc<BitcoindRpcClient>,
    ) -> BitcoinWalletChainObserver {
        BitcoinWalletChainObserver::new(electrs, bitcoind, regtest_address(3))
    }

    struct QueueingStore {
        results: StdMutex<VecDeque<EnqueueWalletRequestResult>>,
    }

    impl QueueingStore {
        fn new(results: Vec<EnqueueWalletRequestResult>) -> Self {
            Self {
                results: StdMutex::new(results.into()),
            }
        }
    }

    #[async_trait]
    impl WalletStore for QueueingStore {
        async fn enqueue(
            &self,
            _scope: &str,
            _request: &WalletRequest,
        ) -> Result<EnqueueWalletRequestResult, ExecutorError> {
            self.results
                .lock()
                .expect("lock")
                .pop_front()
                .ok_or_else(|| ExecutorError::Domain("missing enqueue result".into()))
        }

        async fn restore(&self, _scope: &str) -> Result<RestoredWalletState, ExecutorError> {
            Ok(RestoredWalletState::default())
        }

        async fn resolve_pending(
            &self,
            _scope: &str,
            _dedupe_key: &str,
        ) -> Result<ResolvePendingWalletRequestResult, ExecutorError> {
            Ok(ResolvePendingWalletRequestResult::CancelledPending)
        }

        async fn persist_broadcast(
            &self,
            _scope: &str,
            _plan: &BroadcastPersistencePlan,
        ) -> Result<PersistedBatchReceipt, ExecutorError> {
            unreachable!("not used in runner handle tests")
        }

        async fn revert_broadcast(
            &self,
            _scope: &str,
            _receipt: &PersistedBatchReceipt,
        ) -> Result<(), ExecutorError> {
            unreachable!("not used in runner handle tests")
        }

        async fn persist_confirmation(
            &self,
            _scope: &str,
            _plan: &ConfirmationPersistencePlan,
        ) -> Result<(), ExecutorError> {
            unreachable!("not used in runner handle tests")
        }

        async fn list_confirmed_lineage_heads(
            &self,
            _scope: &str,
            _limit: usize,
        ) -> Result<Vec<ConfirmedLineageHead>, ExecutorError> {
            Ok(Vec::new())
        }

        async fn has_submitted_tx(&self, _scope: &str, _txid: Txid) -> Result<bool, ExecutorError> {
            Ok(false)
        }

        async fn persist_reconciliation(
            &self,
            _scope: &str,
            _plan: &ReconciliationPersistencePlan,
        ) -> Result<(), ExecutorError> {
            unreachable!("not used in runner handle tests")
        }
    }

    fn test_handle(
        store: Arc<dyn WalletStore>,
    ) -> (
        BitcoinWalletRunnerHandle,
        mpsc::Receiver<RunnerCommand>,
        Arc<Mutex<HashMap<String, Vec<oneshot::Sender<Txid>>>>>,
    ) {
        let (request_tx, request_rx) = mpsc::channel(4);
        let waiters = Arc::new(Mutex::new(HashMap::new()));

        (
            BitcoinWalletRunnerHandle {
                scope: format!("bitcoin_regtest:{}", Uuid::new_v4()),
                store,
                request_tx,
                waiters: Arc::clone(&waiters),
            },
            request_rx,
            waiters,
        )
    }

    #[tokio::test]
    async fn submit_enqueues_without_registering_waiter() {
        let store = Arc::new(QueueingStore::new(vec![
            EnqueueWalletRequestResult::EnqueuedPending,
        ]));
        let (handle, mut request_rx, waiters) = test_handle(store);
        let request = send_request("submit-only");

        handle.submit(request.clone()).await.expect("submit");

        let queued = request_rx.recv().await.expect("queued request");
        match queued {
            RunnerCommand::Submit(queued) => assert_eq!(*queued, request),
            RunnerCommand::ResolvePending { .. } => panic!("expected submit command"),
        }
        assert!(waiters.lock().await.is_empty());
    }

    #[tokio::test]
    async fn submit_and_wait_registers_waiter_until_submission_arrives() {
        let store = Arc::new(QueueingStore::new(vec![
            EnqueueWalletRequestResult::EnqueuedPending,
        ]));
        let (handle, mut request_rx, waiters) = test_handle(store);
        let request = send_request("wait-submit");
        let expected_txid = Txid::from_byte_array([3u8; 32]);

        let waiter_task = tokio::spawn({
            let handle = handle.clone();
            let request = request.clone();
            async move { handle.submit_and_wait(request).await }
        });

        let queued = request_rx.recv().await.expect("queued request");
        match queued {
            RunnerCommand::Submit(queued) => assert_eq!(*queued, request),
            RunnerCommand::ResolvePending { .. } => panic!("expected submit command"),
        }

        {
            let mut registered = waiters.lock().await;
            let senders = registered
                .remove(request.dedupe_key())
                .expect("registered waiter");
            assert_eq!(senders.len(), 1);
            senders
                .into_iter()
                .next()
                .expect("sender")
                .send(expected_txid)
                .expect("send txid");
        }

        assert_eq!(
            waiter_task.await.expect("join").expect("txid"),
            expected_txid
        );
    }

    #[tokio::test]
    async fn submit_and_wait_returns_immediate_inflight_txid() {
        let lineage_id = LineageId::from_uuid(Uuid::new_v4());
        let txid = Txid::from_byte_array([4u8; 32]);
        let store = Arc::new(QueueingStore::new(vec![
            EnqueueWalletRequestResult::AlreadyInflight { lineage_id, txid },
        ]));
        let (handle, mut request_rx, waiters) = test_handle(store);

        let observed = handle
            .submit_and_wait(send_request("inflight"))
            .await
            .expect("submit_and_wait");

        assert_eq!(observed, txid);
        assert!(request_rx.try_recv().is_err());
        assert!(waiters.lock().await.is_empty());
    }

    #[tokio::test]
    async fn observer_treats_unconfirmed_electrs_tx_without_mempool_entry_as_missing() {
        let txid = Txid::from_byte_array([8u8; 32]);
        let electrs = MockServer::start().await;
        let bitcoind = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/tx/{txid}/status")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "confirmed": false,
                "block_height": null,
                "block_hash": null,
                "block_time": null
            })))
            .mount(&electrs)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": null,
                "error": {"code": -5, "message": "Transaction not found"}
            })))
            .mount(&bitcoind)
            .await;

        let observer = test_observer(
            Arc::new(ElectrsClient::new(electrs.uri())),
            test_bitcoind(&bitcoind),
        );

        assert_eq!(
            observer.observe_tx(txid).await.expect("observe"),
            ObservedTxState::Missing
        );
    }

    #[tokio::test]
    async fn observer_uses_bitcoind_mempool_when_electrs_cannot_find_tx() {
        let txid = Txid::from_byte_array([9u8; 32]);
        let electrs = MockServer::start().await;
        let bitcoind = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/tx/{txid}/status")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&electrs)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "result": {
                    "vsize": 111,
                    "weight": 444,
                    "fees": {
                        "base": 0.00001,
                        "modified": 0.00001,
                        "ancestor": 0.00001,
                        "descendant": 0.00001
                    },
                    "depends": [],
                    "spentby": []
                },
                "error": null
            })))
            .mount(&bitcoind)
            .await;

        let observer = test_observer(
            Arc::new(ElectrsClient::new(electrs.uri())),
            test_bitcoind(&bitcoind),
        );

        assert_eq!(
            observer.observe_tx(txid).await.expect("observe"),
            ObservedTxState::InMempool
        );
    }

    #[tokio::test]
    async fn current_height_retries_until_fetch_succeeds() {
        let attempts = Arc::new(StdMutex::new(0usize));

        let height = retry_with_delay(
            {
                let attempts = Arc::clone(&attempts);
                move || {
                    let attempts = Arc::clone(&attempts);
                    async move {
                        let mut count = attempts.lock().expect("lock");
                        *count += 1;
                        if *count < 3 {
                            Err(BitcoinClientError::Rpc("transient".into()))
                        } else {
                            Ok(321_654)
                        }
                    }
                }
            },
            3,
            Duration::ZERO,
            "failed to fetch bitcoin block height",
        )
        .await
        .expect("height");

        assert_eq!(height, 321_654);
        assert_eq!(*attempts.lock().expect("lock"), 3);
    }

    #[tokio::test]
    async fn current_height_returns_error_after_retry_exhaustion() {
        let attempts = Arc::new(StdMutex::new(0usize));

        let error = retry_with_delay(
            {
                let attempts = Arc::clone(&attempts);
                move || {
                    let attempts = Arc::clone(&attempts);
                    async move {
                        *attempts.lock().expect("lock") += 1;
                        Err(BitcoinClientError::Rpc("electrs down".into()))
                    }
                }
            },
            3,
            Duration::ZERO,
            "failed to fetch bitcoin block height",
        )
        .await
        .expect_err("height fetch should fail");

        assert!(error.to_string().contains("after 3 attempts"));
        assert!(error.to_string().contains("electrs down"));
        assert_eq!(*attempts.lock().expect("lock"), 3);
    }

    #[test]
    fn planner_cost_uses_full_fee_for_fresh_builds() {
        let build = BuildTxReceipt {
            tx: dummy_tx(),
            fee_paid_sats: 1_234,
            cover_utxos: Vec::new(),
            lineage_prevout: None,
        };

        assert_eq!(
            planner_cost_from_build(&build, None).expect("planner cost"),
            1_234
        );
    }

    #[test]
    fn planner_cost_uses_fee_delta_for_replacements() {
        let build = BuildTxReceipt {
            tx: dummy_tx(),
            fee_paid_sats: 2_500,
            cover_utxos: Vec::new(),
            lineage_prevout: None,
        };

        assert_eq!(
            planner_cost_from_build(&build, Some(1_700)).expect("planner cost"),
            800
        );
    }
}
