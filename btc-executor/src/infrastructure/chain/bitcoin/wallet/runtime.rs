//! In-memory state machine for the Bitcoin wallet runner.
//!
//! The runtime owns the mutable view of pending requests, live lineages, and
//! confirmation/reconciliation side effects between storage calls. It is
//! intentionally pure where possible so the runner can recover the same state
//! from persistence after a restart.
//!
//! State outline:
//!
//! ```text
//! pending.free
//!   requests with no chain-anchor dependency
//!
//! pending.anchored
//!   requests that must spend from a specific confirmed change output
//!
//! live_lineages[lineage_id]
//!   current mempool head
//!   + all prior txids in that lineage
//!   + request membership history
//!   + carried-forward fee cover inputs
//! ```
//!
//! Recovery outline:
//!
//! ```text
//! restore rows from store
//!   -> rebuild pending + inflight snapshots
//!   -> probe current head tx state
//!   -> confirmed winner => queue confirmation handling
//!   -> mempool winner   => keep lineage live
//!   -> missing head     => probe older siblings and reconcile later
//! ```
//!
//! Example lineage snapshot:
//!
//! ```text
//! lineage L:
//!   head_txid   = tx_3
//!   all_txids   = [tx_1, tx_2, tx_3]
//!   requests    =
//!     req_a -> txid_history [tx_1, tx_2, tx_3]
//!     req_b -> txid_history [tx_1, tx_2]
//!     req_c -> txid_history [tx_3]
//!
//! meaning:
//!   - req_b was present before the latest replacement but not in tx_3
//!   - if tx_3 disappears and tx_2 is the surviving sibling, req_b survives
//!   - if tx_3 confirms, req_b becomes orphaned work and may be requeued
//! ```

use std::collections::HashMap;

use crate::errors::ExecutorError;
use crate::timestamp::Timestamp;
use crate::infrastructure::chain::bitcoin::clients::EsploraTx;
use async_trait::async_trait;
use bitcoin::Txid;

use super::{
    plan_wallet_batches, BroadcastPersistencePlan, ChainAnchor, CoverUtxo, LineageId,
    LiveLineageRequest, LiveLineageSnapshot, PendingWalletRequest, PersistedBatchReceipt,
    PlannedBatchAction, PlannerCostEvaluator, RestoredWalletState, WalletConfig, WalletRequest,
    WalletStore,
};

/// Live lineage tracked in memory while its current head is still pending.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveLineage {
    /// Stable lineage identity across fresh submission and RBF replacements.
    pub lineage_id: LineageId,
    /// Current head txid that the runtime expects to see in mempool or chain.
    pub head_txid: Txid,
    /// Full tx history for the lineage, oldest to newest.
    pub all_txids: Vec<Txid>,
    /// Requests still represented in the lineage.
    pub requests: Vec<LiveLineageRequest>,
    /// Wallet-controlled fee inputs currently attached to the head.
    pub cover_utxos: Vec<CoverUtxo>,
    /// Optional confirmed anchor this lineage is chained to.
    pub chain_anchor: Option<ChainAnchor>,
}

impl From<LiveLineageSnapshot> for LiveLineage {
    fn from(snapshot: LiveLineageSnapshot) -> Self {
        Self {
            lineage_id: snapshot.lineage_id,
            head_txid: snapshot.head_txid,
            all_txids: snapshot.all_txids,
            requests: snapshot.requests,
            cover_utxos: snapshot.cover_utxos,
            chain_anchor: snapshot.chain_anchor,
        }
    }
}

impl LiveLineage {
    /// Clone just the underlying wallet requests, discarding runtime metadata.
    pub fn wallet_requests(&self) -> Vec<WalletRequest> {
        self.requests
            .iter()
            .map(|request| request.request.clone())
            .collect()
    }

    /// Derive the wallet-owned prevout that should be force-included when
    /// building descendants chained to the same confirmed anchor.
    pub fn derived_lineage_prevout(&self) -> Option<CoverUtxo> {
        self.chain_anchor.as_ref().map(|anchor| CoverUtxo {
            outpoint: anchor.change_outpoint,
            value: anchor.change_value,
            script_pubkey: anchor.change_script_pubkey.clone(),
        })
    }
}

/// Pending requests partitioned by whether they must respect a chain anchor.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PendingState {
    pub free: Vec<PendingWalletRequest>,
    pub anchored: Vec<PendingWalletRequest>,
}

impl PendingState {
    /// Partition pending requests into free and anchor-bound buckets.
    pub fn from_requests(requests: Vec<PendingWalletRequest>) -> Self {
        let mut free = Vec::new();
        let mut anchored = Vec::new();

        for request in requests {
            if request.chain_anchor.is_some() {
                anchored.push(request);
            } else {
                free.push(request);
            }
        }

        Self { free, anchored }
    }

    /// True when there is no pending work of any kind.
    pub fn is_empty(&self) -> bool {
        self.free.is_empty() && self.anchored.is_empty()
    }
}

/// Complete in-memory state owned by the wallet runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletRuntimeState {
    pub live_lineages: HashMap<LineageId, LiveLineage>,
    pub pending: PendingState,
    pub missing_observations: HashMap<LineageId, u32>,
    pub current_height: u64,
}

/// Executor used by [`WalletRuntimeState::tick`] to hand planned actions back
/// to the runner without coupling the state machine to async I/O.
pub trait PlannedActionExecutor {
    fn execute(&mut self, action: PlannedBatchAction);
}

/// Broadcast outcome reported by the chain broadcaster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroadcastAcceptance {
    Accepted,
    Rejected,
    Ambiguous,
}

/// Result of persisting and then attempting to broadcast a batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroadcastSubmissionResult {
    Accepted(PersistedBatchReceipt),
    Rejected(PersistedBatchReceipt),
    Ambiguous(PersistedBatchReceipt),
}

/// Async broadcaster used by the runner after persistence succeeds.
#[async_trait]
pub trait WalletBatchBroadcaster: Send + Sync {
    async fn broadcast(
        &self,
        scope: &str,
        receipt: &PersistedBatchReceipt,
    ) -> Result<BroadcastAcceptance, ExecutorError>;
}

/// Chain-observation state for a tx currently tracked by the runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ObservedTxState {
    Confirmed,
    InMempool,
    Missing,
}

/// Observer contract used by recovery and live polling.
#[async_trait]
pub trait WalletTxObserver: Send + Sync {
    async fn observe_tx(&self, txid: Txid) -> Result<ObservedTxState, ExecutorError>;

    async fn load_wallet_funding_inputs(&self, txid: Txid) -> Result<Vec<CoverUtxo>, ExecutorError>;

    async fn load_tx(&self, txid: Txid) -> Result<EsploraTx, ExecutorError>;
}

#[async_trait]
impl<T> super::reconcile::WalletReconciliationObserver for T
where
    T: WalletTxObserver + Send + Sync,
{
    async fn observe_tx(&self, txid: Txid) -> Result<ObservedTxState, ExecutorError> {
        WalletTxObserver::observe_tx(self, txid).await
    }

    async fn load_tx(&self, txid: Txid) -> Result<EsploraTx, ExecutorError> {
        WalletTxObserver::load_tx(self, txid).await
    }
}

/// Runtime reconstructed from persistence plus chain observations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredWalletRuntime {
    pub runtime: WalletRuntimeState,
    pub confirmations_pending: Vec<PendingLineageConfirmation>,
}

/// Lineage that confirmed while the runner was offline or polling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLineageConfirmation {
    pub lineage: LiveLineage,
    pub confirmed_txid: Txid,
}

/// Output of one live-lineage observation sweep.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveObservationOutcome {
    pub confirmations_pending: Vec<PendingLineageConfirmation>,
}

/// Persist a broadcast plan first, then submit it to the network.
///
/// This ordering is deliberate: if the process crashes after the network accepts
/// the tx, the persisted receipt still allows recovery and replay-safe repair.
///
/// In other words, persistence is the durable "we may have submitted this"
/// boundary, and broadcasting is only attempted after that boundary exists.
pub async fn persist_and_submit_broadcast<S, B>(
    store: &S,
    broadcaster: &B,
    scope: &str,
    plan: &BroadcastPersistencePlan,
) -> Result<BroadcastSubmissionResult, ExecutorError>
where
    S: WalletStore + ?Sized,
    B: WalletBatchBroadcaster + ?Sized,
{
    // Persist first so an accepted network submission can always be recovered
    // later, even if the process dies immediately after broadcast.
    let receipt = store.persist_broadcast(scope, plan).await?;

    match broadcaster.broadcast(scope, &receipt).await? {
        BroadcastAcceptance::Accepted => Ok(BroadcastSubmissionResult::Accepted(receipt)),
        BroadcastAcceptance::Ambiguous => Ok(BroadcastSubmissionResult::Ambiguous(receipt)),
        BroadcastAcceptance::Rejected => {
            // Rejected submissions must roll persistence back so the runtime
            // does not think the lineage head ever became inflight.
            store.revert_broadcast(scope, &receipt).await?;
            Ok(BroadcastSubmissionResult::Rejected(receipt))
        },
    }
}

/// Recover in-memory runtime state from persistence and current chain status.
///
/// This is used on startup before the runner begins accepting new work. It
/// probes each inflight lineage head so confirmed winners can be processed
/// immediately and mempool siblings can be adopted without losing request
/// history.
pub async fn recover_wallet_runtime<S, O>(
    store: &S,
    observer: &O,
    scope: &str,
    current_height: u64,
) -> Result<RecoveredWalletRuntime, ExecutorError>
where
    S: WalletStore + ?Sized,
    O: WalletTxObserver + ?Sized,
{
    let restored = store.restore(scope).await?;
    let mut runtime = WalletRuntimeState {
        live_lineages: HashMap::new(),
        pending: PendingState::from_requests(restored.pending),
        missing_observations: HashMap::new(),
        current_height,
    };
    let mut confirmations_pending = Vec::new();

    for snapshot in restored.inflight {
        let mut lineage = LiveLineage::from(snapshot);
        match observer.observe_tx(lineage.head_txid).await {
            Ok(ObservedTxState::Confirmed) => {
                // A confirmed head discovered during startup should be handled
                // through the normal confirmation path, not left as live state.
                lineage.cover_utxos = load_funding_inputs_for_txid(
                    observer,
                    lineage.head_txid,
                    lineage.chain_anchor.as_ref(),
                )
                .await?;
                confirmations_pending.push(PendingLineageConfirmation {
                    confirmed_txid: lineage.head_txid,
                    lineage,
                });
            },
            Ok(ObservedTxState::InMempool) => {
                // Keep the lineage live and refresh fee-cover inputs from the
                // chain observer so future RBF attempts start from the right set.
                lineage.cover_utxos = load_funding_inputs_for_txid(
                    observer,
                    lineage.head_txid,
                    lineage.chain_anchor.as_ref(),
                )
                .await?;
                runtime.live_lineages.insert(lineage.lineage_id, lineage);
            },
            Ok(ObservedTxState::Missing) => {
                match probe_missing_head_resolution(observer, &lineage).await {
                    MissingHeadResolution::ConfirmedSibling(confirmed_txid) => {
                        // A prior sibling already confirmed; queue the lineage
                        // for the standard confirmation/replay handling path.
                        confirmations_pending.push(PendingLineageConfirmation {
                            lineage,
                            confirmed_txid,
                        });
                    },
                    MissingHeadResolution::MempoolSibling {
                        adopted_head_txid,
                        cover_utxos,
                    } => {
                        // An older sibling still survives in mempool, so adopt
                        // it as the live head instead of dropping the lineage.
                        lineage.head_txid = adopted_head_txid;
                        lineage.cover_utxos = cover_utxos;
                        runtime.live_lineages.insert(lineage.lineage_id, lineage);
                    },
                    MissingHeadResolution::NoLiveSibling | MissingHeadResolution::ObserverError => {
                        // Leave the lineage live; the runner's missing-head
                        // threshold logic will decide whether to reconcile it.
                        runtime.live_lineages.insert(lineage.lineage_id, lineage);
                    },
                }
            },
            Err(_) => {
                // Observation failures should not discard runtime state.
                runtime.live_lineages.insert(lineage.lineage_id, lineage);
            },
        }
    }

    Ok(RecoveredWalletRuntime {
        runtime,
        confirmations_pending,
    })
}

/// Result of a synchronous runtime tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TickOutcome {
    pub executed_actions: Vec<PlannedBatchAction>,
    pub dropped_requests: Vec<PendingWalletRequest>,
    pub expired_anchor_requests: Vec<PendingWalletRequest>,
}

impl WalletRuntimeState {
    /// Build runtime state directly from restored persistence snapshots.
    pub fn from_restored(restored: RestoredWalletState, current_height: u64) -> Self {
        let live_lineages = restored
            .inflight
            .into_iter()
            .map(LiveLineage::from)
            .map(|lineage| (lineage.lineage_id, lineage))
            .collect();

        Self {
            live_lineages,
            pending: PendingState::from_requests(restored.pending),
            missing_observations: HashMap::new(),
            current_height,
        }
    }

    /// Look up a live lineage by id.
    pub fn live_lineage(&self, lineage_id: &LineageId) -> Option<&LiveLineage> {
        self.live_lineages.get(lineage_id)
    }

    /// Increment and return the number of consecutive times a lineage head has
    /// been missing from both chain observers.
    pub fn record_missing_observation(&mut self, lineage_id: LineageId) -> u32 {
        let observations = self.missing_observations.entry(lineage_id).or_default();
        *observations += 1;
        *observations
    }

    /// Return the current missing-observation count for a lineage.
    pub fn missing_observations(&self, lineage_id: &LineageId) -> u32 {
        self.missing_observations
            .get(lineage_id)
            .copied()
            .unwrap_or_default()
    }

    /// Clear any missing-observation counter for a lineage.
    pub fn reset_missing_observations(&mut self, lineage_id: &LineageId) {
        self.missing_observations.remove(lineage_id);
    }

    /// Release all pending requests currently pinned to `chain_anchor`.
    pub fn release_chain_anchor_group(
        &mut self,
        chain_anchor: &ChainAnchor,
    ) -> Vec<PendingWalletRequest> {
        let mut released = Vec::new();
        let mut still_anchored = Vec::new();

        for request in self.pending.anchored.drain(..) {
            if request.chain_anchor.as_ref() == Some(chain_anchor) {
                // Releasing the group removes the special chaining constraint
                // but keeps the logical request alive as ordinary free work.
                let mut free_request = request;
                free_request.chain_anchor = None;
                released.push(free_request.clone());
                self.pending.free.push(free_request);
            } else {
                still_anchored.push(request);
            }
        }

        self.pending.anchored = still_anchored;
        released
    }

    /// Poll the currently live lineages and collect confirmations that require
    /// persistence updates by the runner.
    pub async fn observe_live_lineages<O>(&mut self, observer: &O) -> LiveObservationOutcome
    where
        O: WalletTxObserver,
    {
        let mut outcome = LiveObservationOutcome::default();
        let lineage_ids = self.live_lineages.keys().copied().collect::<Vec<_>>();

        for lineage_id in lineage_ids {
            let Some(lineage) = self.live_lineages.get(&lineage_id).cloned() else {
                continue;
            };

            match observer.observe_tx(lineage.head_txid).await {
                Ok(ObservedTxState::Confirmed) => {
                    // Once confirmed, remove the live lineage and hand it back
                    // to the runner for durable confirmation persistence.
                    self.live_lineages.remove(&lineage_id);
                    self.reset_missing_observations(&lineage_id);
                    outcome
                        .confirmations_pending
                        .push(PendingLineageConfirmation {
                            confirmed_txid: lineage.head_txid,
                            lineage,
                        });
                },
                Ok(ObservedTxState::InMempool) => {
                    // Healthy mempool observation resets the missing counter.
                    self.reset_missing_observations(&lineage_id);
                },
                Ok(ObservedTxState::Missing) => {
                    match probe_missing_head_resolution(observer, &lineage).await {
                        MissingHeadResolution::ConfirmedSibling(confirmed_txid) => {
                            // A sibling won after the current head disappeared.
                            self.live_lineages.remove(&lineage_id);
                            self.reset_missing_observations(&lineage_id);
                            outcome
                                .confirmations_pending
                                .push(PendingLineageConfirmation {
                                    lineage,
                                    confirmed_txid,
                                });
                        },
                        MissingHeadResolution::MempoolSibling {
                            adopted_head_txid,
                            cover_utxos,
                        } => {
                            // Swap the live head to the surviving sibling so
                            // future observations follow the right txid.
                            if let Some(entry) = self.live_lineages.get_mut(&lineage_id) {
                                entry.head_txid = adopted_head_txid;
                                entry.cover_utxos = cover_utxos;
                            }
                            self.reset_missing_observations(&lineage_id);
                        },
                        MissingHeadResolution::NoLiveSibling => {
                            // Missing once may just be observer lag; count
                            // consecutive misses before forcing reconciliation.
                            self.record_missing_observation(lineage_id);
                        },
                        MissingHeadResolution::ObserverError => {
                            // Leave counters unchanged on observer failure.
                        },
                    }
                },
                Err(_) => {
                    // Observation errors are non-fatal; keep the lineage live.
                },
            }
        }

        outcome
    }

    /// Apply one synchronous state-machine tick.
    ///
    /// Callers supply `new_requests`, the current block height, a cost
    /// evaluator, and a lightweight executor for planned actions. The runner
    /// performs the async work for those actions after the state transition.
    ///
    /// The order is intentional:
    ///
    /// 1. add newly queued work
    /// 2. expire no-longer-needed anchors
    /// 3. drop stale pending requests
    /// 4. plan actions against the cleaned state
    pub fn tick<E, X>(
        &mut self,
        new_requests: Vec<PendingWalletRequest>,
        current_height: u64,
        now: Timestamp,
        config: &WalletConfig,
        evaluator: &E,
        executor: &mut X,
    ) -> TickOutcome
    where
        E: PlannerCostEvaluator,
        X: PlannedActionExecutor,
    {
        self.current_height = current_height;
        for request in new_requests {
            // New requests become pending first; planning happens after anchor
            // expiry and stale-request cleanup.
            self.push_pending(request);
        }

        // A confirmed anchor eventually becomes "old enough" that downstream
        // requests no longer need to stay pinned to that exact change output.
        let expired_anchor_requests = self.expire_chain_anchors(config.chain_anchor_confirmations);
        // Pending requests that sit too long without ever making it into a batch
        // are dropped so the runner does not retry them forever.
        let dropped_requests = self.drop_stale_pending(now, config.max_pending_ttl_secs);

        // Planning is purely synchronous; the runner executes the chosen actions
        // asynchronously after this state transition returns.
        let executed_actions = plan_wallet_batches(self, config, evaluator);
        for action in executed_actions.iter().cloned() {
            executor.execute(action);
        }

        TickOutcome {
            executed_actions,
            dropped_requests,
            expired_anchor_requests,
        }
    }

    fn push_pending(&mut self, request: PendingWalletRequest) {
        // The pending split is purely about planning constraints; the request
        // payload itself is unchanged.
        if request.chain_anchor.is_some() {
            self.pending.anchored.push(request);
        } else {
            self.pending.free.push(request);
        }
    }

    fn expire_chain_anchors(&mut self, required_confirmations: u64) -> Vec<PendingWalletRequest> {
        let mut expired = Vec::new();
        let mut still_anchored = Vec::new();

        for request in self.pending.anchored.drain(..) {
            let Some(anchor) = request.chain_anchor.as_ref() else {
                self.pending.free.push(request);
                continue;
            };

            // Once the anchor is old enough, it no longer needs special
            // treatment and can compete with normal free requests.
            let confirmations = self
                .current_height
                .saturating_sub(anchor.confirmed_height)
                .saturating_add(1);
            if confirmations >= required_confirmations {
                // After enough confirmations the exact anchor no longer needs
                // to be preserved, so the request can compete with normal work.
                let mut free_request = request.clone();
                free_request.chain_anchor = None;
                expired.push(free_request.clone());
                self.pending.free.push(free_request);
            } else {
                still_anchored.push(request);
            }
        }

        self.pending.anchored = still_anchored;
        expired
    }

    fn drop_stale_pending(
        &mut self,
        now: Timestamp,
        max_pending_ttl_secs: u64,
    ) -> Vec<PendingWalletRequest> {
        let ttl = time::Duration::seconds(max_pending_ttl_secs as i64);
        let mut dropped = Vec::new();

        self.pending.free.retain(|request| {
            // Free and anchored requests use the same TTL policy; only the
            // storage-side outcome differs later when the runner records drops.
            let stale = request.created_at.0 + ttl < now.0;
            if stale {
                dropped.push(request.clone());
            }
            !stale
        });

        self.pending.anchored.retain(|request| {
            let stale = request.created_at.0 + ttl < now.0;
            if stale {
                dropped.push(request.clone());
            }
            !stale
        });

        dropped
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum MissingHeadResolution {
    ConfirmedSibling(Txid),
    MempoolSibling {
        adopted_head_txid: Txid,
        cover_utxos: Vec<CoverUtxo>,
    },
    NoLiveSibling,
    ObserverError,
}

async fn probe_missing_head_resolution<O>(
    observer: &O,
    lineage: &LiveLineage,
) -> MissingHeadResolution
where
    O: WalletTxObserver + ?Sized,
{
    let mut saw_error = false;

    for sibling_txid in lineage
        .all_txids
        .iter()
        .rev()
        .copied()
        .filter(|txid| *txid != lineage.head_txid)
    {
        // Search newest-to-oldest because the latest surviving sibling should
        // define request membership if the current head vanished.
        match observer.observe_tx(sibling_txid).await {
            Ok(ObservedTxState::Confirmed) => {
                return MissingHeadResolution::ConfirmedSibling(sibling_txid);
            },
            Ok(ObservedTxState::InMempool) => {
                match load_funding_inputs_for_txid(
                    observer,
                    sibling_txid,
                    lineage.chain_anchor.as_ref(),
                )
                .await
                {
                    Ok(cover_utxos) => {
                        // Keep the surviving sibling as the new live head and
                        // refresh its wallet-owned funding inputs.
                        return MissingHeadResolution::MempoolSibling {
                            adopted_head_txid: sibling_txid,
                            cover_utxos,
                        };
                    },
                    Err(_) => {
                        saw_error = true;
                        break;
                    },
                }
            },
            Ok(ObservedTxState::Missing) => {},
            Err(_) => {
                saw_error = true;
            },
        }
    }

    if saw_error {
        MissingHeadResolution::ObserverError
    } else {
        MissingHeadResolution::NoLiveSibling
    }
}

async fn load_funding_inputs_for_txid<O>(
    observer: &O,
    txid: Txid,
    chain_anchor: Option<&ChainAnchor>,
) -> Result<Vec<CoverUtxo>, ExecutorError>
where
    O: WalletTxObserver + ?Sized,
{
    let mut funding_inputs = observer.load_wallet_funding_inputs(txid).await?;
    if let Some(anchor) = chain_anchor {
        // The confirmed anchor is tracked separately in runtime state and should
        // not be mixed into generic fee-cover inputs. Chained descendants add
        // it back explicitly as the lineage prevout.
        funding_inputs.retain(|utxo| utxo.outpoint != anchor.change_outpoint);
    }
    Ok(funding_inputs)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use crate::errors::ExecutorError;
    use crate::timestamp::Timestamp;
    use crate::infrastructure::chain::bitcoin::wallet::store::ConfirmedLineageHead;
    use async_trait::async_trait;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, OutPoint, ScriptBuf, Txid};
    use std::sync::Mutex;
    use time::OffsetDateTime;

    use super::{
        persist_and_submit_broadcast, recover_wallet_runtime, BroadcastAcceptance,
        BroadcastSubmissionResult, LiveLineage, LiveObservationOutcome, ObservedTxState,
        PendingLineageConfirmation, PendingState, PlannedActionExecutor,
        WalletBatchBroadcaster, WalletRuntimeState, WalletTxObserver,
    };
    use crate::infrastructure::chain::bitcoin::clients::{EsploraTx, TxStatus, TxVout};
    use crate::infrastructure::chain::bitcoin::wallet::{
        reconcile_missing_lineage, BroadcastPersistenceKind, BroadcastPersistencePlan,
        ChainAnchor, ConfirmationPersistencePlan, CoverUtxo, LineageId, LiveLineageRequest,
        LiveLineageSnapshot, PendingWalletRequest, PersistedBatchReceipt,
        PersistedWalletRequestSnapshot, PlannedBatchAction, PlannerCostEvaluator,
        ReconciledLineageState, ReconciliationPersistenceKind, ReconciliationPersistencePlan,
        ReconciliationResult, RestoredWalletState, WalletConfig, WalletRequest,
        WalletRequestLifecycleStatus, WalletStore,
    };
    use std::collections::HashMap;

    fn regtest_address(seed: u8) -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn sample_request(key: &str, seed: u8) -> WalletRequest {
        WalletRequest::send(key, regtest_address(seed), 12_000).expect("request")
    }

    fn sample_anchor(seed: u8, height: u64) -> ChainAnchor {
        let txid = Txid::from_byte_array([seed; 32]);
        ChainAnchor {
            confirmed_txid: txid,
            change_outpoint: OutPoint { txid, vout: 1 },
            change_value: 22_000,
            change_script_pubkey: ScriptBuf::new(),
            confirmed_height: height,
        }
    }

    fn tx_output(script_pubkey: &ScriptBuf, value: u64) -> TxVout {
        TxVout {
            scriptpubkey: hex::encode(script_pubkey.as_bytes()),
            scriptpubkey_address: None,
            value,
        }
    }

    fn confirmed_tx(txid: Txid, outputs: Vec<TxVout>, block_height: u64) -> EsploraTx {
        EsploraTx {
            txid: txid.to_string(),
            version: 2,
            locktime: 0,
            vin: Vec::new(),
            vout: outputs,
            size: 200,
            weight: 800,
            fee: 500,
            status: TxStatus {
                confirmed: true,
                block_height: Some(block_height),
                block_hash: None,
                block_time: None,
            },
        }
    }

    fn timestamp(secs: i64) -> Timestamp {
        Timestamp(OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(secs))
    }

    fn live_request(key: &str, seed: u8, txid_history: Vec<Txid>) -> LiveLineageRequest {
        LiveLineageRequest {
            request: sample_request(key, seed),
            txid_history,
            created_at: Timestamp::default(),
        }
    }

    fn cover_utxo(seed: u8, vout: u32, value: u64) -> CoverUtxo {
        CoverUtxo {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([seed; 32]),
                vout,
            },
            value,
            script_pubkey: ScriptBuf::new(),
        }
    }

    fn sample_receipt(lineage_id: LineageId, txid: Txid) -> PersistedBatchReceipt {
        PersistedBatchReceipt {
            lineage_id,
            txid,
            raw_tx_hex: "deadbeef".to_string(),
            snapshots: vec![PersistedWalletRequestSnapshot {
                dedupe_key: "req-1".to_string(),
                status: WalletRequestLifecycleStatus::Pending,
                lineage_id: None,
                batch_txid: None,
                txid_history: Vec::new(),
                chain_anchor: None,
            }],
        }
    }

    #[test]
    fn runtime_restores_live_lineage_metadata_without_loss() {
        let lineage_id = LineageId::new();
        let anchor = sample_anchor(9, 321);
        let _request = sample_request("send-1", 1);
        let head_txid = Txid::from_byte_array([7u8; 32]);
        let sibling_txid = Txid::from_byte_array([8u8; 32]);

        let state = WalletRuntimeState::from_restored(
            RestoredWalletState {
                pending: Vec::new(),
                inflight: vec![LiveLineageSnapshot {
                    lineage_id,
                    head_txid,
                    all_txids: vec![sibling_txid, head_txid],
                    requests: vec![live_request("send-1", 1, vec![sibling_txid, head_txid])],
                    cover_utxos: vec![cover_utxo(20, 0, 17_000)],
                    chain_anchor: Some(anchor.clone()),
                }],
            },
            870_000,
        );

        let lineage = state
            .live_lineage(&lineage_id)
            .expect("lineage should be present");

        assert_eq!(
            lineage,
            &LiveLineage {
                lineage_id,
                head_txid,
                all_txids: vec![sibling_txid, head_txid],
                requests: vec![live_request("send-1", 1, vec![sibling_txid, head_txid])],
                cover_utxos: vec![cover_utxo(20, 0, 17_000)],
                chain_anchor: Some(anchor),
            }
        );
        assert_eq!(state.current_height, 870_000);
    }

    #[test]
    fn pending_state_preserves_free_and_anchored_requests_without_loss() {
        let free = PendingWalletRequest {
            request: sample_request("free-1", 2),
            chain_anchor: None,
            created_at: Timestamp::default(),
        };
        let anchored = PendingWalletRequest {
            request: sample_request("anchored-1", 3),
            chain_anchor: Some(sample_anchor(4, 654)),
            created_at: Timestamp::default(),
        };

        let pending = PendingState::from_requests(vec![free.clone(), anchored.clone()]);

        assert_eq!(pending.free, vec![free]);
        assert_eq!(pending.anchored, vec![anchored]);
        assert!(!pending.is_empty());
    }

    #[test]
    fn missing_observations_are_tracked_per_lineage() {
        let first = LineageId::new();
        let second = LineageId::new();
        let mut state = WalletRuntimeState::from_restored(RestoredWalletState::default(), 0);

        assert_eq!(state.record_missing_observation(first), 1);
        assert_eq!(state.record_missing_observation(first), 2);
        assert_eq!(state.record_missing_observation(second), 1);
        assert_eq!(state.missing_observations(&first), 2);
        assert_eq!(state.missing_observations(&second), 1);

        state.reset_missing_observations(&first);

        assert_eq!(state.missing_observations(&first), 0);
        assert_eq!(state.missing_observations(&second), 1);
    }

    #[derive(Default)]
    struct NoopPlanner;

    impl PlannerCostEvaluator for NoopPlanner {
        fn fresh_cost(&self, _requests: &[PendingWalletRequest]) -> Option<u64> {
            None
        }

        fn rbf_cost(
            &self,
            _lineage: &LiveLineage,
            _requests: &[PendingWalletRequest],
        ) -> Option<u64> {
            None
        }

        fn chained_cost(
            &self,
            _chain_anchor: &ChainAnchor,
            _existing_lineage: Option<&LiveLineage>,
            _requests: &[PendingWalletRequest],
        ) -> Option<u64> {
            None
        }
    }

    #[derive(Default)]
    struct RecordingExecutor {
        calls: Vec<PlannedBatchAction>,
    }

    impl PlannedActionExecutor for RecordingExecutor {
        fn execute(&mut self, action: PlannedBatchAction) {
            self.calls.push(action);
        }
    }

    #[derive(Default)]
    struct FixedPlanner {
        fresh: HashMap<String, u64>,
        chained: HashMap<String, u64>,
    }

    impl FixedPlanner {
        fn with_fresh(mut self, keys: &[&str], cost: u64) -> Self {
            self.fresh.insert(join_keys(keys), cost);
            self
        }

        fn with_chained(mut self, keys: &[&str], cost: u64) -> Self {
            self.chained.insert(join_keys(keys), cost);
            self
        }
    }

    impl PlannerCostEvaluator for FixedPlanner {
        fn fresh_cost(&self, requests: &[PendingWalletRequest]) -> Option<u64> {
            self.fresh.get(&request_keys(requests)).copied()
        }

        fn rbf_cost(
            &self,
            _lineage: &LiveLineage,
            _requests: &[PendingWalletRequest],
        ) -> Option<u64> {
            None
        }

        fn chained_cost(
            &self,
            _chain_anchor: &ChainAnchor,
            _existing_lineage: Option<&LiveLineage>,
            requests: &[PendingWalletRequest],
        ) -> Option<u64> {
            self.chained.get(&request_keys(requests)).copied()
        }
    }

    fn request_keys(requests: &[PendingWalletRequest]) -> String {
        let mut keys = requests
            .iter()
            .map(|request| request.request.dedupe_key().to_string())
            .collect::<Vec<_>>();
        keys.sort();
        keys.join("|")
    }

    fn join_keys(keys: &[&str]) -> String {
        let mut keys = keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
        keys.sort();
        keys.join("|")
    }

    #[test]
    fn tick_moves_incoming_requests_into_pending_state() {
        let mut state = WalletRuntimeState::from_restored(RestoredWalletState::default(), 0);
        let request = PendingWalletRequest {
            request: sample_request("free-1", 4),
            chain_anchor: None,
            created_at: timestamp(10),
        };

        let outcome = state.tick(
            vec![request.clone()],
            100,
            timestamp(10),
            &WalletConfig::default(),
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert!(outcome.executed_actions.is_empty());
        assert_eq!(state.pending.free, vec![request]);
        assert!(state.pending.anchored.is_empty());
    }

    #[test]
    fn tick_places_incoming_anchored_requests_in_anchored_bucket() {
        let anchor = sample_anchor(10, 100);
        let mut state = WalletRuntimeState::from_restored(RestoredWalletState::default(), 0);
        let request = PendingWalletRequest {
            request: sample_request("anchored-1", 4),
            chain_anchor: Some(anchor.clone()),
            created_at: timestamp(10),
        };

        state.tick(
            vec![request.clone()],
            100,
            timestamp(10),
            &WalletConfig::default(),
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert!(state.pending.free.is_empty());
        assert_eq!(state.pending.anchored, vec![request]);
    }

    #[test]
    fn tick_clears_expired_chain_anchors_before_planning() {
        let anchor = sample_anchor(11, 100);
        let anchored = PendingWalletRequest {
            request: sample_request("anchored-1", 5),
            chain_anchor: Some(anchor),
            created_at: timestamp(100),
        };
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: Vec::new(),
                anchored: vec![anchored.clone()],
            },
            missing_observations: HashMap::new(),
            current_height: 100,
        };
        let mut config = WalletConfig::default();
        config.chain_anchor_confirmations = 6;

        let outcome = state.tick(
            Vec::new(),
            105,
            timestamp(105),
            &config,
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert_eq!(outcome.expired_anchor_requests.len(), 1);
        assert!(state.pending.anchored.is_empty());
        assert_eq!(state.pending.free.len(), 1);
        assert_eq!(state.pending.free[0].request, anchored.request);
        assert_eq!(state.pending.free[0].chain_anchor, None);
    }

    #[test]
    fn tick_keeps_unexpired_chain_anchors_pending() {
        let anchor = sample_anchor(13, 100);
        let anchored = PendingWalletRequest {
            request: sample_request("anchored-1", 5),
            chain_anchor: Some(anchor.clone()),
            created_at: timestamp(100),
        };
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: Vec::new(),
                anchored: vec![anchored.clone()],
            },
            missing_observations: HashMap::new(),
            current_height: 100,
        };
        let mut config = WalletConfig::default();
        config.chain_anchor_confirmations = 6;

        let outcome = state.tick(
            Vec::new(),
            104,
            timestamp(104),
            &config,
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert!(outcome.expired_anchor_requests.is_empty());
        assert!(state.pending.free.is_empty());
        assert_eq!(state.pending.anchored, vec![anchored]);
    }

    #[test]
    fn release_chain_anchor_group_moves_requests_to_free_without_anchor() {
        let anchor = sample_anchor(14, 200);
        let other_anchor = sample_anchor(15, 200);
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: Vec::new(),
                anchored: vec![
                    PendingWalletRequest {
                        request: sample_request("anchored-a", 5),
                        chain_anchor: Some(anchor.clone()),
                        created_at: timestamp(0),
                    },
                    PendingWalletRequest {
                        request: sample_request("anchored-b", 6),
                        chain_anchor: Some(anchor.clone()),
                        created_at: timestamp(1),
                    },
                    PendingWalletRequest {
                        request: sample_request("anchored-other", 7),
                        chain_anchor: Some(other_anchor.clone()),
                        created_at: timestamp(2),
                    },
                ],
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };

        let released = state.release_chain_anchor_group(&anchor);

        assert_eq!(
            released,
            vec![
                PendingWalletRequest {
                    request: sample_request("anchored-a", 5),
                    chain_anchor: None,
                    created_at: timestamp(0),
                },
                PendingWalletRequest {
                    request: sample_request("anchored-b", 6),
                    chain_anchor: None,
                    created_at: timestamp(1),
                },
            ]
        );
        assert_eq!(state.pending.free, released);
        assert_eq!(
            state.pending.anchored,
            vec![PendingWalletRequest {
                request: sample_request("anchored-other", 7),
                chain_anchor: Some(other_anchor),
                created_at: timestamp(2),
            }]
        );
    }

    #[test]
    fn tick_drops_stale_pending_requests_after_ttl() {
        let request = PendingWalletRequest {
            request: sample_request("free-1", 6),
            chain_anchor: None,
            created_at: timestamp(0),
        };
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: vec![request.clone()],
                anchored: Vec::new(),
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };
        let mut config = WalletConfig::default();
        config.max_pending_ttl_secs = 10;

        let outcome = state.tick(
            Vec::new(),
            0,
            timestamp(11),
            &config,
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert_eq!(outcome.dropped_requests, vec![request]);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn tick_drops_stale_anchored_requests_after_ttl() {
        let request = PendingWalletRequest {
            request: sample_request("anchored-1", 6),
            chain_anchor: Some(sample_anchor(14, 0)),
            created_at: timestamp(0),
        };
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: Vec::new(),
                anchored: vec![request.clone()],
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };
        let mut config = WalletConfig::default();
        config.max_pending_ttl_secs = 10;
        config.chain_anchor_confirmations = 100;

        let outcome = state.tick(
            Vec::new(),
            0,
            timestamp(11),
            &config,
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert_eq!(outcome.dropped_requests, vec![request]);
        assert!(state.pending.is_empty());
    }

    #[test]
    fn tick_keeps_request_at_exact_ttl_boundary() {
        let request = PendingWalletRequest {
            request: sample_request("free-1", 6),
            chain_anchor: None,
            created_at: timestamp(0),
        };
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: vec![request.clone()],
                anchored: Vec::new(),
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };
        let mut config = WalletConfig::default();
        config.max_pending_ttl_secs = 10;

        let outcome = state.tick(
            Vec::new(),
            0,
            timestamp(10),
            &config,
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert!(outcome.dropped_requests.is_empty());
        assert_eq!(state.pending.free, vec![request]);
    }

    #[test]
    fn tick_skips_planner_when_no_executable_work_exists() {
        let request = PendingWalletRequest {
            request: sample_request("free-1", 7),
            chain_anchor: None,
            created_at: timestamp(0),
        };
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: vec![request],
                anchored: Vec::new(),
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };
        let mut executor = RecordingExecutor::default();

        let outcome = state.tick(
            Vec::new(),
            1,
            timestamp(1),
            &WalletConfig::default(),
            &NoopPlanner,
            &mut executor,
        );

        assert!(outcome.executed_actions.is_empty());
        assert!(executor.calls.is_empty());
    }

    #[test]
    fn tick_updates_current_height_even_when_no_work_exists() {
        let mut state = WalletRuntimeState::from_restored(RestoredWalletState::default(), 0);

        state.tick(
            Vec::new(),
            321,
            timestamp(1),
            &WalletConfig::default(),
            &NoopPlanner,
            &mut RecordingExecutor::default(),
        );

        assert_eq!(state.current_height, 321);
    }

    #[test]
    fn tick_expires_anchors_before_planner_evaluation() {
        let chain_anchor = sample_anchor(15, 100);
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: Vec::new(),
                anchored: vec![PendingWalletRequest {
                    request: sample_request("anchored-1", 9),
                    chain_anchor: Some(chain_anchor.clone()),
                    created_at: timestamp(0),
                }],
            },
            missing_observations: HashMap::new(),
            current_height: 100,
        };
        let mut config = WalletConfig::default();
        config.chain_anchor_confirmations = 6;
        let planner = FixedPlanner::default().with_fresh(&["anchored-1"], 20);
        let mut executor = RecordingExecutor::default();

        let outcome = state.tick(
            Vec::new(),
            105,
            timestamp(1),
            &config,
            &planner,
            &mut executor,
        );

        assert_eq!(
            outcome.executed_actions,
            vec![PlannedBatchAction::Fresh {
                requests: vec![PendingWalletRequest {
                    request: sample_request("anchored-1", 9),
                    chain_anchor: None,
                    created_at: timestamp(0),
                }],
            }]
        );
    }

    #[test]
    fn tick_drops_stale_requests_before_planner_evaluation() {
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: vec![PendingWalletRequest {
                    request: sample_request("free-1", 8),
                    chain_anchor: None,
                    created_at: timestamp(0),
                }],
                anchored: Vec::new(),
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };
        let mut config = WalletConfig::default();
        config.max_pending_ttl_secs = 10;
        let planner = FixedPlanner::default().with_fresh(&["free-1"], 20);
        let mut executor = RecordingExecutor::default();

        let outcome = state.tick(
            Vec::new(),
            0,
            timestamp(11),
            &config,
            &planner,
            &mut executor,
        );

        assert!(outcome.executed_actions.is_empty());
        assert!(executor.calls.is_empty());
    }

    #[test]
    fn tick_executes_planned_actions_sequentially() {
        let chain_anchor = sample_anchor(12, 100);
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::new(),
            pending: PendingState {
                free: vec![PendingWalletRequest {
                    request: sample_request("free-1", 8),
                    chain_anchor: None,
                    created_at: timestamp(0),
                }],
                anchored: vec![PendingWalletRequest {
                    request: sample_request("anchored-1", 9),
                    chain_anchor: Some(chain_anchor.clone()),
                    created_at: timestamp(0),
                }],
            },
            missing_observations: HashMap::new(),
            current_height: 0,
        };
        let planner = FixedPlanner::default()
            .with_chained(&["anchored-1"], 100)
            .with_fresh(&["free-1"], 40);
        let mut executor = RecordingExecutor::default();

        let outcome = state.tick(
            Vec::new(),
            101,
            timestamp(1),
            &WalletConfig::default(),
            &planner,
            &mut executor,
        );

        assert_eq!(outcome.executed_actions, executor.calls);
        assert_eq!(
            executor.calls,
            vec![
                PlannedBatchAction::Chained {
                    existing_lineage_id: None,
                    chain_anchor: chain_anchor.clone(),
                    requests: vec![PendingWalletRequest {
                        request: sample_request("anchored-1", 9),
                        chain_anchor: Some(chain_anchor),
                        created_at: timestamp(0),
                    }],
                },
                PlannedBatchAction::Fresh {
                    requests: vec![PendingWalletRequest {
                        request: sample_request("free-1", 8),
                        chain_anchor: None,
                        created_at: timestamp(0),
                    }],
                },
            ]
        );
    }

    #[test]
    fn tick_reuses_existing_chained_lineage_for_same_anchor() {
        let lineage_id = LineageId::new();
        let chain_anchor = sample_anchor(16, 100);
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::from([(
                lineage_id,
                LiveLineage {
                    lineage_id,
                    head_txid: Txid::from_byte_array([61u8; 32]),
                    all_txids: vec![Txid::from_byte_array([61u8; 32])],
                    requests: vec![live_request(
                        "existing",
                        10,
                        vec![Txid::from_byte_array([61u8; 32])],
                    )],
                    cover_utxos: vec![cover_utxo(41, 0, 19_000)],
                    chain_anchor: Some(chain_anchor.clone()),
                },
            )]),
            pending: PendingState {
                free: Vec::new(),
                anchored: vec![PendingWalletRequest {
                    request: sample_request("anchored-1", 11),
                    chain_anchor: Some(chain_anchor.clone()),
                    created_at: timestamp(0),
                }],
            },
            missing_observations: HashMap::new(),
            current_height: 100,
        };
        let planner = FixedPlanner::default().with_chained(&["anchored-1"], 30);
        let mut executor = RecordingExecutor::default();

        let outcome = state.tick(
            Vec::new(),
            101,
            timestamp(1),
            &WalletConfig::default(),
            &planner,
            &mut executor,
        );

        assert_eq!(
            outcome.executed_actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: Some(lineage_id),
                chain_anchor: chain_anchor.clone(),
                requests: vec![PendingWalletRequest {
                    request: sample_request("anchored-1", 11),
                    chain_anchor: Some(chain_anchor),
                    created_at: timestamp(0),
                }],
            }]
        );
    }

    #[test]
    fn tick_mixes_free_requests_into_reused_chained_lineage_when_cheaper() {
        let lineage_id = LineageId::new();
        let chain_anchor = sample_anchor(17, 100);
        let mut state = WalletRuntimeState {
            live_lineages: HashMap::from([(
                lineage_id,
                LiveLineage {
                    lineage_id,
                    head_txid: Txid::from_byte_array([62u8; 32]),
                    all_txids: vec![Txid::from_byte_array([62u8; 32])],
                    requests: vec![live_request(
                        "existing",
                        12,
                        vec![Txid::from_byte_array([62u8; 32])],
                    )],
                    cover_utxos: vec![cover_utxo(42, 0, 23_000)],
                    chain_anchor: Some(chain_anchor.clone()),
                },
            )]),
            pending: PendingState {
                free: vec![PendingWalletRequest {
                    request: sample_request("free-1", 13),
                    chain_anchor: None,
                    created_at: timestamp(0),
                }],
                anchored: vec![PendingWalletRequest {
                    request: sample_request("anchored-1", 14),
                    chain_anchor: Some(chain_anchor.clone()),
                    created_at: timestamp(0),
                }],
            },
            missing_observations: HashMap::new(),
            current_height: 100,
        };
        let planner = FixedPlanner::default()
            .with_chained(&["anchored-1"], 100)
            .with_chained(&["anchored-1", "free-1"], 130)
            .with_fresh(&["free-1"], 60);
        let mut executor = RecordingExecutor::default();

        let outcome = state.tick(
            Vec::new(),
            101,
            timestamp(1),
            &WalletConfig::default(),
            &planner,
            &mut executor,
        );

        assert_eq!(
            outcome.executed_actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: Some(lineage_id),
                chain_anchor: chain_anchor.clone(),
                requests: vec![
                    PendingWalletRequest {
                        request: sample_request("anchored-1", 14),
                        chain_anchor: Some(chain_anchor),
                        created_at: timestamp(0),
                    },
                    PendingWalletRequest {
                        request: sample_request("free-1", 13),
                        chain_anchor: None,
                        created_at: timestamp(0),
                    },
                ],
            }]
        );
    }

    #[derive(Clone)]
    struct FakeWalletStore {
        state: std::sync::Arc<Mutex<FakeWalletStoreState>>,
    }

    #[derive(Clone, Default)]
    struct FakeWalletStoreState {
        persist_receipt: Option<PersistedBatchReceipt>,
        restored: RestoredWalletState,
        persisted_plans: Vec<BroadcastPersistencePlan>,
        reverted_receipts: Vec<PersistedBatchReceipt>,
        reconciled_plans: Vec<ReconciliationPersistencePlan>,
    }

    impl FakeWalletStore {
        fn with_receipt(receipt: PersistedBatchReceipt) -> Self {
            Self {
                state: std::sync::Arc::new(Mutex::new(FakeWalletStoreState {
                    persist_receipt: Some(receipt),
                    restored: RestoredWalletState::default(),
                    persisted_plans: Vec::new(),
                    reverted_receipts: Vec::new(),
                    reconciled_plans: Vec::new(),
                })),
            }
        }

        fn with_restored(restored: RestoredWalletState) -> Self {
            Self {
                state: std::sync::Arc::new(Mutex::new(FakeWalletStoreState {
                    persist_receipt: None,
                    restored,
                    persisted_plans: Vec::new(),
                    reverted_receipts: Vec::new(),
                    reconciled_plans: Vec::new(),
                })),
            }
        }

        fn persisted_plans(&self) -> Vec<BroadcastPersistencePlan> {
            self.state.lock().expect("lock").persisted_plans.clone()
        }

        fn reverted_receipts(&self) -> Vec<PersistedBatchReceipt> {
            self.state.lock().expect("lock").reverted_receipts.clone()
        }

        fn reconciled_plans(&self) -> Vec<ReconciliationPersistencePlan> {
            self.state.lock().expect("lock").reconciled_plans.clone()
        }
    }

    #[async_trait]
    impl WalletStore for FakeWalletStore {
        async fn enqueue(
            &self,
            _scope: &str,
            _request: &WalletRequest,
        ) -> Result<
            crate::infrastructure::chain::bitcoin::wallet::EnqueueWalletRequestResult,
            ExecutorError,
        > {
            unreachable!("enqueue is not used in runtime tests")
        }

        async fn restore(&self, _scope: &str) -> Result<RestoredWalletState, ExecutorError> {
            Ok(self.state.lock().expect("lock").restored.clone())
        }

        async fn resolve_pending(
            &self,
            _scope: &str,
            _dedupe_key: &str,
        ) -> Result<
            crate::infrastructure::chain::bitcoin::wallet::store::ResolvePendingWalletRequestResult,
            ExecutorError,
        > {
            unreachable!("resolve_pending is not used in runtime tests")
        }

        async fn persist_broadcast(
            &self,
            _scope: &str,
            plan: &BroadcastPersistencePlan,
        ) -> Result<PersistedBatchReceipt, ExecutorError> {
            let mut state = self.state.lock().expect("lock");
            state.persisted_plans.push(plan.clone());
            state
                .persist_receipt
                .clone()
                .ok_or_else(|| ExecutorError::Domain("missing fake persist receipt".to_string()))
        }

        async fn revert_broadcast(
            &self,
            _scope: &str,
            receipt: &PersistedBatchReceipt,
        ) -> Result<(), ExecutorError> {
            self.state
                .lock()
                .expect("lock")
                .reverted_receipts
                .push(receipt.clone());
            Ok(())
        }

        async fn persist_confirmation(
            &self,
            _scope: &str,
            _plan: &ConfirmationPersistencePlan,
        ) -> Result<(), ExecutorError> {
            unreachable!("persist_confirmation is not used in current runtime tests")
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
            plan: &ReconciliationPersistencePlan,
        ) -> Result<(), ExecutorError> {
            self.state
                .lock()
                .expect("lock")
                .reconciled_plans
                .push(plan.clone());
            Ok(())
        }
    }

    struct FixedBroadcaster {
        outcome: BroadcastAcceptance,
    }

    #[async_trait]
    impl WalletBatchBroadcaster for FixedBroadcaster {
        async fn broadcast(
            &self,
            _scope: &str,
            _receipt: &PersistedBatchReceipt,
        ) -> Result<BroadcastAcceptance, ExecutorError> {
            Ok(self.outcome.clone())
        }
    }

    #[derive(Clone, Copy)]
    enum FakeObservedState {
        Confirmed,
        InMempool,
        Missing,
        Error,
    }

    struct FixedObserver {
        states: HashMap<Txid, FakeObservedState>,
        funding_inputs: HashMap<Txid, Vec<CoverUtxo>>,
        txs: HashMap<Txid, EsploraTx>,
    }

    #[async_trait]
    impl WalletTxObserver for FixedObserver {
        async fn observe_tx(&self, txid: Txid) -> Result<ObservedTxState, ExecutorError> {
            match self
                .states
                .get(&txid)
                .copied()
                .unwrap_or(FakeObservedState::Missing)
            {
                FakeObservedState::Confirmed => Ok(ObservedTxState::Confirmed),
                FakeObservedState::InMempool => Ok(ObservedTxState::InMempool),
                FakeObservedState::Missing => Ok(ObservedTxState::Missing),
                FakeObservedState::Error => {
                    Err(ExecutorError::Domain("temporary observer failure".to_string()))
                },
            }
        }

        async fn load_wallet_funding_inputs(
            &self,
            txid: Txid,
        ) -> Result<Vec<CoverUtxo>, ExecutorError> {
            Ok(self.funding_inputs.get(&txid).cloned().unwrap_or_default())
        }

        async fn load_tx(&self, txid: Txid) -> Result<EsploraTx, ExecutorError> {
            self.txs
                .get(&txid)
                .cloned()
                .ok_or_else(|| ExecutorError::Domain(format!("missing fake tx {txid}")))
        }
    }

    #[tokio::test]
    async fn persist_and_submit_reverts_rejected_broadcasts() {
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([21u8; 32]);
        let receipt = sample_receipt(lineage_id, txid);
        let store = FakeWalletStore::with_receipt(receipt.clone());
        let plan = BroadcastPersistencePlan {
            kind: BroadcastPersistenceKind::Fresh,
            lineage_id,
            txid,
            raw_tx_hex: "deadbeef".to_string(),
            included_request_keys: vec!["req-1".to_string()],
            dropped_request_keys: Vec::new(),
        };

        let result = persist_and_submit_broadcast(
            &store,
            &FixedBroadcaster {
                outcome: BroadcastAcceptance::Rejected,
            },
            "scope",
            &plan,
        )
        .await
        .expect("persist and submit");

        assert_eq!(result, BroadcastSubmissionResult::Rejected(receipt.clone()));
        assert_eq!(store.persisted_plans(), vec![plan]);
        assert_eq!(store.reverted_receipts(), vec![receipt]);
    }

    #[tokio::test]
    async fn persist_and_submit_keeps_ambiguous_broadcasts_inflight() {
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([22u8; 32]);
        let receipt = sample_receipt(lineage_id, txid);
        let store = FakeWalletStore::with_receipt(receipt.clone());
        let plan = BroadcastPersistencePlan {
            kind: BroadcastPersistenceKind::Fresh,
            lineage_id,
            txid,
            raw_tx_hex: "deadbeef".to_string(),
            included_request_keys: vec!["req-1".to_string()],
            dropped_request_keys: Vec::new(),
        };

        let result = persist_and_submit_broadcast(
            &store,
            &FixedBroadcaster {
                outcome: BroadcastAcceptance::Ambiguous,
            },
            "scope",
            &plan,
        )
        .await
        .expect("persist and submit");

        assert_eq!(result, BroadcastSubmissionResult::Ambiguous(receipt));
        assert_eq!(store.persisted_plans(), vec![plan]);
        assert!(store.reverted_receipts().is_empty());
    }

    #[tokio::test]
    async fn recover_wallet_runtime_routes_confirmed_heads_into_confirmation_queue() {
        let lineage_id = LineageId::new();
        let head_txid = Txid::from_byte_array([71u8; 32]);
        let store = FakeWalletStore::with_restored(RestoredWalletState {
            pending: vec![PendingWalletRequest {
                request: sample_request("pending-1", 1),
                chain_anchor: None,
                created_at: timestamp(0),
            }],
            inflight: vec![LiveLineageSnapshot {
                lineage_id,
                head_txid,
                all_txids: vec![head_txid],
                requests: vec![live_request("inflight-1", 2, vec![head_txid])],
                cover_utxos: Vec::new(),
                chain_anchor: None,
            }],
        });
        let observer = FixedObserver {
            states: HashMap::from([(head_txid, FakeObservedState::Confirmed)]),
            funding_inputs: HashMap::from([(
                head_txid,
                vec![cover_utxo(90, 0, 21_000), cover_utxo(91, 1, 17_000)],
            )]),
            txs: HashMap::new(),
        };

        let recovered = recover_wallet_runtime(&store, &observer, "scope", 900_000)
            .await
            .expect("recover runtime");

        assert!(recovered.runtime.live_lineages.is_empty());
        assert_eq!(recovered.runtime.pending.free.len(), 1);
        assert!(recovered.runtime.missing_observations.is_empty());
        assert_eq!(recovered.confirmations_pending.len(), 1);
        assert_eq!(
            recovered.confirmations_pending[0].lineage.lineage_id,
            lineage_id
        );
        assert_eq!(recovered.confirmations_pending[0].confirmed_txid, head_txid);
        assert_eq!(
            recovered.confirmations_pending[0].lineage.cover_utxos,
            vec![cover_utxo(90, 0, 21_000), cover_utxo(91, 1, 17_000)]
        );
    }

    #[tokio::test]
    async fn recover_wallet_runtime_resumes_mempool_heads_as_live_lineages() {
        let lineage_id = LineageId::new();
        let head_txid = Txid::from_byte_array([72u8; 32]);
        let store = FakeWalletStore::with_restored(RestoredWalletState {
            pending: Vec::new(),
            inflight: vec![LiveLineageSnapshot {
                lineage_id,
                head_txid,
                all_txids: vec![Txid::from_byte_array([70u8; 32]), head_txid],
                requests: vec![live_request(
                    "inflight-1",
                    2,
                    vec![Txid::from_byte_array([70u8; 32]), head_txid],
                )],
                cover_utxos: Vec::new(),
                chain_anchor: Some(sample_anchor(7, 321)),
            }],
        });
        let observer = FixedObserver {
            states: HashMap::from([(head_txid, FakeObservedState::InMempool)]),
            funding_inputs: HashMap::from([(
                head_txid,
                vec![
                    cover_utxo(90, 0, 21_000),
                    CoverUtxo {
                        outpoint: sample_anchor(7, 321).change_outpoint,
                        value: sample_anchor(7, 321).change_value,
                        script_pubkey: sample_anchor(7, 321).change_script_pubkey,
                    },
                ],
            )]),
            txs: HashMap::new(),
        };

        let recovered = recover_wallet_runtime(&store, &observer, "scope", 900_001)
            .await
            .expect("recover runtime");

        let lineage = recovered
            .runtime
            .live_lineages
            .get(&lineage_id)
            .expect("live lineage");
        assert_eq!(lineage.head_txid, head_txid);
        assert_eq!(lineage.all_txids.len(), 2);
        assert_eq!(lineage.cover_utxos, vec![cover_utxo(90, 0, 21_000)]);
        assert_eq!(
            lineage.derived_lineage_prevout(),
            Some(CoverUtxo {
                outpoint: sample_anchor(7, 321).change_outpoint,
                value: sample_anchor(7, 321).change_value,
                script_pubkey: sample_anchor(7, 321).change_script_pubkey,
            })
        );
        assert!(recovered.confirmations_pending.is_empty());
        assert_eq!(recovered.runtime.current_height, 900_001);
        assert_eq!(recovered.runtime.missing_observations(&lineage_id), 0);
    }

    #[tokio::test]
    async fn recover_wallet_runtime_routes_confirmed_sibling_into_confirmation_queue() {
        let lineage_id = LineageId::new();
        let sibling_txid = Txid::from_byte_array([74u8; 32]);
        let head_txid = Txid::from_byte_array([75u8; 32]);
        let store = FakeWalletStore::with_restored(RestoredWalletState {
            pending: Vec::new(),
            inflight: vec![LiveLineageSnapshot {
                lineage_id,
                head_txid,
                all_txids: vec![sibling_txid, head_txid],
                requests: vec![live_request("inflight-1", 2, vec![sibling_txid, head_txid])],
                cover_utxos: Vec::new(),
                chain_anchor: None,
            }],
        });
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (sibling_txid, FakeObservedState::Confirmed),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let recovered = recover_wallet_runtime(&store, &observer, "scope", 900_003)
            .await
            .expect("recover runtime");

        assert!(recovered.runtime.live_lineages.is_empty());
        assert_eq!(
            recovered.confirmations_pending,
            vec![PendingLineageConfirmation {
                lineage: LiveLineage {
                    lineage_id,
                    head_txid,
                    all_txids: vec![sibling_txid, head_txid],
                    requests: vec![live_request("inflight-1", 2, vec![sibling_txid, head_txid])],
                    cover_utxos: Vec::new(),
                    chain_anchor: None,
                },
                confirmed_txid: sibling_txid,
            }]
        );
    }

    #[tokio::test]
    async fn recover_wallet_runtime_adopts_mempool_sibling_as_new_head() {
        let lineage_id = LineageId::new();
        let sibling_txid = Txid::from_byte_array([76u8; 32]);
        let head_txid = Txid::from_byte_array([77u8; 32]);
        let store = FakeWalletStore::with_restored(RestoredWalletState {
            pending: Vec::new(),
            inflight: vec![LiveLineageSnapshot {
                lineage_id,
                head_txid,
                all_txids: vec![sibling_txid, head_txid],
                requests: vec![live_request("inflight-1", 2, vec![sibling_txid, head_txid])],
                cover_utxos: Vec::new(),
                chain_anchor: Some(sample_anchor(12, 600)),
            }],
        });
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (sibling_txid, FakeObservedState::InMempool),
            ]),
            funding_inputs: HashMap::from([(
                sibling_txid,
                vec![
                    cover_utxo(91, 0, 31_000),
                    CoverUtxo {
                        outpoint: sample_anchor(12, 600).change_outpoint,
                        value: sample_anchor(12, 600).change_value,
                        script_pubkey: sample_anchor(12, 600).change_script_pubkey,
                    },
                ],
            )]),
            txs: HashMap::new(),
        };

        let recovered = recover_wallet_runtime(&store, &observer, "scope", 900_004)
            .await
            .expect("recover runtime");

        let lineage = recovered
            .runtime
            .live_lineages
            .get(&lineage_id)
            .expect("live lineage");
        assert_eq!(lineage.head_txid, sibling_txid);
        assert_eq!(lineage.cover_utxos, vec![cover_utxo(91, 0, 31_000)]);
        assert!(recovered.confirmations_pending.is_empty());
    }

    #[tokio::test]
    async fn recover_wallet_runtime_keeps_missing_heads_live_with_reset_observation_counter() {
        let lineage_id = LineageId::new();
        let head_txid = Txid::from_byte_array([73u8; 32]);
        let store = FakeWalletStore::with_restored(RestoredWalletState {
            pending: Vec::new(),
            inflight: vec![LiveLineageSnapshot {
                lineage_id,
                head_txid,
                all_txids: vec![head_txid],
                requests: vec![live_request("inflight-1", 2, vec![head_txid])],
                cover_utxos: Vec::new(),
                chain_anchor: None,
            }],
        });
        let observer = FixedObserver {
            states: HashMap::from([(head_txid, FakeObservedState::Error)]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let recovered = recover_wallet_runtime(&store, &observer, "scope", 900_002)
            .await
            .expect("recover runtime");

        assert!(recovered.confirmations_pending.is_empty());
        assert!(recovered.runtime.live_lineages.contains_key(&lineage_id));
        assert_eq!(recovered.runtime.missing_observations(&lineage_id), 0);
    }

    #[tokio::test]
    async fn observe_live_lineages_routes_confirmed_sibling_without_waiting_three_ticks() {
        let lineage_id = LineageId::new();
        let sibling_txid = Txid::from_byte_array([78u8; 32]);
        let head_txid = Txid::from_byte_array([79u8; 32]);
        let mut state = WalletRuntimeState::from_restored(
            RestoredWalletState {
                pending: Vec::new(),
                inflight: vec![LiveLineageSnapshot {
                    lineage_id,
                    head_txid,
                    all_txids: vec![sibling_txid, head_txid],
                    requests: vec![live_request("req-1", 1, vec![sibling_txid, head_txid])],
                    cover_utxos: Vec::new(),
                    chain_anchor: None,
                }],
            },
            0,
        );
        state.record_missing_observation(lineage_id);
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (sibling_txid, FakeObservedState::Confirmed),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let outcome = state.observe_live_lineages(&observer).await;

        assert_eq!(
            outcome,
            LiveObservationOutcome {
                confirmations_pending: vec![PendingLineageConfirmation {
                    lineage: LiveLineage {
                        lineage_id,
                        head_txid,
                        all_txids: vec![sibling_txid, head_txid],
                        requests: vec![live_request("req-1", 1, vec![sibling_txid, head_txid])],
                        cover_utxos: Vec::new(),
                        chain_anchor: None,
                    },
                    confirmed_txid: sibling_txid,
                }],
            }
        );
        assert!(!state.live_lineages.contains_key(&lineage_id));
        assert_eq!(state.missing_observations(&lineage_id), 0);
    }

    #[tokio::test]
    async fn observe_live_lineages_adopts_mempool_sibling_and_resets_observations() {
        let lineage_id = LineageId::new();
        let sibling_txid = Txid::from_byte_array([80u8; 32]);
        let head_txid = Txid::from_byte_array([81u8; 32]);
        let anchor = sample_anchor(13, 700);
        let mut state = WalletRuntimeState::from_restored(
            RestoredWalletState {
                pending: Vec::new(),
                inflight: vec![LiveLineageSnapshot {
                    lineage_id,
                    head_txid,
                    all_txids: vec![sibling_txid, head_txid],
                    requests: vec![live_request("req-1", 1, vec![sibling_txid, head_txid])],
                    cover_utxos: Vec::new(),
                    chain_anchor: Some(anchor.clone()),
                }],
            },
            0,
        );
        state.record_missing_observation(lineage_id);
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (sibling_txid, FakeObservedState::InMempool),
            ]),
            funding_inputs: HashMap::from([(
                sibling_txid,
                vec![
                    cover_utxo(92, 0, 33_000),
                    CoverUtxo {
                        outpoint: anchor.change_outpoint,
                        value: anchor.change_value,
                        script_pubkey: anchor.change_script_pubkey.clone(),
                    },
                ],
            )]),
            txs: HashMap::new(),
        };

        let outcome = state.observe_live_lineages(&observer).await;

        assert!(outcome.confirmations_pending.is_empty());
        let lineage = state.live_lineages.get(&lineage_id).expect("lineage");
        assert_eq!(lineage.head_txid, sibling_txid);
        assert_eq!(lineage.cover_utxos, vec![cover_utxo(92, 0, 33_000)]);
        assert_eq!(state.missing_observations(&lineage_id), 0);
    }

    #[tokio::test]
    async fn observe_live_lineages_increments_only_missing_lineage_counter_when_no_sibling_is_alive(
    ) {
        let first = LineageId::new();
        let second = LineageId::new();
        let first_head = Txid::from_byte_array([82u8; 32]);
        let first_sibling = Txid::from_byte_array([83u8; 32]);
        let second_head = Txid::from_byte_array([84u8; 32]);
        let mut state = WalletRuntimeState::from_restored(
            RestoredWalletState {
                pending: Vec::new(),
                inflight: vec![
                    LiveLineageSnapshot {
                        lineage_id: first,
                        head_txid: first_head,
                        all_txids: vec![first_sibling, first_head],
                        requests: vec![live_request("first", 1, vec![first_sibling, first_head])],
                        cover_utxos: Vec::new(),
                        chain_anchor: None,
                    },
                    LiveLineageSnapshot {
                        lineage_id: second,
                        head_txid: second_head,
                        all_txids: vec![second_head],
                        requests: vec![live_request("second", 2, vec![second_head])],
                        cover_utxos: Vec::new(),
                        chain_anchor: None,
                    },
                ],
            },
            0,
        );
        state.record_missing_observation(second);
        let observer = FixedObserver {
            states: HashMap::from([
                (first_head, FakeObservedState::Missing),
                (first_sibling, FakeObservedState::Missing),
                (second_head, FakeObservedState::InMempool),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let outcome = state.observe_live_lineages(&observer).await;

        assert!(outcome.confirmations_pending.is_empty());
        assert_eq!(state.missing_observations(&first), 1);
        assert_eq!(state.missing_observations(&second), 0);
        assert_eq!(
            state
                .live_lineages
                .get(&first)
                .expect("first lineage")
                .head_txid,
            first_head
        );
    }

    #[tokio::test]
    async fn reconcile_missing_lineage_confirms_survivor_and_sets_anchor_when_within_depth() {
        let store = FakeWalletStore::with_restored(RestoredWalletState::default());
        let survivor_txid = Txid::from_byte_array([90u8; 32]);
        let head_txid = Txid::from_byte_array([91u8; 32]);
        let wallet_script_pubkey = regtest_address(30).script_pubkey();
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![survivor_txid, head_txid],
            requests: vec![
                live_request("winner", 1, vec![survivor_txid, head_txid]),
                live_request("orphan", 2, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (survivor_txid, FakeObservedState::Confirmed),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::from([(
                survivor_txid,
                confirmed_tx(
                    survivor_txid,
                    vec![
                        tx_output(&regtest_address(31).script_pubkey(), 12_000),
                        tx_output(&wallet_script_pubkey, 23_000),
                    ],
                    100,
                ),
            )]),
        };

        let result = reconcile_missing_lineage(
            &store,
            &observer,
            "scope",
            &lineage,
            102,
            6,
            &wallet_script_pubkey,
        )
        .await
        .expect("reconcile lineage");

        assert_eq!(
            result,
            ReconciliationResult {
                state: ReconciledLineageState::Confirmed {
                    confirmed_txid: survivor_txid,
                },
                survivor_requests: vec![sample_request("winner", 1)],
                requeued_requests: vec![PendingWalletRequest {
                    request: sample_request("orphan", 2),
                    chain_anchor: Some(ChainAnchor {
                        confirmed_txid: survivor_txid,
                        change_outpoint: OutPoint {
                            txid: survivor_txid,
                            vout: 1,
                        },
                        change_value: 23_000,
                        change_script_pubkey: wallet_script_pubkey.clone(),
                        confirmed_height: 100,
                    }),
                    created_at: Timestamp::default(),
                }],
            }
        );
        assert_eq!(
            store.reconciled_plans(),
            vec![ReconciliationPersistencePlan {
                lineage_id: lineage.lineage_id,
                kind: ReconciliationPersistenceKind::Confirmed {
                    confirmed_txid: survivor_txid,
                },
                survivor_request_keys: vec!["winner".to_string()],
                requeued_request_keys: vec!["orphan".to_string()],
                chain_anchor: Some(ChainAnchor {
                    confirmed_txid: survivor_txid,
                    change_outpoint: OutPoint {
                        txid: survivor_txid,
                        vout: 1,
                    },
                    change_value: 23_000,
                    change_script_pubkey: wallet_script_pubkey,
                    confirmed_height: 100,
                }),
            }]
        );
    }

    #[tokio::test]
    async fn reconcile_missing_lineage_confirms_survivor_without_anchor_when_depth_expired() {
        let store = FakeWalletStore::with_restored(RestoredWalletState::default());
        let survivor_txid = Txid::from_byte_array([92u8; 32]);
        let head_txid = Txid::from_byte_array([93u8; 32]);
        let wallet_script_pubkey = regtest_address(32).script_pubkey();
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![survivor_txid, head_txid],
            requests: vec![
                live_request("winner", 1, vec![survivor_txid, head_txid]),
                live_request("orphan", 2, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (survivor_txid, FakeObservedState::Confirmed),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::from([(
                survivor_txid,
                confirmed_tx(
                    survivor_txid,
                    vec![tx_output(&wallet_script_pubkey, 23_000)],
                    100,
                ),
            )]),
        };

        let result = reconcile_missing_lineage(
            &store,
            &observer,
            "scope",
            &lineage,
            110,
            6,
            &wallet_script_pubkey,
        )
        .await
        .expect("reconcile lineage");

        assert_eq!(
            result.state,
            ReconciledLineageState::Confirmed {
                confirmed_txid: survivor_txid
            }
        );
        assert_eq!(result.requeued_requests[0].chain_anchor, None);
        assert_eq!(store.reconciled_plans()[0].chain_anchor, None);
    }

    #[tokio::test]
    async fn reconcile_missing_lineage_keeps_mempool_survivor_and_requeues_others_without_anchor() {
        let store = FakeWalletStore::with_restored(RestoredWalletState::default());
        let survivor_txid = Txid::from_byte_array([94u8; 32]);
        let head_txid = Txid::from_byte_array([95u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![survivor_txid, head_txid],
            requests: vec![
                live_request("survivor", 1, vec![survivor_txid, head_txid]),
                live_request("requeue", 2, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (survivor_txid, FakeObservedState::InMempool),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let result = reconcile_missing_lineage(
            &store,
            &observer,
            "scope",
            &lineage,
            110,
            6,
            &ScriptBuf::new(),
        )
        .await
        .expect("reconcile lineage");

        assert_eq!(
            result.state,
            ReconciledLineageState::InMempool {
                surviving_txid: survivor_txid
            }
        );
        assert_eq!(
            result.survivor_requests,
            vec![sample_request("survivor", 1)]
        );
        assert_eq!(
            result.requeued_requests,
            vec![PendingWalletRequest {
                request: sample_request("requeue", 2),
                chain_anchor: None,
                created_at: Timestamp::default(),
            }]
        );
    }

    #[tokio::test]
    async fn reconcile_missing_lineage_requeues_all_requests_when_no_survivor_exists() {
        let store = FakeWalletStore::with_restored(RestoredWalletState::default());
        let head_txid = Txid::from_byte_array([96u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![head_txid],
            requests: vec![
                live_request("req-1", 1, vec![head_txid]),
                live_request("req-2", 2, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let observer = FixedObserver {
            states: HashMap::from([(head_txid, FakeObservedState::Missing)]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let result = reconcile_missing_lineage(
            &store,
            &observer,
            "scope",
            &lineage,
            110,
            6,
            &ScriptBuf::new(),
        )
        .await
        .expect("reconcile lineage");

        assert_eq!(result.state, ReconciledLineageState::NoSurvivor);
        assert!(result.survivor_requests.is_empty());
        assert_eq!(result.requeued_requests.len(), 2);
        assert_eq!(
            store.reconciled_plans(),
            vec![ReconciliationPersistencePlan {
                lineage_id: lineage.lineage_id,
                kind: ReconciliationPersistenceKind::NoSurvivor,
                survivor_request_keys: Vec::new(),
                requeued_request_keys: vec!["req-1".to_string(), "req-2".to_string()],
                chain_anchor: None,
            }]
        );
    }

    #[tokio::test]
    async fn reconcile_missing_lineage_leaves_state_untouched_when_candidate_query_errors() {
        let store = FakeWalletStore::with_restored(RestoredWalletState::default());
        let head_txid = Txid::from_byte_array([97u8; 32]);
        let sibling_txid = Txid::from_byte_array([98u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![sibling_txid, head_txid],
            requests: vec![live_request("req-1", 1, vec![sibling_txid, head_txid])],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (sibling_txid, FakeObservedState::Error),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::new(),
        };

        let error = reconcile_missing_lineage(
            &store,
            &observer,
            "scope",
            &lineage,
            110,
            6,
            &ScriptBuf::new(),
        )
        .await
        .expect_err("candidate error must fail reconciliation");

        assert!(error.to_string().contains("temporary observer failure"));
        assert!(store.reconciled_plans().is_empty());
    }

    #[tokio::test]
    async fn reconcile_missing_lineage_surfaces_missing_change_output_and_persists_nothing() {
        let store = FakeWalletStore::with_restored(RestoredWalletState::default());
        let survivor_txid = Txid::from_byte_array([99u8; 32]);
        let head_txid = Txid::from_byte_array([100u8; 32]);
        let wallet_script_pubkey = regtest_address(33).script_pubkey();
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![survivor_txid, head_txid],
            requests: vec![
                live_request("winner", 1, vec![survivor_txid, head_txid]),
                live_request("orphan", 2, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let observer = FixedObserver {
            states: HashMap::from([
                (head_txid, FakeObservedState::Missing),
                (survivor_txid, FakeObservedState::Confirmed),
            ]),
            funding_inputs: HashMap::new(),
            txs: HashMap::from([(
                survivor_txid,
                confirmed_tx(
                    survivor_txid,
                    vec![tx_output(&regtest_address(34).script_pubkey(), 12_000)],
                    100,
                ),
            )]),
        };

        let error = reconcile_missing_lineage(
            &store,
            &observer,
            "scope",
            &lineage,
            102,
            6,
            &wallet_script_pubkey,
        )
        .await
        .expect_err("missing change output must fail reconciliation");

        assert!(error.to_string().contains("has no wallet change output"));
        assert!(store.reconciled_plans().is_empty());
    }
}
