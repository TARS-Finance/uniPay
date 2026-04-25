//! Persistence-facing types for the Bitcoin wallet runner.
//!
//! The wallet runtime is restart-safe because every request and lineage moves
//! through explicit persisted states. These types describe the snapshots and
//! transition plans that the runtime expects repository implementations to
//! support atomically.
//!
//! Store vocabulary:
//!
//! ```text
//! pending
//!   request is known locally but not attached to any submitted tx yet
//!
//! inflight
//!   request currently belongs to a live lineage head in mempool/unknown state
//!
//! confirmed
//!   request is finalized under the txid that actually won on chain
//!
//! dropped
//!   request expired or was cancelled before a submission happened
//! ```
//!
//! Typical transitions:
//!
//! ```text
//! enqueue           : pending
//! persist_broadcast : pending -> inflight
//! persist_confirmation:
//!   winner members  : inflight -> confirmed
//!   orphaned work   : inflight -> pending(anchor=?)
//! persist_reconciliation:
//!   mempool winner  : inflight -> inflight(surviving_txid)
//!   no survivor     : inflight -> pending
//! revert_broadcast  : restore exact pre-broadcast snapshot
//! ```

use crate::errors::ExecutorError;
use crate::timestamp::Timestamp;
use async_trait::async_trait;
use bitcoin::Txid;

use super::{ChainAnchor, CoverUtxo, LineageId, WalletRequest};

/// Result of attempting to enqueue a request into the wallet store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnqueueWalletRequestResult {
    /// The request was inserted as newly pending work.
    EnqueuedPending,
    /// The same dedupe key is already pending.
    AlreadyPending,
    /// The request is already attached to a live lineage in mempool.
    AlreadyInflight { lineage_id: LineageId, txid: Txid },
    /// The request has already confirmed and should be treated as complete.
    AlreadyConfirmed { lineage_id: LineageId, txid: Txid },
    /// The request previously expired or was explicitly dropped.
    AlreadyDropped,
}

/// Result of asking the store to resolve a still-pending request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvePendingWalletRequestResult {
    /// The request was still pending and could be cancelled locally.
    CancelledPending,
    /// The request had already been submitted, so the caller should use `txid`.
    AlreadySubmitted { txid: Txid },
}

/// Request that has not yet been attached to a submitted batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingWalletRequest {
    /// Original user/coordinator request.
    pub request: WalletRequest,
    /// Optional anchor that forces the request to remain chained to a specific
    /// confirmed change output.
    pub chain_anchor: Option<ChainAnchor>,
    /// Original enqueue time used for TTL decisions.
    pub created_at: Timestamp,
}

/// Request that is currently part of a live lineage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveLineageRequest {
    /// Original request payload.
    pub request: WalletRequest,
    /// Ordered txids this request has appeared in across fresh/RBF builds.
    pub txid_history: Vec<Txid>,
    /// Original enqueue time preserved across retries/reconciliation.
    pub created_at: Timestamp,
}

/// Restorable snapshot for a single live lineage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveLineageSnapshot {
    /// Stable lineage id across replacements.
    pub lineage_id: LineageId,
    /// Current mempool head observed for the lineage.
    pub head_txid: Txid,
    /// Full txid history for reconciliation against missing heads.
    pub all_txids: Vec<Txid>,
    /// Requests currently surviving inside the lineage.
    pub requests: Vec<LiveLineageRequest>,
    /// Wallet-owned fee-paying inputs carried forward for future RBF attempts.
    pub cover_utxos: Vec<CoverUtxo>,
    /// Optional chain anchor shared by chained descendants.
    pub chain_anchor: Option<ChainAnchor>,
}

/// Complete restore payload for a wallet scope.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoredWalletState {
    pub pending: Vec<PendingWalletRequest>,
    pub inflight: Vec<LiveLineageSnapshot>,
}

/// Kind of persistence transition performed when a batch is first stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BroadcastPersistenceKind {
    Fresh,
    Rbf,
    Chained,
}

/// Atomic persistence plan for a newly built batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BroadcastPersistencePlan {
    /// Whether this batch started fresh, replaced an existing lineage, or
    /// chained onto a confirmed anchor.
    pub kind: BroadcastPersistenceKind,
    /// Lineage receiving the submission.
    pub lineage_id: LineageId,
    /// Candidate txid before broadcast.
    pub txid: Txid,
    /// Raw transaction persisted so ambiguous broadcasts can be retried.
    pub raw_tx_hex: String,
    /// Requests included in the batch.
    pub included_request_keys: Vec<String>,
    /// Requests intentionally removed when writing the new lineage head.
    ///
    /// This is only used for RBF. Example:
    ///
    /// ```text
    /// old head tx_1 = [req_a, req_b]
    /// new head tx_2 = [req_a]
    ///
    /// included_request_keys = ["req_a"]
    /// dropped_request_keys  = ["req_b"]
    ///
    /// store result:
    ///   req_a -> inflight(lineage=L, batch_txid=tx_2)
    ///   req_b -> pending(lineage=NULL, batch_txid=NULL)
    /// ```
    pub dropped_request_keys: Vec<String>,
}

/// Lifecycle states recorded for individual wallet requests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WalletRequestLifecycleStatus {
    Pending,
    Inflight,
    Confirmed,
    Dropped,
}

/// Persisted state snapshot for a request after a broadcast attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedWalletRequestSnapshot {
    pub dedupe_key: String,
    pub status: WalletRequestLifecycleStatus,
    pub lineage_id: Option<LineageId>,
    pub batch_txid: Option<Txid>,
    pub txid_history: Vec<Txid>,
    pub chain_anchor: Option<ChainAnchor>,
}

/// Persisted batch metadata returned to the broadcaster/runtime.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PersistedBatchReceipt {
    pub lineage_id: LineageId,
    pub txid: Txid,
    pub raw_tx_hex: String,
    pub snapshots: Vec<PersistedWalletRequestSnapshot>,
}

/// Confirmed head selected for a lineage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmedLineageHead {
    pub lineage_id: LineageId,
    pub confirmed_txid: Txid,
}

/// Atomic persistence plan applied when a lineage confirmation wins.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmationPersistencePlan {
    pub lineage_id: LineageId,
    pub confirmed_txid: Txid,
    pub confirmed_request_keys: Vec<String>,
    /// Requests that used to belong to the lineage but are not present in the
    /// confirmed winner. They return to pending work, sometimes carrying a
    /// `chain_anchor` if the confirmed winner created reusable change.
    pub orphaned_request_keys: Vec<String>,
    /// Optional confirmed anchor handed to orphaned requests so the next batch
    /// is forced to spend from the winning change output.
    pub chain_anchor: Option<ChainAnchor>,
}

/// Persisted outcome when reconciling a lineage whose head disappeared.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationPersistenceKind {
    Confirmed { confirmed_txid: Txid },
    InMempool { surviving_txid: Txid },
    NoSurvivor,
}

/// Atomic persistence plan for missing-lineage reconciliation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationPersistencePlan {
    pub lineage_id: LineageId,
    pub kind: ReconciliationPersistenceKind,
    /// Requests still represented by the surviving sibling, if one exists.
    pub survivor_request_keys: Vec<String>,
    /// Requests orphaned by the surviving sibling or by the complete lack of a
    /// survivor. These are requeued to pending.
    pub requeued_request_keys: Vec<String>,
    pub chain_anchor: Option<ChainAnchor>,
}

/// Storage contract required by the wallet runner.
///
/// Implementations are assumed to provide per-method atomicity so the runtime
/// can recover safely after process crashes between ticks.
///
/// The runner depends on these methods as durable state boundaries:
///
/// - `enqueue` creates logical work.
/// - `persist_broadcast` is the "submission may happen now" boundary.
/// - `persist_confirmation` chooses the confirmed winner for a lineage.
/// - `persist_reconciliation` rewrites state when the latest head disappears.
/// - `revert_broadcast` undoes a rejected submission using the saved snapshot.
#[async_trait]
pub trait WalletStore: Send + Sync {
    /// Insert a pending request if its dedupe key has not been seen before.
    async fn enqueue(
        &self,
        scope: &str,
        request: &WalletRequest,
    ) -> Result<EnqueueWalletRequestResult, ExecutorError>;

    /// Restore the full runtime state for `scope`.
    async fn restore(&self, scope: &str) -> Result<RestoredWalletState, ExecutorError>;

    /// Resolve a request that may still be pending when the caller wants a
    /// definite outcome for a dedupe key.
    async fn resolve_pending(
        &self,
        scope: &str,
        dedupe_key: &str,
    ) -> Result<ResolvePendingWalletRequestResult, ExecutorError>;

    /// Persist a new lineage head and request membership before broadcast.
    async fn persist_broadcast(
        &self,
        scope: &str,
        plan: &BroadcastPersistencePlan,
    ) -> Result<PersistedBatchReceipt, ExecutorError>;

    /// Undo a persisted broadcast when the node rejected the raw transaction.
    async fn revert_broadcast(
        &self,
        scope: &str,
        receipt: &PersistedBatchReceipt,
    ) -> Result<(), ExecutorError>;

    /// Mark a confirmed lineage winner and split confirmed/orphaned requests.
    async fn persist_confirmation(
        &self,
        scope: &str,
        plan: &ConfirmationPersistencePlan,
    ) -> Result<(), ExecutorError>;

    /// List the latest confirmed winners per lineage for fee repair or replay.
    async fn list_confirmed_lineage_heads(
        &self,
        scope: &str,
        limit: usize,
    ) -> Result<Vec<ConfirmedLineageHead>, ExecutorError>;

    /// Return whether the wallet itself already submitted `txid`.
    async fn has_submitted_tx(&self, scope: &str, txid: Txid) -> Result<bool, ExecutorError>;

    /// Persist the result of reconciling a missing lineage head.
    async fn persist_reconciliation(
        &self,
        scope: &str,
        plan: &ReconciliationPersistencePlan,
    ) -> Result<(), ExecutorError>;
}
