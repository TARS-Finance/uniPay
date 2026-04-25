//! Bitcoin wallet batching subsystem.
//!
//! This module is the write-side execution engine behind the Bitcoin
//! integration. It accepts high-level wallet requests, batches them into
//! transactions, persists enough lineage metadata to survive restarts/RBF, and
//! reconciles missing or replaced heads back into pending work.
//!
//! High-level flow:
//!
//! ```text
//! BitcoinActionExecutor
//!   -> WalletRequestSubmitter
//!   -> WalletStore.enqueue(scope, dedupe_key)
//!   -> BitcoinWalletRunner.tick()
//!   -> planner chooses Fresh / Rbf / Chained
//!   -> tx_builder constructs the batch transaction
//!   -> WalletStore.persist_broadcast(...)
//!   -> broadcaster submits raw tx
//!   -> runtime tracks lineage head + txid history
//!   -> observer sees Confirmed / InMempool / Missing
//!   -> confirm/reconcile helpers persist the winner
//! ```
//!
//! Mental model:
//!
//! - `PendingWalletRequest` is logical work not yet attached to a tx.
//! - `LiveLineage` is one logical batch across every replacement tx it produced.
//! - `ChainAnchor` is confirmed change that later requests may chain onto.
//! - `txid_history` is per-request membership history used to decide which
//!   requests survived after a replacement or missing-head repair.
//!
//! Example batch evolution:
//!
//! ```text
//! tick 1:
//!   pending.free = [send_a, redeem_b]
//!   planner -> Fresh
//!   lineage L submits tx_1 with both requests
//!
//! tick 2:
//!   pending.free = [send_c]
//!   lineage L still in mempool
//!   planner -> Rbf(lineage=L, requests=[send_c])
//!   lineage L submits tx_2, and txid_history becomes:
//!     send_a   => [tx_1, tx_2]
//!     redeem_b => [tx_1, tx_2]
//!     send_c   => [tx_2]
//!
//! tick 3:
//!   tx_2 confirms and creates change output C
//!   orphaned child work can be requeued as:
//!     pending.anchored = [refund_d(anchor=C)]
//!   planner later emits Chained(anchor=C, requests=[refund_d])
//! ```
//!
//! Read the wallet subsystem as two linked halves:
//!
//! - `store` explains what is persisted for each lifecycle transition.
//! - `runtime` and `runner` explain when those transitions are chosen.

pub mod config;
pub mod confirm;
pub mod events;
pub mod htlc_adapter;
pub mod planner;
pub mod reconcile;
pub mod request;
pub mod runner;
pub mod runtime;
pub mod state;
pub mod store;

pub use config::WalletConfig;
pub use confirm::{extract_chain_anchor, partition_confirmed_lineage, ConfirmationPartition};
pub use events::WalletEvent;
pub use htlc_adapter::{BitcoinHtlcWalletAdapter, HtlcAction, HtlcAdapterError};
pub use planner::{plan_wallet_batches, PlannedBatchAction, PlannerCostEvaluator};
pub use reconcile::{
    partition_surviving_lineage, reconcile_missing_lineage, ReconciledLineageState,
    ReconciliationPartition, ReconciliationResult, ReconciliationSurvivor,
};
pub use request::{
    SendRequest, SpendRequest, WalletRequest, WalletRequestError, WalletRequestKind,
};
pub use runner::{
    wallet_scope, BitcoinWalletChainObserver, BitcoinWalletRunner, BitcoinWalletRunnerHandle,
    SubmittedWalletBatch, WalletRequestSubmitter,
};
pub use runtime::{
    persist_and_submit_broadcast, recover_wallet_runtime, BroadcastAcceptance,
    BroadcastSubmissionResult, LiveLineage, LiveObservationOutcome, ObservedTxState,
    PendingLineageConfirmation, PendingState, RecoveredWalletRuntime, WalletBatchBroadcaster,
    WalletRuntimeState, WalletTxObserver,
};
pub use store::{
    BroadcastPersistenceKind, BroadcastPersistencePlan, ConfirmationPersistencePlan,
    ConfirmedLineageHead, EnqueueWalletRequestResult, LiveLineageRequest, LiveLineageSnapshot,
    PendingWalletRequest, PersistedBatchReceipt, PersistedWalletRequestSnapshot,
    ReconciliationPersistenceKind, ReconciliationPersistencePlan, RestoredWalletState,
    WalletRequestLifecycleStatus, WalletStore,
};
pub use state::{ChainAnchor, CoverUtxo, LineageId};
