//! Postgres-backed [`WalletStore`] implementation for the Bitcoin runner.
//!
//! The repository persists request lifecycle state, lineage membership, txid
//! history, and chain-anchor metadata so the runtime can recover deterministically
//! after crashes, restarts, or RBF replacement.
//!
//! Store transition examples:
//!
//! ```text
//! enqueue("req_a")
//!   status=pending, lineage_id=NULL, batch_txid=NULL, txid_history=[]
//!
//! persist_broadcast(Fresh, tx_a, ["req_a"])
//!   status=inflight, lineage_id=L, batch_txid=tx_a, txid_history=[tx_a]
//!
//! persist_broadcast(Rbf, tx_b, included=["req_a"], dropped=["req_b"])
//!   req_a -> status=inflight, lineage_id=L, batch_txid=tx_b, txid_history=[tx_a, tx_b]
//!   req_b -> status=pending,  lineage_id=NULL, batch_txid=NULL
//!
//! persist_confirmation(tx_b, confirmed=["req_a"], orphaned=["req_b"], anchor=A)
//!   req_a -> status=confirmed, batch_txid=tx_b, chain_anchor=NULL
//!   req_b -> status=pending,   batch_txid=NULL, chain_anchor=A
//!
//! persist_reconciliation(NoSurvivor, requeued=["req_a","req_b"])
//!   req_a/req_b -> status=pending, lineage_id=NULL, batch_txid=NULL
//! ```
//!
//! Another common case is anchored replay after confirmation:
//!
//! ```text
//! lineage L confirms tx_b and produces wallet change output C
//! orphaned request req_c still needs to run
//!
//! persist_confirmation(
//!   confirmed=["req_a"],
//!   orphaned=["req_c"],
//!   anchor=C,
//! )
//!
//! row result:
//!   req_a -> confirmed(batch_txid=tx_b, chain_anchor=NULL)
//!   req_c -> pending(batch_txid=NULL, chain_anchor=C)
//! ```

use super::map_sqlx_error;
use crate::errors::{PersistenceError, ExecutorError};
use crate::timestamp::Timestamp;
use crate::infrastructure::chain::bitcoin::wallet::store::{
    BroadcastPersistenceKind, BroadcastPersistencePlan, ConfirmationPersistencePlan,
    ConfirmedLineageHead, EnqueueWalletRequestResult, LiveLineageRequest, LiveLineageSnapshot,
    PendingWalletRequest, PersistedBatchReceipt, PersistedWalletRequestSnapshot,
    ReconciliationPersistenceKind, ReconciliationPersistencePlan,
    ResolvePendingWalletRequestResult, RestoredWalletState, WalletRequestLifecycleStatus,
    WalletStore,
};
use crate::infrastructure::chain::bitcoin::wallet::{
    ChainAnchor, LineageId, WalletRequest, WalletRequestKind,
};
use async_trait::async_trait;
use bitcoin::Txid;
use sqlx::types::Json;
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;

/// Postgres implementation of the Bitcoin wallet store contract.
#[derive(Clone)]
pub struct PgBitcoinWalletStore {
    pool: PgPool,
}

impl PgBitcoinWalletStore {
    /// Create a new store over the shared application pool.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Internal persisted request status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WalletRequestStatus {
    Pending,
    Inflight,
    Confirmed,
    Dropped,
}

impl WalletRequestStatus {
    /// Persisted text representation used by SQL rows.
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Inflight => "inflight",
            Self::Confirmed => "confirmed",
            Self::Dropped => "dropped",
        }
    }

    fn as_lifecycle_status(self) -> WalletRequestLifecycleStatus {
        match self {
            Self::Pending => WalletRequestLifecycleStatus::Pending,
            Self::Inflight => WalletRequestLifecycleStatus::Inflight,
            Self::Confirmed => WalletRequestLifecycleStatus::Confirmed,
            Self::Dropped => WalletRequestLifecycleStatus::Dropped,
        }
    }
}

impl TryFrom<&str> for WalletRequestStatus {
    type Error = ExecutorError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "pending" => Ok(Self::Pending),
            "inflight" => Ok(Self::Inflight),
            "confirmed" => Ok(Self::Confirmed),
            "dropped" => Ok(Self::Dropped),
            other => Err(data_corruption(format!(
                "invalid wallet request status: {other}"
            ))),
        }
    }
}

#[derive(Clone, Debug, FromRow)]
struct WalletRequestRow {
    dedupe_key: String,
    status: String,
    lineage_id: Option<String>,
    batch_txid: Option<String>,
    txid_history: Json<Vec<String>>,
    chain_anchor: Option<Json<ChainAnchor>>,
    payload: Json<WalletRequestKind>,
    created_at: time::OffsetDateTime,
}

#[async_trait]
impl WalletStore for PgBitcoinWalletStore {
    async fn enqueue(
        &self,
        scope: &str,
        request: &WalletRequest,
    ) -> Result<EnqueueWalletRequestResult, ExecutorError> {
        // First try the optimistic insert path so normal enqueue stays cheap.
        let insert = sqlx::query(
            "INSERT INTO bitcoin_wallet_requests (
                scope, dedupe_key, kind, status, payload
             ) VALUES (
                $1, $2, $3, $4, $5
             )
             ON CONFLICT (scope, dedupe_key) DO NOTHING",
        )
        .bind(scope)
        .bind(request.dedupe_key())
        .bind(request_kind_name(request.kind()))
        .bind(WalletRequestStatus::Pending.as_str())
        .bind(Json(request.kind().clone()))
        .execute(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        if insert.rows_affected() == 1 {
            tracing::debug!(
                scope,
                dedupe_key = request.dedupe_key(),
                "bitcoin wallet store enqueued pending request",
            );
            return Ok(EnqueueWalletRequestResult::EnqueuedPending);
        }

        // On conflict, load the existing row and translate its persisted state
        // into the logical enqueue outcome the runner expects.
        let existing = load_wallet_request_row(&self.pool, scope, request.dedupe_key())
            .await?
            .ok_or_else(|| {
                data_corruption(format!(
                    "wallet request disappeared after conflict: {}",
                    request.dedupe_key()
                ))
            })?;

        match WalletRequestStatus::try_from(existing.status.as_str())? {
            WalletRequestStatus::Pending => Ok(EnqueueWalletRequestResult::AlreadyPending),
            WalletRequestStatus::Inflight => Ok(EnqueueWalletRequestResult::AlreadyInflight {
                lineage_id: parse_lineage_id(existing.lineage_id.as_deref())?,
                txid: parse_txid(existing.batch_txid.as_deref(), "batch_txid")?,
            }),
            WalletRequestStatus::Confirmed => Ok(EnqueueWalletRequestResult::AlreadyConfirmed {
                lineage_id: parse_lineage_id(existing.lineage_id.as_deref())?,
                txid: parse_txid(existing.batch_txid.as_deref(), "batch_txid")?,
            }),
            WalletRequestStatus::Dropped => Ok(EnqueueWalletRequestResult::AlreadyDropped),
        }
    }

    async fn restore(&self, scope: &str) -> Result<RestoredWalletState, ExecutorError> {
        // Pending requests restore independently from inflight lineages because
        // inflight rows need to be grouped back into lineage snapshots.
        let pending_rows = sqlx::query_as::<_, WalletRequestRow>(
            "SELECT dedupe_key, status, lineage_id, batch_txid, txid_history, chain_anchor, payload, created_at
             FROM bitcoin_wallet_requests
             WHERE scope = $1 AND status = 'pending'
             ORDER BY dedupe_key ASC",
        )
        .bind(scope)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        let inflight_rows = sqlx::query_as::<_, WalletRequestRow>(
            "SELECT dedupe_key, status, lineage_id, batch_txid, txid_history, chain_anchor, payload, created_at
             FROM bitcoin_wallet_requests
             WHERE scope = $1 AND status = 'inflight'
             ORDER BY lineage_id ASC NULLS LAST, dedupe_key ASC",
        )
        .bind(scope)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        let pending = pending_rows
            .into_iter()
            .map(|row| {
                let chain_anchor = row.chain_anchor.clone().map(|anchor| anchor.0);
                let created_at = Timestamp(row.created_at);
                Ok(PendingWalletRequest {
                    request: row.into_request()?,
                    chain_anchor,
                    created_at,
                })
            })
            .collect::<Result<Vec<_>, ExecutorError>>()?;

        // Rebuild each live lineage by walking inflight rows in lineage order
        // and merging their txid histories into one canonical sequence.
        let mut inflight = Vec::new();
        let mut current: Option<InflightAccumulator> = None;

        for row in inflight_rows {
            let lineage_id = parse_lineage_id(row.lineage_id.as_deref())?;
            let head_txid = parse_txid(row.batch_txid.as_deref(), "batch_txid")?;
            let txid_history = parse_txid_history(&row.txid_history.0)?;
            ensure_inflight_row_history_targets_head(&txid_history, head_txid, &row.dedupe_key)?;
            let request = LiveLineageRequest {
                request: row.clone().into_request()?,
                txid_history: txid_history.clone(),
                created_at: Timestamp(row.created_at),
            };
            let row_anchor = row.chain_anchor.map(|anchor| anchor.0);

            match current.as_mut() {
                Some(accumulator) if accumulator.lineage_id == lineage_id => {
                    // All rows inside one lineage must agree on the current head
                    // and any attached chain anchor.
                    if accumulator.head_txid != head_txid {
                        return Err(data_corruption(format!(
                            "inflight lineage {} has inconsistent head txids",
                            lineage_id
                        )));
                    }
                    if row_anchor.is_some() && accumulator.chain_anchor.is_none() {
                        accumulator.chain_anchor = row_anchor.clone();
                    }
                    if row_anchor.is_some()
                        && accumulator.chain_anchor.as_ref() != row_anchor.as_ref()
                    {
                        return Err(data_corruption(format!(
                            "inflight lineage {} has inconsistent chain anchors",
                            lineage_id
                        )));
                    }
                    merge_ordered_unique_txids(&mut accumulator.all_txids, &txid_history)?;
                    accumulator.requests.push(request);
                },
                Some(_) => {
                    // A new lineage id means the previous accumulator is
                    // complete and a new snapshot should start.
                    inflight.push(current.take().expect("current lineage exists").finish());
                    let mut accumulator =
                        InflightAccumulator::new(lineage_id, head_txid, row_anchor);
                    merge_ordered_unique_txids(&mut accumulator.all_txids, &txid_history)?;
                    accumulator.requests.push(request);
                    current = Some(accumulator);
                },
                None => {
                    // First inflight row starts the first accumulator.
                    let mut accumulator =
                        InflightAccumulator::new(lineage_id, head_txid, row_anchor);
                    merge_ordered_unique_txids(&mut accumulator.all_txids, &txid_history)?;
                    accumulator.requests.push(request);
                    current = Some(accumulator);
                },
            }
        }

        if let Some(accumulator) = current {
            inflight.push(accumulator.finish());
        }

        tracing::info!(
            scope,
            pending = pending.len(),
            inflight = inflight.len(),
            "bitcoin wallet store restored state",
        );

        Ok(RestoredWalletState { pending, inflight })
    }

    async fn resolve_pending(
        &self,
        scope: &str,
        dedupe_key: &str,
    ) -> Result<ResolvePendingWalletRequestResult, ExecutorError> {
        // Output usage: the runner handle returns this to chain-port callers
        // that need a definite answer after async observation timed out. That
        // lets the coordinator distinguish "never submitted" from "already in
        // flight under some txid".
        // Lock the row so "resolve pending" races cannot both mutate the same
        // request lifecycle.
        let mut transaction = self.pool.begin().await.map_err(map_sqlx_error)?;
        let rows =
            load_wallet_request_rows_for_update(&mut transaction, scope, &[dedupe_key.to_string()])
                .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| data_corruption(format!("missing wallet request: {dedupe_key}")))?;

        let result = match WalletRequestStatus::try_from(row.status.as_str())? {
            WalletRequestStatus::Pending | WalletRequestStatus::Dropped => {
                // Treat dropped as cancellable as well so callers can converge
                // on a terminal "not submitted" answer.
                update_rows_to_dropped(&mut transaction, scope, &[dedupe_key.to_string()]).await?;
                ResolvePendingWalletRequestResult::CancelledPending
            },
            WalletRequestStatus::Inflight | WalletRequestStatus::Confirmed => {
                ResolvePendingWalletRequestResult::AlreadySubmitted {
                    txid: parse_txid(row.batch_txid.as_deref(), "batch_txid")?,
                }
            },
        };

        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(result)
    }

    async fn persist_broadcast(
        &self,
        scope: &str,
        plan: &BroadcastPersistencePlan,
    ) -> Result<PersistedBatchReceipt, ExecutorError> {
        // Output usage: the returned receipt is kept by the runner so a later
        // node-level rejection can restore the exact pre-broadcast state via
        // `revert_broadcast`.
        validate_persist_plan(plan)?;
        tracing::info!(
            scope,
            lineage_id = %plan.lineage_id,
            txid = %plan.txid,
            kind = ?plan.kind,
            included = plan.included_request_keys.len(),
            dropped = plan.dropped_request_keys.len(),
            "bitcoin wallet store persisting broadcast",
        );

        let mut transaction = self.pool.begin().await.map_err(map_sqlx_error)?;
        let all_keys = collect_plan_keys(plan);
        // Lock every touched row up front so included/dropped sets are checked
        // and updated atomically with respect to competing runner actions.
        let rows = load_wallet_request_rows_for_update(&mut transaction, scope, &all_keys).await?;
        let rows_by_key = index_wallet_request_rows(rows)?;

        match plan.kind {
            BroadcastPersistenceKind::Fresh | BroadcastPersistenceKind::Chained => {
                if !plan.dropped_request_keys.is_empty() {
                    return Err(data_corruption(
                        "fresh and chained broadcasts cannot drop requests during persistence",
                    ));
                }
                for key in &plan.included_request_keys {
                    let row = rows_by_key
                        .get(key)
                        .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
                    ensure_status(row, WalletRequestStatus::Pending, key)?;
                    ensure_txid_history_append_is_safe(row, plan.txid, key)?;
                }
            },
            BroadcastPersistenceKind::Rbf => {
                // RBF is allowed to mix carried inflight requests with newly
                // pending requests, but confirmed/dropped rows are invalid.
                //
                // Example:
                //   tx_1 held [req_a, req_b]
                //   tx_2 replacement holds [req_a, req_c]
                // Then req_a may already be inflight, req_c may still be
                // pending, and req_b must appear in `dropped_request_keys`.
                for key in &plan.included_request_keys {
                    let row = rows_by_key
                        .get(key)
                        .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
                    match WalletRequestStatus::try_from(row.status.as_str())? {
                        WalletRequestStatus::Pending => {},
                        WalletRequestStatus::Inflight => {
                            ensure_matching_lineage(row, plan.lineage_id, key)?;
                        },
                        WalletRequestStatus::Confirmed | WalletRequestStatus::Dropped => {
                            return Err(data_corruption(format!(
                                "rbf included request {key} has invalid status {}",
                                row.status
                            )));
                        },
                    }
                    ensure_txid_history_append_is_safe(row, plan.txid, key)?;
                }
                for key in &plan.dropped_request_keys {
                    let row = rows_by_key
                        .get(key)
                        .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
                    ensure_status(row, WalletRequestStatus::Inflight, key)?;
                    ensure_matching_lineage(row, plan.lineage_id, key)?;
                }
            },
        }

        let mut snapshots = rows_by_key
            .values()
            .map(snapshot_from_row)
            .collect::<Result<Vec<_>, ExecutorError>>()?;
        snapshots.sort_by(|left, right| left.dedupe_key.cmp(&right.dedupe_key));
        // Snapshot before mutating any rows so revert can restore the exact
        // previous status/lineage/head/history shape if broadcast is rejected.

        // Move included requests onto the new head and append the txid to each
        // request's individual history before any optional drop/requeue step.
        update_rows_to_inflight(
            &mut transaction,
            scope,
            &plan.included_request_keys,
            plan.lineage_id,
            plan.txid,
        )
        .await?;

        if matches!(plan.kind, BroadcastPersistenceKind::Rbf)
            && !plan.dropped_request_keys.is_empty()
        {
            // Requests dropped by the replacement go back to pending so the
            // runner can consider them again on a later tick.
            update_rows_to_pending(&mut transaction, scope, &plan.dropped_request_keys).await?;
        }

        transaction.commit().await.map_err(map_sqlx_error)?;

        Ok(PersistedBatchReceipt {
            lineage_id: plan.lineage_id,
            txid: plan.txid,
            raw_tx_hex: plan.raw_tx_hex.clone(),
            snapshots,
        })
    }

    async fn revert_broadcast(
        &self,
        scope: &str,
        receipt: &PersistedBatchReceipt,
    ) -> Result<(), ExecutorError> {
        // This is the persistence-side inverse of `persist_broadcast`. The
        // runner uses it only when the node rejects the raw transaction after
        // the store already crossed the durable "submission may happen now"
        // boundary.
        validate_revert_receipt(receipt)?;
        tracing::warn!(
            scope,
            lineage_id = %receipt.lineage_id,
            txid = %receipt.txid,
            snapshot_count = receipt.snapshots.len(),
            "bitcoin wallet store reverting broadcast",
        );

        let mut transaction = self.pool.begin().await.map_err(map_sqlx_error)?;
        let keys = receipt
            .snapshots
            .iter()
            .map(|snapshot| snapshot.dedupe_key.clone())
            .collect::<Vec<_>>();
        // Lock all rows before replaying the saved snapshots so revert restores
        // the exact pre-broadcast state atomically.
        load_wallet_request_rows_for_update(&mut transaction, scope, &keys).await?;

        for snapshot in &receipt.snapshots {
            sqlx::query(
                "UPDATE bitcoin_wallet_requests
                 SET status = $3,
                     lineage_id = $4,
                     batch_txid = $5,
                     txid_history = $6::jsonb,
                     chain_anchor = $7,
                     updated_at = now()
                 WHERE scope = $1 AND dedupe_key = $2",
            )
            .bind(scope)
            .bind(&snapshot.dedupe_key)
            .bind(lifecycle_status_as_str(snapshot.status.clone()))
            .bind(snapshot.lineage_id.map(|lineage_id| lineage_id.to_string()))
            .bind(snapshot.batch_txid.map(|txid| txid.to_string()))
            .bind(encode_txid_history(&snapshot.txid_history)?)
            .bind(
                snapshot
                    .chain_anchor
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(|err| {
                        PersistenceError::DataCorruption(format!(
                            "invalid chain_anchor snapshot for {}: {err}",
                            snapshot.dedupe_key
                        ))
                    })?,
            )
            .execute(&mut *transaction)
            .await
            .map_err(map_sqlx_error)?;
        }

        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn persist_confirmation(
        &self,
        scope: &str,
        plan: &ConfirmationPersistencePlan,
    ) -> Result<(), ExecutorError> {
        // Output usage: after this commits, runner recovery and the fee handler
        // both see the same winner. Confirmed rows drive
        // `list_confirmed_lineage_heads`, while orphaned rows re-enter pending
        // planning with an optional chain anchor.
        validate_confirmation_plan(plan)?;
        tracing::info!(
            scope,
            lineage_id = %plan.lineage_id,
            confirmed_txid = %plan.confirmed_txid,
            confirmed = plan.confirmed_request_keys.len(),
            orphaned = plan.orphaned_request_keys.len(),
            anchor_created = plan.chain_anchor.is_some(),
            "bitcoin wallet store persisting confirmation",
        );

        let mut transaction = self.pool.begin().await.map_err(map_sqlx_error)?;
        let all_keys = collect_confirmation_keys(plan);
        // Both confirmed and orphaned requests are locked because the winner
        // selection and any anchor handoff must commit together.
        let rows = load_wallet_request_rows_for_update(&mut transaction, scope, &all_keys).await?;
        let rows_by_key = index_wallet_request_rows(rows)?;

        for key in &plan.confirmed_request_keys {
            let row = rows_by_key
                .get(key)
                .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
            ensure_status(row, WalletRequestStatus::Inflight, key)?;
            ensure_matching_lineage(row, plan.lineage_id, key)?;
        }

        for key in &plan.orphaned_request_keys {
            let row = rows_by_key
                .get(key)
                .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
            ensure_status(row, WalletRequestStatus::Inflight, key)?;
            ensure_matching_lineage(row, plan.lineage_id, key)?;
        }

        update_rows_to_confirmed(
            &mut transaction,
            scope,
            &plan.confirmed_request_keys,
            plan.confirmed_txid,
        )
        .await?;

        if !plan.orphaned_request_keys.is_empty() {
            // Orphaned requests go back to pending, optionally carrying the
            // fresh chain anchor needed for child batches.
            //
            // Example:
            //   tx_2 confirms with reusable change C
            //   req_a confirmed in tx_2
            //   req_b absent from tx_2
            // Then req_b is requeued as pending(anchor=C) so the planner later
            // emits a chained batch that spends from C.
            update_rows_to_pending_with_optional_anchor(
                &mut transaction,
                scope,
                &plan.orphaned_request_keys,
                plan.chain_anchor.as_ref(),
            )
            .await?;
        }

        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }

    async fn list_confirmed_lineage_heads(
        &self,
        scope: &str,
        limit: usize,
    ) -> Result<Vec<ConfirmedLineageHead>, ExecutorError> {
        // Consumed by the Bitcoin fee handler's startup and periodic repair
        // sweeps. One latest confirmed tx per lineage is enough because fee
        // accounting only cares about the currently winning confirmed head.
        #[derive(FromRow)]
        struct ConfirmedHeadRow {
            lineage_id: Option<String>,
            batch_txid: Option<String>,
        }

        let rows = sqlx::query_as::<_, ConfirmedHeadRow>(
            "WITH heads AS (
                 SELECT DISTINCT ON (lineage_id) lineage_id, batch_txid, created_at
                 FROM bitcoin_wallet_requests
                 WHERE scope = $1 AND status = 'confirmed' AND lineage_id IS NOT NULL
                 ORDER BY lineage_id, created_at DESC
             )
             SELECT lineage_id, batch_txid
             FROM heads
             ORDER BY created_at DESC
             LIMIT $2",
        )
        .bind(scope)
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        // The query returns one latest confirmed head per lineage; convert the
        // persisted string columns back into strongly typed ids here.
        rows.into_iter()
            .map(|row| {
                let lineage_id = parse_lineage_id(row.lineage_id.as_deref())?;
                let confirmed_txid = parse_txid(row.batch_txid.as_deref(), "batch_txid")?;
                Ok(ConfirmedLineageHead {
                    lineage_id,
                    confirmed_txid,
                })
            })
            .collect()
    }

    async fn has_submitted_tx(&self, scope: &str, txid: Txid) -> Result<bool, ExecutorError> {
        // Check both the current batch head and the historical txid list so the
        // external reconciler can ignore any tx ever produced by the wallet.
        // This is Bitcoin-only because reconciler classification is address-
        // history based and must distinguish our own wallet batches from truly
        // external deposits/withdrawals.
        let exists = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                SELECT 1
                FROM bitcoin_wallet_requests
                WHERE scope = $1
                  AND (
                    batch_txid = $2
                    OR txid_history @> jsonb_build_array($2::text)
                  )
            )",
        )
        .bind(scope)
        .bind(txid.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(map_sqlx_error)?;

        Ok(exists)
    }

    async fn persist_reconciliation(
        &self,
        scope: &str,
        plan: &ReconciliationPersistencePlan,
    ) -> Result<(), ExecutorError> {
        // Output usage: this commits the runner's missing-lineage conclusion so
        // a restart sees the same survivor/requeue split that the live runner
        // decided.
        validate_reconciliation_plan(plan)?;
        tracing::info!(
            scope,
            lineage_id = %plan.lineage_id,
            kind = ?plan.kind,
            survivors = plan.survivor_request_keys.len(),
            requeued = plan.requeued_request_keys.len(),
            anchor_created = plan.chain_anchor.is_some(),
            "bitcoin wallet store persisting reconciliation",
        );

        let mut transaction = self.pool.begin().await.map_err(map_sqlx_error)?;
        let all_keys = collect_reconciliation_keys(plan);
        // Lock all affected inflight rows so survivor selection and requeueing
        // happen as one atomic lineage transition.
        let rows = load_wallet_request_rows_for_update(&mut transaction, scope, &all_keys).await?;
        let rows_by_key = index_wallet_request_rows(rows)?;

        for key in &plan.survivor_request_keys {
            let row = rows_by_key
                .get(key)
                .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
            ensure_status(row, WalletRequestStatus::Inflight, key)?;
            ensure_matching_lineage(row, plan.lineage_id, key)?;
        }

        for key in &plan.requeued_request_keys {
            let row = rows_by_key
                .get(key)
                .ok_or_else(|| data_corruption(format!("missing wallet request: {key}")))?;
            ensure_status(row, WalletRequestStatus::Inflight, key)?;
            ensure_matching_lineage(row, plan.lineage_id, key)?;
        }

        match &plan.kind {
            ReconciliationPersistenceKind::Confirmed { confirmed_txid } => {
                // A confirmed sibling won; finalize survivors and optionally
                // requeue the rest against the extracted confirmed anchor.
                //
                // Example:
                //   lineage history [tx_1, tx_2, tx_3], head=tx_3 missing
                //   tx_2 is confirmed and req_b never made it into tx_2
                // Then survivors confirm under tx_2 and req_b returns to pending.
                update_rows_to_confirmed(
                    &mut transaction,
                    scope,
                    &plan.survivor_request_keys,
                    *confirmed_txid,
                )
                .await?;

                if !plan.requeued_request_keys.is_empty() {
                    update_rows_to_pending_with_optional_anchor(
                        &mut transaction,
                        scope,
                        &plan.requeued_request_keys,
                        plan.chain_anchor.as_ref(),
                    )
                    .await?;
                }
            },
            ReconciliationPersistenceKind::InMempool { surviving_txid } => {
                // A mempool sibling still survives, so keep those requests
                // inflight but requeue the ones orphaned by newer attempts.
                //
                // This rewrites `batch_txid` back to the surviving sibling
                // without appending history, because the sibling was already
                // part of each survivor's recorded txid_history.
                update_rows_to_surviving_inflight(
                    &mut transaction,
                    scope,
                    &plan.survivor_request_keys,
                    plan.lineage_id,
                    *surviving_txid,
                )
                .await?;

                if !plan.requeued_request_keys.is_empty() {
                    update_rows_to_pending_with_optional_anchor(
                        &mut transaction,
                        scope,
                        &plan.requeued_request_keys,
                        None,
                    )
                    .await?;
                }
            },
            ReconciliationPersistenceKind::NoSurvivor => {
                // Nothing survived, so every request goes back to pending.
                update_rows_to_pending_with_optional_anchor(
                    &mut transaction,
                    scope,
                    &plan.requeued_request_keys,
                    None,
                )
                .await?;
            },
        }

        transaction.commit().await.map_err(map_sqlx_error)?;
        Ok(())
    }
}

#[derive(Debug)]
struct InflightAccumulator {
    lineage_id: LineageId,
    head_txid: Txid,
    all_txids: Vec<Txid>,
    requests: Vec<LiveLineageRequest>,
    chain_anchor: Option<ChainAnchor>,
}

impl InflightAccumulator {
    /// Start rebuilding one inflight lineage snapshot from persisted rows.
    fn new(lineage_id: LineageId, head_txid: Txid, chain_anchor: Option<ChainAnchor>) -> Self {
        Self {
            lineage_id,
            head_txid,
            all_txids: Vec::new(),
            requests: Vec::new(),
            chain_anchor,
        }
    }

    fn finish(self) -> LiveLineageSnapshot {
        LiveLineageSnapshot {
            lineage_id: self.lineage_id,
            head_txid: self.head_txid,
            all_txids: self.all_txids,
            requests: self.requests,
            cover_utxos: Vec::new(),
            chain_anchor: self.chain_anchor,
        }
    }
}

impl WalletRequestRow {
    /// Rehydrate the typed wallet request payload from its persisted row.
    fn into_request(self) -> Result<WalletRequest, ExecutorError> {
        WalletRequest::from_parts(self.dedupe_key, self.payload.0).map_err(|err| {
            PersistenceError::DataCorruption(format!("invalid wallet request payload: {err}"))
                .into()
        })
    }
}

/// Load and lock wallet request rows in dedupe-key order.
async fn load_wallet_request_rows_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
) -> Result<Vec<WalletRequestRow>, ExecutorError> {
    if dedupe_keys.is_empty() {
        return Ok(Vec::new());
    }

    // `FOR UPDATE` is the main race barrier for wallet lifecycle writes. Each
    // persistence method locks all participating rows, validates them, then
    // rewrites them in one transaction.
    let rows = sqlx::query_as::<_, WalletRequestRow>(
        "SELECT dedupe_key, status, lineage_id, batch_txid, txid_history, chain_anchor, payload, created_at
         FROM bitcoin_wallet_requests
         WHERE scope = $1 AND dedupe_key = ANY($2)
         ORDER BY dedupe_key ASC
         FOR UPDATE",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .fetch_all(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    if rows.len() != dedupe_keys.len() {
        return Err(data_corruption(format!(
            "wallet broadcast persistence expected {} rows but loaded {}",
            dedupe_keys.len(),
            rows.len()
        )));
    }

    Ok(rows)
}

/// Load one wallet request row without taking a write lock.
async fn load_wallet_request_row(
    pool: &PgPool,
    scope: &str,
    dedupe_key: &str,
) -> Result<Option<WalletRequestRow>, ExecutorError> {
    Ok(sqlx::query_as::<_, WalletRequestRow>(
        "SELECT dedupe_key, status, lineage_id, batch_txid, txid_history, chain_anchor, payload, created_at
         FROM bitcoin_wallet_requests
         WHERE scope = $1 AND dedupe_key = $2",
    )
    .bind(scope)
    .bind(dedupe_key)
    .fetch_optional(pool)
    .await
    .map_err(map_sqlx_error)?)
}

/// Validate the shape of a broadcast persistence plan before touching storage.
fn validate_persist_plan(plan: &BroadcastPersistencePlan) -> Result<(), ExecutorError> {
    if plan.included_request_keys.is_empty() {
        return Err(data_corruption(
            "wallet broadcast persistence requires at least one included request",
        ));
    }

    ensure_unique_keys("included", &plan.included_request_keys)?;
    ensure_unique_keys("dropped", &plan.dropped_request_keys)?;

    let dropped = plan.dropped_request_keys.iter().collect::<BTreeSet<_>>();
    if plan
        .included_request_keys
        .iter()
        .any(|key| dropped.contains(key))
    {
        return Err(data_corruption(
            "wallet broadcast persistence cannot include and drop the same request",
        ));
    }

    Ok(())
}

/// Validate the shape of a broadcast revert receipt.
fn validate_revert_receipt(receipt: &PersistedBatchReceipt) -> Result<(), ExecutorError> {
    if receipt.snapshots.is_empty() {
        return Err(data_corruption(
            "wallet broadcast revert requires at least one snapshot",
        ));
    }

    let keys = receipt
        .snapshots
        .iter()
        .map(|snapshot| snapshot.dedupe_key.clone())
        .collect::<Vec<_>>();
    ensure_unique_keys("snapshot", &keys)
}

/// Validate the shape of a confirmation persistence plan.
fn validate_confirmation_plan(plan: &ConfirmationPersistencePlan) -> Result<(), ExecutorError> {
    if plan.confirmed_request_keys.is_empty() {
        return Err(data_corruption(
            "wallet confirmation persistence requires at least one confirmed request",
        ));
    }

    ensure_unique_keys("confirmed", &plan.confirmed_request_keys)?;
    ensure_unique_keys("orphaned", &plan.orphaned_request_keys)?;

    let orphaned = plan.orphaned_request_keys.iter().collect::<BTreeSet<_>>();
    if plan
        .confirmed_request_keys
        .iter()
        .any(|key| orphaned.contains(key))
    {
        return Err(data_corruption(
            "wallet confirmation persistence cannot confirm and orphan the same request",
        ));
    }

    match (&plan.chain_anchor, plan.orphaned_request_keys.is_empty()) {
        (Some(_), true) => Err(data_corruption(
            "wallet confirmation persistence cannot set a chain anchor without orphaned requests",
        )),
        _ => Ok(()),
    }
}

/// Validate the shape of a reconciliation persistence plan.
fn validate_reconciliation_plan(plan: &ReconciliationPersistencePlan) -> Result<(), ExecutorError> {
    ensure_unique_keys("survivor", &plan.survivor_request_keys)?;
    ensure_unique_keys("requeued", &plan.requeued_request_keys)?;

    let requeued = plan.requeued_request_keys.iter().collect::<BTreeSet<_>>();
    if plan
        .survivor_request_keys
        .iter()
        .any(|key| requeued.contains(key))
    {
        return Err(data_corruption(
            "wallet reconciliation persistence cannot keep and requeue the same request",
        ));
    }

    match &plan.kind {
        ReconciliationPersistenceKind::Confirmed { .. } => {
            if plan.survivor_request_keys.is_empty() {
                return Err(data_corruption(
                    "wallet reconciliation persistence needs at least one confirmed survivor",
                ));
            }
            Ok(())
        },
        ReconciliationPersistenceKind::InMempool { .. } => {
            if plan.survivor_request_keys.is_empty() {
                return Err(data_corruption(
                    "wallet reconciliation persistence needs at least one inflight survivor",
                ));
            }
            if plan.chain_anchor.is_some() {
                return Err(data_corruption(
                    "wallet reconciliation persistence cannot set a chain anchor for mempool survivors",
                ));
            }
            Ok(())
        },
        ReconciliationPersistenceKind::NoSurvivor => {
            if !plan.survivor_request_keys.is_empty() {
                return Err(data_corruption(
                    "wallet reconciliation persistence cannot keep survivors when no survivor exists",
                ));
            }
            if plan.chain_anchor.is_some() {
                return Err(data_corruption(
                    "wallet reconciliation persistence cannot set a chain anchor without a confirmed survivor",
                ));
            }
            if plan.requeued_request_keys.is_empty() {
                return Err(data_corruption(
                    "wallet reconciliation persistence requires requests to requeue when no survivor exists",
                ));
            }
            Ok(())
        },
    }
}

/// Ensure the request-key list has no duplicates.
fn ensure_unique_keys(label: &str, keys: &[String]) -> Result<(), ExecutorError> {
    let unique = keys.iter().collect::<BTreeSet<_>>();
    if unique.len() != keys.len() {
        return Err(data_corruption(format!(
            "wallet broadcast persistence has duplicate {label} request keys"
        )));
    }
    Ok(())
}

/// Sorted union of every request key touched by a broadcast plan.
fn collect_plan_keys(plan: &BroadcastPersistencePlan) -> Vec<String> {
    let mut keys = plan.included_request_keys.clone();
    keys.extend(plan.dropped_request_keys.clone());
    // Stable ordering keeps row-lock acquisition deterministic across callers.
    keys.sort();
    keys
}

/// Sorted union of every request key touched by a confirmation plan.
fn collect_confirmation_keys(plan: &ConfirmationPersistencePlan) -> Vec<String> {
    let mut keys = plan.confirmed_request_keys.clone();
    keys.extend(plan.orphaned_request_keys.clone());
    // Stable ordering keeps row-lock acquisition deterministic across callers.
    keys.sort();
    keys
}

/// Sorted union of every request key touched by a reconciliation plan.
fn collect_reconciliation_keys(plan: &ReconciliationPersistencePlan) -> Vec<String> {
    let mut keys = plan.survivor_request_keys.clone();
    keys.extend(plan.requeued_request_keys.clone());
    // Stable ordering keeps row-lock acquisition deterministic across callers.
    keys.sort();
    keys
}

/// Index loaded rows by dedupe key while rejecting duplicates.
fn index_wallet_request_rows(
    rows: Vec<WalletRequestRow>,
) -> Result<HashMap<String, WalletRequestRow>, ExecutorError> {
    let mut by_key = HashMap::with_capacity(rows.len());
    for row in rows {
        let dedupe_key = row.dedupe_key.clone();
        if by_key.insert(dedupe_key.clone(), row).is_some() {
            return Err(data_corruption(format!(
                "duplicate wallet request row loaded for {dedupe_key}"
            )));
        }
    }
    Ok(by_key)
}

/// Assert that the row currently has the expected lifecycle status.
fn ensure_status(
    row: &WalletRequestRow,
    expected: WalletRequestStatus,
    dedupe_key: &str,
) -> Result<(), ExecutorError> {
    let actual = WalletRequestStatus::try_from(row.status.as_str())?;
    if actual != expected {
        return Err(data_corruption(format!(
            "wallet request {dedupe_key} has status {}, expected {}",
            row.status,
            expected.as_str()
        )));
    }
    Ok(())
}

/// Assert that the row belongs to the expected lineage.
fn ensure_matching_lineage(
    row: &WalletRequestRow,
    expected: LineageId,
    dedupe_key: &str,
) -> Result<(), ExecutorError> {
    let actual = parse_lineage_id(row.lineage_id.as_deref())?;
    if actual != expected {
        return Err(data_corruption(format!(
            "wallet request {dedupe_key} belongs to lineage {actual}, expected {expected}"
        )));
    }
    Ok(())
}

/// Snapshot the persisted row into the revert payload shape.
fn snapshot_from_row(row: &WalletRequestRow) -> Result<PersistedWalletRequestSnapshot, ExecutorError> {
    Ok(PersistedWalletRequestSnapshot {
        dedupe_key: row.dedupe_key.clone(),
        status: WalletRequestStatus::try_from(row.status.as_str())?.as_lifecycle_status(),
        lineage_id: row
            .lineage_id
            .as_deref()
            .map(|value| parse_lineage_id(Some(value)))
            .transpose()?,
        batch_txid: row
            .batch_txid
            .as_deref()
            .map(|value| parse_txid(Some(value), "batch_txid"))
            .transpose()?,
        txid_history: parse_txid_history(&row.txid_history.0)?,
        chain_anchor: row.chain_anchor.clone().map(|anchor| anchor.0),
    })
}

/// Parse the JSON-encoded txid history column into typed txids.
fn parse_txid_history(encoded_txids: &[String]) -> Result<Vec<Txid>, ExecutorError> {
    encoded_txids
        .iter()
        .map(|encoded_txid| {
            Txid::from_str(encoded_txid)
                .map_err(|err| data_corruption(format!("invalid txid_history entry: {err}")))
        })
        .collect()
}

/// Encode typed txids back into the JSON array stored in Postgres.
fn encode_txid_history(txids: &[Txid]) -> Result<String, ExecutorError> {
    serde_json::to_string(&txids.iter().map(ToString::to_string).collect::<Vec<_>>()).map_err(
        |err| {
            PersistenceError::DataCorruption(format!("failed to encode txid history: {err}")).into()
        },
    )
}

/// Convert lifecycle status into the persisted SQL string.
fn lifecycle_status_as_str(status: WalletRequestLifecycleStatus) -> &'static str {
    match status {
        WalletRequestLifecycleStatus::Pending => "pending",
        WalletRequestLifecycleStatus::Inflight => "inflight",
        WalletRequestLifecycleStatus::Confirmed => "confirmed",
        WalletRequestLifecycleStatus::Dropped => "dropped",
    }
}

/// Persisted request-kind discriminator used for audit/debugging.
fn request_kind_name(kind: &WalletRequestKind) -> &'static str {
    match kind {
        WalletRequestKind::Send(_) => "send",
        WalletRequestKind::Spend(_) => "spend",
    }
}

/// Merge one row's txid history into the lineage-wide canonical order.
///
/// Different requests from the same lineage can store overlapping prefixes or
/// suffixes. This helper rebuilds a single ordered lineage history while
/// rejecting contradictory orderings that would make reconciliation ambiguous.
fn merge_ordered_unique_txids(
    all_txids: &mut Vec<Txid>,
    row_txids: &[Txid],
) -> Result<(), ExecutorError> {
    ensure_unique_txid_history(row_txids)?;

    let mut cursor: Option<usize> = None;
    for txid in row_txids {
        if let Some(existing_index) = all_txids.iter().position(|existing| existing == txid) {
            // Existing txids must appear in a non-decreasing order across rows.
            if let Some(cursor_index) = cursor
                && existing_index < cursor_index
            {
                return Err(data_corruption(
                    "inflight lineage txid histories disagree on submission order",
                ));
            }
            cursor = Some(existing_index);
        } else {
            // New txids are inserted immediately after the last matched txid so
            // overlapping histories stitch together into one canonical order.
            let insert_at = cursor.map_or(0, |index| index + 1);
            all_txids.insert(insert_at, *txid);
            cursor = Some(insert_at);
        }
    }

    Ok(())
}

/// Ensure one txid history does not contain duplicates internally.
fn ensure_unique_txid_history(txids: &[Txid]) -> Result<(), ExecutorError> {
    let unique = txids.iter().collect::<BTreeSet<_>>();
    if unique.len() != txids.len() {
        return Err(data_corruption(
            "wallet request txid_history contains duplicate txids",
        ));
    }
    Ok(())
}

/// Ensure an inflight request's txid history ends at the current stored head.
fn ensure_inflight_row_history_targets_head(
    txid_history: &[Txid],
    head_txid: Txid,
    dedupe_key: &str,
) -> Result<(), ExecutorError> {
    ensure_unique_txid_history(txid_history)?;

    match txid_history.last().copied() {
        Some(last_txid) if last_txid == head_txid => Ok(()),
        Some(last_txid) => Err(data_corruption(format!(
            "inflight request {dedupe_key} ends at txid {last_txid} but head is {head_txid}"
        ))),
        None => Err(data_corruption(format!(
            "inflight request {dedupe_key} has empty txid_history"
        ))),
    }
}

/// Ensure appending `next_txid` would not duplicate an already-recorded txid.
fn ensure_txid_history_append_is_safe(
    row: &WalletRequestRow,
    next_txid: Txid,
    dedupe_key: &str,
) -> Result<(), ExecutorError> {
    let txid_history = parse_txid_history(&row.txid_history.0)?;
    ensure_unique_txid_history(&txid_history)?;

    if txid_history.contains(&next_txid) {
        return Err(data_corruption(format!(
            "wallet request {dedupe_key} already contains txid {next_txid} in txid_history"
        )));
    }

    Ok(())
}

/// Move the given requests onto a new inflight lineage head and append the txid
/// to each request's individual history.
async fn update_rows_to_inflight(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
    lineage_id: LineageId,
    txid: Txid,
) -> Result<(), ExecutorError> {
    // Appending to `txid_history` is what later lets confirmation/reconciliation
    // answer which requests survived into which tx in the lineage.
    sqlx::query(
        "UPDATE bitcoin_wallet_requests
         SET status = 'inflight',
             lineage_id = $3,
             batch_txid = $4,
             txid_history = txid_history || jsonb_build_array($5::text),
             updated_at = now()
         WHERE scope = $1 AND dedupe_key = ANY($2)",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .bind(lineage_id.to_string())
    .bind(txid.to_string())
    .bind(txid.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    Ok(())
}

/// Keep surviving requests inflight on an already-known mempool sibling.
async fn update_rows_to_surviving_inflight(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
    lineage_id: LineageId,
    txid: Txid,
) -> Result<(), ExecutorError> {
    // Keep history intact here. The request is pending again, but we still want
    // audit/recovery code to know which historical txs previously carried it.
    sqlx::query(
        "UPDATE bitcoin_wallet_requests
         SET status = 'inflight',
             lineage_id = $3,
             batch_txid = $4,
             updated_at = now()
         WHERE scope = $1 AND dedupe_key = ANY($2)",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .bind(lineage_id.to_string())
    .bind(txid.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    Ok(())
}

/// Reset requests back to plain pending state.
async fn update_rows_to_pending(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
) -> Result<(), ExecutorError> {
    // This is "same logical request, new planning constraint": the request is
    // pending again, but it may now be pinned to confirmed change for chaining.
    sqlx::query(
        "UPDATE bitcoin_wallet_requests
         SET status = 'pending',
             lineage_id = NULL,
             batch_txid = NULL,
             updated_at = now()
         WHERE scope = $1 AND dedupe_key = ANY($2)",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    Ok(())
}

/// Mark requests as dropped/cancelled before they ever became submitted.
async fn update_rows_to_dropped(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
) -> Result<(), ExecutorError> {
    sqlx::query(
        "UPDATE bitcoin_wallet_requests
         SET status = 'dropped',
             lineage_id = NULL,
             batch_txid = NULL,
             updated_at = now()
         WHERE scope = $1 AND dedupe_key = ANY($2)",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    Ok(())
}

/// Finalize requests as confirmed under the winning txid.
async fn update_rows_to_confirmed(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
    confirmed_txid: Txid,
) -> Result<(), ExecutorError> {
    sqlx::query(
        "UPDATE bitcoin_wallet_requests
         SET status = 'confirmed',
             batch_txid = $3,
             chain_anchor = NULL,
             updated_at = now()
         WHERE scope = $1 AND dedupe_key = ANY($2)",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .bind(confirmed_txid.to_string())
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    Ok(())
}

/// Requeue requests to pending while optionally attaching a confirmed chain
/// anchor for future chained descendants.
async fn update_rows_to_pending_with_optional_anchor(
    transaction: &mut Transaction<'_, Postgres>,
    scope: &str,
    dedupe_keys: &[String],
    chain_anchor: Option<&ChainAnchor>,
) -> Result<(), ExecutorError> {
    sqlx::query(
        "UPDATE bitcoin_wallet_requests
         SET status = 'pending',
             lineage_id = NULL,
             batch_txid = NULL,
             chain_anchor = $3,
             updated_at = now()
         WHERE scope = $1 AND dedupe_key = ANY($2)",
    )
    .bind(scope)
    .bind(dedupe_keys)
    .bind(
        chain_anchor
            .map(serde_json::to_value)
            .transpose()
            .map_err(|err| {
                PersistenceError::DataCorruption(format!("invalid chain_anchor: {err}"))
            })?,
    )
    .execute(&mut **transaction)
    .await
    .map_err(map_sqlx_error)?;

    Ok(())
}

/// Parse a persisted lineage id column.
fn parse_lineage_id(value: Option<&str>) -> Result<LineageId, ExecutorError> {
    let value = value.ok_or_else(|| data_corruption("missing lineage_id"))?;
    let uuid = uuid::Uuid::from_str(value)
        .map_err(|err| data_corruption(format!("invalid lineage_id: {err}")))?;
    Ok(LineageId::from_uuid(uuid))
}

/// Parse a persisted txid column.
fn parse_txid(value: Option<&str>, field_name: &str) -> Result<Txid, ExecutorError> {
    let value = value.ok_or_else(|| data_corruption(format!("missing {field_name}")))?;
    Txid::from_str(value).map_err(|err| data_corruption(format!("invalid {field_name}: {err}")))
}

/// Convenience helper for surfacing repository invariants as data-corruption
/// port errors.
fn data_corruption(message: impl Into<String>) -> ExecutorError {
    PersistenceError::DataCorruption(message.into()).into()
}

#[cfg(all(test, feature = "persistence-store-tests"))]
mod tests {
    use super::{
        ensure_inflight_row_history_targets_head, merge_ordered_unique_txids, PgBitcoinWalletStore,
    };
    use crate::timestamp::Timestamp;
    use crate::infrastructure::chain::bitcoin::wallet::store::{
        BroadcastPersistenceKind, BroadcastPersistencePlan, ConfirmationPersistencePlan,
        ConfirmedLineageHead, EnqueueWalletRequestResult, PersistedWalletRequestSnapshot,
        ReconciliationPersistenceKind, ReconciliationPersistencePlan, WalletRequestLifecycleStatus,
        WalletStore,
    };
    use crate::infrastructure::chain::bitcoin::wallet::{ChainAnchor, LineageId, WalletRequest};
    use crate::infrastructure::persistence::test_support::{setup_test_db, test_suffix};
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, OutPoint, ScriptBuf, Txid};
    use sqlx::Row;

    fn regtest_address() -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[9u8; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn sample_anchor(txid: Txid) -> ChainAnchor {
        ChainAnchor {
            confirmed_txid: txid,
            change_outpoint: OutPoint { txid, vout: 1 },
            change_value: 24_000,
            change_script_pubkey: ScriptBuf::new(),
            confirmed_height: 777,
        }
    }

    fn test_scope() -> String {
        format!("bitcoin_regtest_{}", test_suffix())
    }

    fn sample_txid(seed: u8) -> Txid {
        Txid::from_byte_array([seed; 32])
    }

    #[test]
    fn merge_ordered_unique_txids_builds_canonical_history_from_overlapping_rows() {
        let txid_1 = sample_txid(101);
        let txid_2 = sample_txid(102);
        let txid_3 = sample_txid(103);
        let txid_4 = sample_txid(104);
        let mut all_txids = Vec::new();

        merge_ordered_unique_txids(&mut all_txids, &[txid_2, txid_3]).expect("first merge");
        merge_ordered_unique_txids(&mut all_txids, &[txid_1, txid_2, txid_3])
            .expect("prefix merge");
        merge_ordered_unique_txids(&mut all_txids, &[txid_3, txid_4]).expect("suffix merge");

        assert_eq!(all_txids, vec![txid_1, txid_2, txid_3, txid_4]);
    }

    #[test]
    fn merge_ordered_unique_txids_keeps_existing_order_when_row_is_subset() {
        let txid_1 = sample_txid(105);
        let txid_2 = sample_txid(106);
        let txid_3 = sample_txid(107);
        let mut all_txids = vec![txid_1, txid_2, txid_3];

        merge_ordered_unique_txids(&mut all_txids, &[txid_2, txid_3]).expect("subset merge");

        assert_eq!(all_txids, vec![txid_1, txid_2, txid_3]);
    }

    #[test]
    fn merge_ordered_unique_txids_rejects_contradictory_ordering() {
        let txid_1 = sample_txid(108);
        let txid_2 = sample_txid(109);
        let txid_3 = sample_txid(110);
        let mut all_txids = vec![txid_1, txid_2, txid_3];

        let error = merge_ordered_unique_txids(&mut all_txids, &[txid_2, txid_1, txid_3])
            .expect_err("contradictory ordering must fail");

        assert!(error
            .to_string()
            .contains("txid histories disagree on submission order"));
    }

    #[test]
    fn merge_ordered_unique_txids_rejects_duplicate_txid_inside_single_row_history() {
        let txid_1 = sample_txid(111);
        let txid_2 = sample_txid(112);
        let mut all_txids = Vec::new();

        let error = merge_ordered_unique_txids(&mut all_txids, &[txid_1, txid_2, txid_1])
            .expect_err("duplicate row txids must fail");

        assert!(error
            .to_string()
            .contains("txid_history contains duplicate txids"));
    }

    #[test]
    fn ensure_inflight_row_history_targets_head_accepts_valid_current_head() {
        let txid_1 = sample_txid(113);
        let txid_2 = sample_txid(114);

        ensure_inflight_row_history_targets_head(&[txid_1, txid_2], txid_2, "req-1")
            .expect("history ending at head should be valid");
    }

    #[test]
    fn ensure_inflight_row_history_targets_head_rejects_empty_history() {
        let error = ensure_inflight_row_history_targets_head(&[], sample_txid(115), "req-2")
            .expect_err("empty history must fail");

        assert!(error.to_string().contains("has empty txid_history"));
    }

    #[test]
    fn ensure_inflight_row_history_targets_head_rejects_non_head_tail() {
        let txid_1 = sample_txid(116);
        let txid_2 = sample_txid(117);
        let txid_3 = sample_txid(118);

        let error = ensure_inflight_row_history_targets_head(&[txid_1, txid_2], txid_3, "req-3")
            .expect_err("history tail that differs from head must fail");

        assert!(error.to_string().contains("ends at txid"));
    }

    #[tokio::test]
    async fn enqueue_persists_pending_row_with_empty_txid_history() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let request = WalletRequest::send("send-1", regtest_address(), 12_000).unwrap();

        let result = store
            .enqueue(&scope, &request)
            .await
            .expect("enqueue should succeed");

        assert_eq!(result, EnqueueWalletRequestResult::EnqueuedPending);

        let row = sqlx::query(
            "SELECT kind, status, lineage_id, batch_txid, txid_history, chain_anchor, payload
             FROM bitcoin_wallet_requests
             WHERE scope = $1 AND dedupe_key = $2",
        )
        .bind(&scope)
        .bind("send-1")
        .fetch_one(&pool)
        .await
        .expect("wallet request row should exist");

        let kind: String = row.get("kind");
        let status: String = row.get("status");
        let lineage_id: Option<String> = row.get("lineage_id");
        let batch_txid: Option<String> = row.get("batch_txid");
        let txid_history: serde_json::Value = row.get("txid_history");
        let chain_anchor: Option<serde_json::Value> = row.get("chain_anchor");
        let payload: serde_json::Value = row.get("payload");

        assert_eq!(kind, "send");
        assert_eq!(status, "pending");
        assert_eq!(lineage_id, None);
        assert_eq!(batch_txid, None);
        assert_eq!(txid_history, serde_json::json!([]));
        assert_eq!(chain_anchor, None);
        assert_eq!(payload["kind"], serde_json::json!("send"));
        assert_eq!(payload["amount"], serde_json::json!(12_000));
        assert_eq!(
            payload["address"],
            serde_json::json!(regtest_address().to_string())
        );
    }

    #[tokio::test]
    async fn duplicate_enqueue_returns_existing_logical_state() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let request = WalletRequest::send("dup-1", regtest_address(), 9_000).unwrap();

        let first = store
            .enqueue(&scope, &request)
            .await
            .expect("initial enqueue should succeed");
        assert_eq!(first, EnqueueWalletRequestResult::EnqueuedPending);

        let second = store
            .enqueue(&scope, &request)
            .await
            .expect("duplicate enqueue should succeed");
        assert_eq!(second, EnqueueWalletRequestResult::AlreadyPending);

        let lineage_id = LineageId::new();
        let batch_txid = Txid::from_byte_array([2u8; 32]);
        sqlx::query(
            "UPDATE bitcoin_wallet_requests
             SET status = 'inflight',
                 lineage_id = $3,
                 batch_txid = $4,
                 txid_history = $5::jsonb,
                 updated_at = now()
             WHERE scope = $1 AND dedupe_key = $2",
        )
        .bind(&scope)
        .bind("dup-1")
        .bind(lineage_id.to_string())
        .bind(batch_txid.to_string())
        .bind(serde_json::json!([batch_txid.to_string()]).to_string())
        .execute(&pool)
        .await
        .expect("should update row to inflight");

        let third = store
            .enqueue(&scope, &request)
            .await
            .expect("duplicate inflight enqueue should succeed");
        assert_eq!(
            third,
            EnqueueWalletRequestResult::AlreadyInflight {
                lineage_id,
                txid: batch_txid,
            }
        );
    }

    #[tokio::test]
    async fn list_confirmed_lineage_heads_returns_one_head_per_lineage() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_a = LineageId::new();
        let lineage_b = LineageId::new();
        let txid_a = sample_txid(201);
        let txid_b = sample_txid(202);

        insert_confirmed_send(
            &pool,
            &scope,
            "confirmed-a-1",
            5_000,
            lineage_a,
            txid_a,
            serde_json::json!([txid_a.to_string()]),
        )
        .await;
        insert_confirmed_send(
            &pool,
            &scope,
            "confirmed-a-2",
            6_000,
            lineage_a,
            txid_a,
            serde_json::json!([txid_a.to_string()]),
        )
        .await;
        insert_confirmed_send(
            &pool,
            &scope,
            "confirmed-b-1",
            7_000,
            lineage_b,
            txid_b,
            serde_json::json!([txid_b.to_string()]),
        )
        .await;

        let mut heads = store
            .list_confirmed_lineage_heads(&scope, 128)
            .await
            .expect("head query should succeed");
        heads.sort_by_key(|head| head.lineage_id.to_string());
        let mut expected = vec![
            ConfirmedLineageHead {
                lineage_id: lineage_a,
                confirmed_txid: txid_a,
            },
            ConfirmedLineageHead {
                lineage_id: lineage_b,
                confirmed_txid: txid_b,
            },
        ];
        expected.sort_by_key(|head| head.lineage_id.to_string());

        assert_eq!(heads, expected);
    }

    #[tokio::test]
    async fn has_submitted_tx_returns_true_for_current_batch_txid_and_txid_history() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage = LineageId::new();
        let old_txid = sample_txid(205);
        let current_txid = sample_txid(206);

        insert_confirmed_send(
            &pool,
            &scope,
            "confirmed-a",
            5_000,
            lineage,
            current_txid,
            serde_json::json!([old_txid.to_string(), current_txid.to_string()]),
        )
        .await;

        assert!(store
            .has_submitted_tx(&scope, current_txid)
            .await
            .expect("current batch txid lookup"),);
        assert!(store
            .has_submitted_tx(&scope, old_txid)
            .await
            .expect("historical txid lookup"),);
    }

    #[tokio::test]
    async fn has_submitted_tx_returns_false_for_unknown_txid() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage = LineageId::new();
        let current_txid = sample_txid(207);
        let unknown_txid = sample_txid(208);

        insert_confirmed_send(
            &pool,
            &scope,
            "confirmed-a",
            5_000,
            lineage,
            current_txid,
            serde_json::json!([current_txid.to_string()]),
        )
        .await;

        assert!(!store
            .has_submitted_tx(&scope, unknown_txid)
            .await
            .expect("unknown txid lookup"),);
    }

    #[tokio::test]
    async fn restore_returns_pending_rows_and_groups_inflight_by_lineage_id() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let pending = WalletRequest::send("pending-1", regtest_address(), 5_000).unwrap();

        store
            .enqueue(&scope, &pending)
            .await
            .expect("pending enqueue should succeed");

        let lineage_id = LineageId::new();
        let head_txid = Txid::from_byte_array([3u8; 32]);
        let anchor = sample_anchor(Txid::from_byte_array([4u8; 32]));

        insert_inflight_send(
            &pool,
            &scope,
            "inflight-1",
            7_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            Some(anchor.clone()),
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "inflight-2",
            8_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            Some(anchor.clone()),
        )
        .await;

        let restored = store.restore(&scope).await.expect("restore should succeed");

        assert_eq!(restored.pending.len(), 1);
        assert_eq!(restored.pending[0].request, pending);
        assert!(restored.pending[0].created_at >= Timestamp::default());

        assert_eq!(restored.inflight.len(), 1);
        let lineage = &restored.inflight[0];
        assert_eq!(lineage.lineage_id, lineage_id);
        assert_eq!(lineage.head_txid, head_txid);
        assert_eq!(lineage.chain_anchor.as_ref(), Some(&anchor));
        assert_eq!(lineage.requests.len(), 2);
        assert_eq!(lineage.requests[0].request.dedupe_key(), "inflight-1");
        assert_eq!(lineage.requests[1].request.dedupe_key(), "inflight-2");
        assert!(lineage.cover_utxos.is_empty());
    }

    #[tokio::test]
    async fn restore_unions_txid_history_for_live_lineage_snapshot() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let txid_1 = Txid::from_byte_array([5u8; 32]);
        let txid_2 = Txid::from_byte_array([6u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "history-1",
            11_000,
            lineage_id,
            txid_2,
            serde_json::json!([txid_1.to_string(), txid_2.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "history-2",
            12_000,
            lineage_id,
            txid_2,
            serde_json::json!([txid_2.to_string()]),
            None,
        )
        .await;

        let restored = store.restore(&scope).await.expect("restore should succeed");

        assert_eq!(restored.inflight.len(), 1);
        assert_eq!(restored.inflight[0].all_txids, vec![txid_1, txid_2]);
    }

    #[tokio::test]
    async fn restore_reconstructs_all_txids_in_submission_order_even_when_rows_arrive_out_of_order()
    {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let txid_1 = Txid::from_byte_array([11u8; 32]);
        let txid_2 = Txid::from_byte_array([12u8; 32]);
        let txid_3 = Txid::from_byte_array([13u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "a-shorter-history",
            11_000,
            lineage_id,
            txid_3,
            serde_json::json!([txid_2.to_string(), txid_3.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "z-earlier-history",
            12_000,
            lineage_id,
            txid_3,
            serde_json::json!([txid_1.to_string(), txid_2.to_string(), txid_3.to_string()]),
            None,
        )
        .await;

        let restored = store.restore(&scope).await.expect("restore should succeed");

        assert_eq!(restored.inflight.len(), 1);
        assert_eq!(restored.inflight[0].all_txids, vec![txid_1, txid_2, txid_3]);
    }

    #[tokio::test]
    async fn restore_rejects_inflight_lineage_with_contradictory_txid_history_order() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let txid_1 = Txid::from_byte_array([14u8; 32]);
        let txid_2 = Txid::from_byte_array([15u8; 32]);
        let head_txid = Txid::from_byte_array([16u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "history-a",
            11_000,
            lineage_id,
            head_txid,
            serde_json::json!([
                txid_1.to_string(),
                txid_2.to_string(),
                head_txid.to_string()
            ]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "history-b",
            12_000,
            lineage_id,
            head_txid,
            serde_json::json!([
                txid_2.to_string(),
                txid_1.to_string(),
                head_txid.to_string()
            ]),
            None,
        )
        .await;

        let error = store
            .restore(&scope)
            .await
            .expect_err("contradictory history must fail restore");

        assert!(error
            .to_string()
            .contains("txid histories disagree on submission order"));
    }

    #[tokio::test]
    async fn persist_fresh_broadcast_marks_included_requests_inflight() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let first = WalletRequest::send("fresh-1", regtest_address(), 5_000).unwrap();
        let second = WalletRequest::send("fresh-2", regtest_address(), 7_000).unwrap();
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([31u8; 32]);

        store.enqueue(&scope, &first).await.expect("enqueue first");
        store
            .enqueue(&scope, &second)
            .await
            .expect("enqueue second");

        let receipt = store
            .persist_broadcast(
                &scope,
                &BroadcastPersistencePlan {
                    kind: BroadcastPersistenceKind::Fresh,
                    lineage_id,
                    txid,
                    raw_tx_hex: "deadbeef".to_string(),
                    included_request_keys: vec!["fresh-1".to_string(), "fresh-2".to_string()],
                    dropped_request_keys: Vec::new(),
                },
            )
            .await
            .expect("persist fresh broadcast");

        assert_eq!(receipt.lineage_id, lineage_id);
        assert_eq!(receipt.txid, txid);
        assert_eq!(
            receipt.snapshots,
            vec![
                PersistedWalletRequestSnapshot {
                    dedupe_key: "fresh-1".to_string(),
                    status: WalletRequestLifecycleStatus::Pending,
                    lineage_id: None,
                    batch_txid: None,
                    txid_history: Vec::new(),
                    chain_anchor: None,
                },
                PersistedWalletRequestSnapshot {
                    dedupe_key: "fresh-2".to_string(),
                    status: WalletRequestLifecycleStatus::Pending,
                    lineage_id: None,
                    batch_txid: None,
                    txid_history: Vec::new(),
                    chain_anchor: None,
                },
            ]
        );

        assert_row_state(
            &pool,
            &scope,
            "fresh-1",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(txid),
                txid_history: vec![txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "fresh-2",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(txid),
                txid_history: vec![txid],
                chain_anchor: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_rbf_broadcast_updates_carried_requests_and_requeues_dropped_requests() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let first_txid = Txid::from_byte_array([41u8; 32]);
        let replacement_txid = Txid::from_byte_array([42u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "carry-1",
            9_000,
            lineage_id,
            first_txid,
            serde_json::json!([first_txid.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "drop-1",
            10_000,
            lineage_id,
            first_txid,
            serde_json::json!([first_txid.to_string()]),
            None,
        )
        .await;
        store
            .enqueue(
                &scope,
                &WalletRequest::send("new-1", regtest_address(), 11_000).unwrap(),
            )
            .await
            .expect("enqueue new request");

        store
            .persist_broadcast(
                &scope,
                &BroadcastPersistencePlan {
                    kind: BroadcastPersistenceKind::Rbf,
                    lineage_id,
                    txid: replacement_txid,
                    raw_tx_hex: "deadbeef".to_string(),
                    included_request_keys: vec!["carry-1".to_string(), "new-1".to_string()],
                    dropped_request_keys: vec!["drop-1".to_string()],
                },
            )
            .await
            .expect("persist rbf broadcast");

        assert_row_state(
            &pool,
            &scope,
            "carry-1",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(replacement_txid),
                txid_history: vec![first_txid, replacement_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "new-1",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(replacement_txid),
                txid_history: vec![replacement_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "drop-1",
            TestRowState {
                status: "pending".to_string(),
                lineage_id: None,
                batch_txid: None,
                txid_history: vec![first_txid],
                chain_anchor: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_broadcast_rejects_duplicate_txid_append_for_included_request() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([49u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "dup-1",
            10_000,
            lineage_id,
            txid,
            serde_json::json!([txid.to_string()]),
            None,
        )
        .await;

        let error = store
            .persist_broadcast(
                &scope,
                &BroadcastPersistencePlan {
                    kind: BroadcastPersistenceKind::Rbf,
                    lineage_id,
                    txid,
                    raw_tx_hex: "deadbeef".to_string(),
                    included_request_keys: vec!["dup-1".to_string()],
                    dropped_request_keys: Vec::new(),
                },
            )
            .await
            .expect_err("duplicate txid append must fail");

        assert!(error.to_string().contains("already contains txid"));
    }

    #[tokio::test]
    async fn persist_chained_broadcast_preserves_chain_anchor_for_audit() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([51u8; 32]);
        let anchor = sample_anchor(Txid::from_byte_array([52u8; 32]));

        insert_pending_send(&pool, &scope, "anchored-1", 13_000, Some(anchor.clone())).await;
        store
            .enqueue(
                &scope,
                &WalletRequest::send("fresh-mix", regtest_address(), 14_000).unwrap(),
            )
            .await
            .expect("enqueue mixed fresh request");

        store
            .persist_broadcast(
                &scope,
                &BroadcastPersistencePlan {
                    kind: BroadcastPersistenceKind::Chained,
                    lineage_id,
                    txid,
                    raw_tx_hex: "deadbeef".to_string(),
                    included_request_keys: vec!["anchored-1".to_string(), "fresh-mix".to_string()],
                    dropped_request_keys: Vec::new(),
                },
            )
            .await
            .expect("persist chained broadcast");

        assert_row_state(
            &pool,
            &scope,
            "anchored-1",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(txid),
                txid_history: vec![txid],
                chain_anchor: Some(anchor),
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "fresh-mix",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(txid),
                txid_history: vec![txid],
                chain_anchor: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn revert_broadcast_restores_rbf_rows_exactly() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let first_txid = Txid::from_byte_array([61u8; 32]);
        let replacement_txid = Txid::from_byte_array([62u8; 32]);
        let anchor = sample_anchor(Txid::from_byte_array([63u8; 32]));

        insert_inflight_send(
            &pool,
            &scope,
            "carry-1",
            9_000,
            lineage_id,
            first_txid,
            serde_json::json!([first_txid.to_string()]),
            Some(anchor.clone()),
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "drop-1",
            10_000,
            lineage_id,
            first_txid,
            serde_json::json!([first_txid.to_string()]),
            None,
        )
        .await;
        store
            .enqueue(
                &scope,
                &WalletRequest::send("new-1", regtest_address(), 11_000).unwrap(),
            )
            .await
            .expect("enqueue new request");

        let before = load_snapshots(&pool, &scope, &["carry-1", "drop-1", "new-1"]).await;

        let receipt = store
            .persist_broadcast(
                &scope,
                &BroadcastPersistencePlan {
                    kind: BroadcastPersistenceKind::Rbf,
                    lineage_id,
                    txid: replacement_txid,
                    raw_tx_hex: "deadbeef".to_string(),
                    included_request_keys: vec!["carry-1".to_string(), "new-1".to_string()],
                    dropped_request_keys: vec!["drop-1".to_string()],
                },
            )
            .await
            .expect("persist rbf broadcast");

        store
            .revert_broadcast(&scope, &receipt)
            .await
            .expect("revert broadcast");

        let after = load_snapshots(&pool, &scope, &["carry-1", "drop-1", "new-1"]).await;
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn persist_confirmation_requeues_orphans_with_chain_anchor_and_confirms_winners() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let winner_txid = Txid::from_byte_array([71u8; 32]);
        let head_txid = Txid::from_byte_array([72u8; 32]);
        let anchor = sample_anchor(winner_txid);

        insert_inflight_send(
            &pool,
            &scope,
            "winner-1",
            9_000,
            lineage_id,
            head_txid,
            serde_json::json!([winner_txid.to_string(), head_txid.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "winner-2",
            10_000,
            lineage_id,
            head_txid,
            serde_json::json!([winner_txid.to_string(), head_txid.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "orphan-1",
            11_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            None,
        )
        .await;

        store
            .persist_confirmation(
                &scope,
                &ConfirmationPersistencePlan {
                    lineage_id,
                    confirmed_txid: winner_txid,
                    confirmed_request_keys: vec!["winner-1".to_string(), "winner-2".to_string()],
                    orphaned_request_keys: vec!["orphan-1".to_string()],
                    chain_anchor: Some(anchor.clone()),
                },
            )
            .await
            .expect("persist confirmation");

        assert_row_state(
            &pool,
            &scope,
            "winner-1",
            TestRowState {
                status: "confirmed".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(winner_txid),
                txid_history: vec![winner_txid, head_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "winner-2",
            TestRowState {
                status: "confirmed".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(winner_txid),
                txid_history: vec![winner_txid, head_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "orphan-1",
            TestRowState {
                status: "pending".to_string(),
                lineage_id: None,
                batch_txid: None,
                txid_history: vec![head_txid],
                chain_anchor: Some(anchor),
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_confirmation_without_orphans_does_not_set_chain_anchor() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let confirmed_txid = Txid::from_byte_array([73u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "winner-1",
            12_000,
            lineage_id,
            confirmed_txid,
            serde_json::json!([confirmed_txid.to_string()]),
            None,
        )
        .await;

        store
            .persist_confirmation(
                &scope,
                &ConfirmationPersistencePlan {
                    lineage_id,
                    confirmed_txid,
                    confirmed_request_keys: vec!["winner-1".to_string()],
                    orphaned_request_keys: Vec::new(),
                    chain_anchor: None,
                },
            )
            .await
            .expect("persist no-orphan confirmation");

        assert_row_state(
            &pool,
            &scope,
            "winner-1",
            TestRowState {
                status: "confirmed".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(confirmed_txid),
                txid_history: vec![confirmed_txid],
                chain_anchor: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_reconciliation_confirms_survivor_and_requeues_non_survivors_with_anchor() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let survivor_txid = Txid::from_byte_array([81u8; 32]);
        let head_txid = Txid::from_byte_array([82u8; 32]);
        let anchor = sample_anchor(survivor_txid);

        insert_inflight_send(
            &pool,
            &scope,
            "survivor-1",
            9_000,
            lineage_id,
            head_txid,
            serde_json::json!([survivor_txid.to_string(), head_txid.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "requeue-1",
            10_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            None,
        )
        .await;

        store
            .persist_reconciliation(
                &scope,
                &ReconciliationPersistencePlan {
                    lineage_id,
                    kind: ReconciliationPersistenceKind::Confirmed {
                        confirmed_txid: survivor_txid,
                    },
                    survivor_request_keys: vec!["survivor-1".to_string()],
                    requeued_request_keys: vec!["requeue-1".to_string()],
                    chain_anchor: Some(anchor.clone()),
                },
            )
            .await
            .expect("persist confirmed reconciliation");

        assert_row_state(
            &pool,
            &scope,
            "survivor-1",
            TestRowState {
                status: "confirmed".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(survivor_txid),
                txid_history: vec![survivor_txid, head_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "requeue-1",
            TestRowState {
                status: "pending".to_string(),
                lineage_id: None,
                batch_txid: None,
                txid_history: vec![head_txid],
                chain_anchor: Some(anchor),
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_reconciliation_keeps_mempool_survivor_inflight_and_requeues_non_survivors() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let survivor_txid = Txid::from_byte_array([83u8; 32]);
        let head_txid = Txid::from_byte_array([84u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "survivor-1",
            11_000,
            lineage_id,
            head_txid,
            serde_json::json!([survivor_txid.to_string(), head_txid.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "requeue-1",
            12_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            None,
        )
        .await;

        store
            .persist_reconciliation(
                &scope,
                &ReconciliationPersistencePlan {
                    lineage_id,
                    kind: ReconciliationPersistenceKind::InMempool {
                        surviving_txid: survivor_txid,
                    },
                    survivor_request_keys: vec!["survivor-1".to_string()],
                    requeued_request_keys: vec!["requeue-1".to_string()],
                    chain_anchor: None,
                },
            )
            .await
            .expect("persist mempool reconciliation");

        assert_row_state(
            &pool,
            &scope,
            "survivor-1",
            TestRowState {
                status: "inflight".to_string(),
                lineage_id: Some(lineage_id),
                batch_txid: Some(survivor_txid),
                txid_history: vec![survivor_txid, head_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "requeue-1",
            TestRowState {
                status: "pending".to_string(),
                lineage_id: None,
                batch_txid: None,
                txid_history: vec![head_txid],
                chain_anchor: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_reconciliation_requeues_all_requests_when_no_survivor_exists() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let head_txid = Txid::from_byte_array([85u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "requeue-1",
            13_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            None,
        )
        .await;
        insert_inflight_send(
            &pool,
            &scope,
            "requeue-2",
            14_000,
            lineage_id,
            head_txid,
            serde_json::json!([head_txid.to_string()]),
            None,
        )
        .await;

        store
            .persist_reconciliation(
                &scope,
                &ReconciliationPersistencePlan {
                    lineage_id,
                    kind: ReconciliationPersistenceKind::NoSurvivor,
                    survivor_request_keys: Vec::new(),
                    requeued_request_keys: vec!["requeue-1".to_string(), "requeue-2".to_string()],
                    chain_anchor: None,
                },
            )
            .await
            .expect("persist no-survivor reconciliation");

        assert_row_state(
            &pool,
            &scope,
            "requeue-1",
            TestRowState {
                status: "pending".to_string(),
                lineage_id: None,
                batch_txid: None,
                txid_history: vec![head_txid],
                chain_anchor: None,
            },
        )
        .await;
        assert_row_state(
            &pool,
            &scope,
            "requeue-2",
            TestRowState {
                status: "pending".to_string(),
                lineage_id: None,
                batch_txid: None,
                txid_history: vec![head_txid],
                chain_anchor: None,
            },
        )
        .await;
    }

    #[tokio::test]
    async fn persist_reconciliation_rejects_anchor_for_mempool_survivor() {
        let (pool, _readers, _uow_factory) = setup_test_db().await;
        let store = PgBitcoinWalletStore::new(pool.clone());
        let scope = test_scope();
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([86u8; 32]);

        insert_inflight_send(
            &pool,
            &scope,
            "survivor-1",
            11_000,
            lineage_id,
            txid,
            serde_json::json!([txid.to_string()]),
            None,
        )
        .await;

        let error = store
            .persist_reconciliation(
                &scope,
                &ReconciliationPersistencePlan {
                    lineage_id,
                    kind: ReconciliationPersistenceKind::InMempool {
                        surviving_txid: txid,
                    },
                    survivor_request_keys: vec!["survivor-1".to_string()],
                    requeued_request_keys: Vec::new(),
                    chain_anchor: Some(sample_anchor(txid)),
                },
            )
            .await
            .expect_err("mempool survivor cannot take anchor");

        assert!(error.to_string().contains("cannot set a chain anchor"));
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TestRowState {
        status: String,
        lineage_id: Option<LineageId>,
        batch_txid: Option<Txid>,
        txid_history: Vec<Txid>,
        chain_anchor: Option<ChainAnchor>,
    }

    async fn assert_row_state(
        pool: &sqlx::PgPool,
        scope: &str,
        dedupe_key: &str,
        expected: TestRowState,
    ) {
        let actual = load_row_state(pool, scope, dedupe_key).await;
        assert_eq!(actual, expected);
    }

    async fn load_row_state(pool: &sqlx::PgPool, scope: &str, dedupe_key: &str) -> TestRowState {
        let row = sqlx::query(
            "SELECT status, lineage_id, batch_txid, txid_history, chain_anchor
             FROM bitcoin_wallet_requests
             WHERE scope = $1 AND dedupe_key = $2",
        )
        .bind(scope)
        .bind(dedupe_key)
        .fetch_one(pool)
        .await
        .expect("wallet row should exist");

        let status: String = row.get("status");
        let lineage_id = row
            .get::<Option<String>, _>("lineage_id")
            .map(|value| value.parse::<uuid::Uuid>().expect("lineage uuid"))
            .map(LineageId::from_uuid);
        let batch_txid = row
            .get::<Option<String>, _>("batch_txid")
            .map(|value| value.parse::<Txid>().expect("batch txid"));
        let txid_history = row
            .get::<serde_json::Value, _>("txid_history")
            .as_array()
            .expect("txid history array")
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .expect("txid string")
                    .parse::<Txid>()
                    .expect("valid txid")
            })
            .collect::<Vec<_>>();
        let chain_anchor = row
            .get::<Option<serde_json::Value>, _>("chain_anchor")
            .map(|value| serde_json::from_value(value).expect("anchor json"));

        TestRowState {
            status,
            lineage_id,
            batch_txid,
            txid_history,
            chain_anchor,
        }
    }

    async fn load_snapshots(
        pool: &sqlx::PgPool,
        scope: &str,
        dedupe_keys: &[&str],
    ) -> Vec<TestRowState> {
        let mut states = Vec::new();
        for dedupe_key in dedupe_keys {
            states.push(load_row_state(pool, scope, dedupe_key).await);
        }
        states
    }

    async fn insert_pending_send(
        pool: &sqlx::PgPool,
        scope: &str,
        dedupe_key: &str,
        amount: u64,
        chain_anchor: Option<ChainAnchor>,
    ) {
        sqlx::query(
            "INSERT INTO bitcoin_wallet_requests (
                scope, dedupe_key, kind, status, txid_history, chain_anchor, payload
             ) VALUES (
                $1, $2, 'send', 'pending', '[]'::jsonb, $3, $4
             )",
        )
        .bind(scope)
        .bind(dedupe_key)
        .bind(
            chain_anchor
                .map(serde_json::to_value)
                .transpose()
                .expect("anchor json"),
        )
        .bind(serde_json::json!({
            "kind": "send",
            "address": regtest_address().to_string(),
            "amount": amount,
        }))
        .execute(pool)
        .await
        .expect("should insert pending wallet request");
    }

    async fn insert_inflight_send(
        pool: &sqlx::PgPool,
        scope: &str,
        dedupe_key: &str,
        amount: u64,
        lineage_id: LineageId,
        batch_txid: Txid,
        txid_history: serde_json::Value,
        chain_anchor: Option<ChainAnchor>,
    ) {
        sqlx::query(
            "INSERT INTO bitcoin_wallet_requests (
                scope, dedupe_key, kind, status, lineage_id, batch_txid, txid_history, chain_anchor, payload
             ) VALUES (
                $1, $2, 'send', 'inflight', $3, $4, $5::jsonb, $6, $7
             )",
        )
        .bind(scope)
        .bind(dedupe_key)
        .bind(lineage_id.to_string())
        .bind(batch_txid.to_string())
        .bind(txid_history.to_string())
        .bind(chain_anchor.map(serde_json::to_value).transpose().expect("anchor json"))
        .bind(serde_json::json!({
            "kind": "send",
            "address": regtest_address().to_string(),
            "amount": amount,
        }))
        .execute(pool)
        .await
        .expect("should insert inflight wallet request");
    }

    async fn insert_confirmed_send(
        pool: &sqlx::PgPool,
        scope: &str,
        dedupe_key: &str,
        amount: u64,
        lineage_id: LineageId,
        batch_txid: Txid,
        txid_history: serde_json::Value,
    ) {
        sqlx::query(
            "INSERT INTO bitcoin_wallet_requests (
                scope, dedupe_key, kind, status, lineage_id, batch_txid, txid_history, chain_anchor, payload
             ) VALUES (
                $1, $2, 'send', 'confirmed', $3, $4, $5::jsonb, NULL, $6
             )",
        )
        .bind(scope)
        .bind(dedupe_key)
        .bind(lineage_id.to_string())
        .bind(batch_txid.to_string())
        .bind(txid_history.to_string())
        .bind(serde_json::json!({
            "kind": "send",
            "address": regtest_address().to_string(),
            "amount": amount,
        }))
        .execute(pool)
        .await
        .expect("should insert confirmed wallet request");
    }
}
