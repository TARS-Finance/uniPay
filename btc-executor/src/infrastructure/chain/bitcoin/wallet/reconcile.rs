//! Reconciliation for live lineages whose head disappears from both Electrs and
//! bitcoind.
//!
//! The wallet runtime uses this to recover after mempool eviction, replacement,
//! or restart. It scans the lineage history from newest to oldest to find the
//! surviving confirmed/mempool winner and requeues only the requests that were
//! orphaned by later replacements.
//!
//! Example:
//!
//! ```text
//! lineage txids: [tx_a, tx_b, tx_c]
//! current head : tx_c
//!
//! if tx_c disappears:
//!   - tx_b confirmed -> keep requests whose txid_history contains tx_b
//!   - tx_b mempool   -> keep those requests inflight under tx_b
//!   - none survive   -> requeue every request back to pending
//! ```

use crate::timestamp::Timestamp;
use crate::errors::ExecutorError;
use crate::infrastructure::chain::bitcoin::clients::EsploraTx;
use async_trait::async_trait;
use bitcoin::{ScriptBuf, Txid};

use super::{
    LiveLineage, PendingWalletRequest, ReconciliationPersistenceKind,
    ReconciliationPersistencePlan, WalletRequest, WalletStore, extract_chain_anchor,
};

/// Minimal observer contract needed by missing-lineage reconciliation.
#[async_trait]
pub trait WalletReconciliationObserver: Send + Sync {
    async fn observe_tx(
        &self,
        txid: Txid,
    ) -> Result<crate::infrastructure::chain::bitcoin::wallet::ObservedTxState, ExecutorError>;

    async fn load_tx(&self, txid: Txid) -> Result<EsploraTx, ExecutorError>;
}

/// Surviving state found for a lineage after probing chain/mempool history.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciliationSurvivor {
    Confirmed { txid: Txid },
    InMempool { txid: Txid },
    NoSurvivor,
}

/// Request partition induced by the surviving tx state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationPartition {
    pub survivor: ReconciliationSurvivor,
    pub survivor_requests: Vec<WalletRequest>,
    pub requeued_requests: Vec<WalletRequest>,
}

/// Runtime state that should replace the missing lineage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconciledLineageState {
    Confirmed { confirmed_txid: Txid },
    InMempool { surviving_txid: Txid },
    NoSurvivor,
}

/// Full reconciliation result returned to the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconciliationResult {
    pub state: ReconciledLineageState,
    pub survivor_requests: Vec<WalletRequest>,
    pub requeued_requests: Vec<PendingWalletRequest>,
}

/// Partition requests into surviving versus requeued groups.
pub fn partition_surviving_lineage(
    lineage: &LiveLineage,
    survivor: ReconciliationSurvivor,
) -> ReconciliationPartition {
    match survivor.clone() {
        ReconciliationSurvivor::Confirmed { txid } | ReconciliationSurvivor::InMempool { txid } => {
            let mut survivor_requests = Vec::new();
            let mut requeued_requests = Vec::new();

            for request in &lineage.requests {
                // A request survives if it appeared in the surviving tx's
                // history; otherwise it was orphaned by later replacements.
                if request.txid_history.contains(&txid) {
                    survivor_requests.push(request.request.clone());
                } else {
                    requeued_requests.push(request.request.clone());
                }
            }

            ReconciliationPartition {
                survivor,
                survivor_requests,
                requeued_requests,
            }
        }
        ReconciliationSurvivor::NoSurvivor => ReconciliationPartition {
            survivor,
            survivor_requests: Vec::new(),
            requeued_requests: lineage.wallet_requests(),
        },
    }
}

/// Reconcile a lineage whose current head can no longer be observed.
///
/// The search walks `all_txids` from newest to oldest because later siblings
/// dominate earlier ones for request membership. Once a survivor is found, the
/// function persists that outcome before returning so the runner can recover
/// safely from the next crash or restart.
pub async fn reconcile_missing_lineage<S, O>(
    store: &S,
    observer: &O,
    scope: &str,
    lineage: &LiveLineage,
    current_height: u64,
    chain_anchor_confirmations: u64,
    wallet_script_pubkey: &ScriptBuf,
) -> Result<ReconciliationResult, ExecutorError>
where
    S: WalletStore + ?Sized,
    O: WalletReconciliationObserver + ?Sized,
{
    for candidate_txid in lineage.all_txids.iter().rev().copied() {
        // Walk backward from newest to oldest because the latest surviving
        // sibling should win if multiple historical txids still exist.
        match observer.observe_tx(candidate_txid).await? {
            crate::infrastructure::chain::bitcoin::wallet::ObservedTxState::Confirmed => {
                let partition = partition_surviving_lineage(
                    lineage,
                    ReconciliationSurvivor::Confirmed {
                        txid: candidate_txid,
                    },
                );
                let chain_anchor = if partition.requeued_requests.is_empty() {
                    None
                } else {
                    // Only build a chain anchor when orphaned requests still
                    // need to continue from the newly confirmed winner.
                    let confirmed_tx = observer.load_tx(candidate_txid).await?;
                    let confirmed_height = confirmed_tx.status.block_height.ok_or_else(|| {
                        ExecutorError::Domain(format!(
                            "confirmed tx {candidate_txid} is missing block height"
                        ))
                    })?;
                    let confirmations = current_height
                        .saturating_sub(confirmed_height)
                        .saturating_add(1);
                    if confirmations < chain_anchor_confirmations {
                        // Freshly confirmed winners may still expose useful
                        // change outputs for chained descendants.
                        Some(extract_chain_anchor(
                            candidate_txid,
                            confirmed_height,
                            &confirmed_tx,
                            wallet_script_pubkey,
                        )?)
                    } else {
                        None
                    }
                };

                let plan = ReconciliationPersistencePlan {
                    lineage_id: lineage.lineage_id,
                    kind: ReconciliationPersistenceKind::Confirmed {
                        confirmed_txid: candidate_txid,
                    },
                    survivor_request_keys: partition
                        .survivor_requests
                        .iter()
                        .map(|request| request.dedupe_key().to_string())
                        .collect(),
                    requeued_request_keys: partition
                        .requeued_requests
                        .iter()
                        .map(|request| request.dedupe_key().to_string())
                        .collect(),
                    chain_anchor: chain_anchor.clone(),
                };
                // Persist the winner before returning so the runner can recover
                // the same decision after a crash.
                store.persist_reconciliation(scope, &plan).await?;

                return Ok(ReconciliationResult {
                    state: ReconciledLineageState::Confirmed {
                        confirmed_txid: candidate_txid,
                    },
                    survivor_requests: partition.survivor_requests,
                    requeued_requests: partition
                        .requeued_requests
                        .into_iter()
                        .map(|request| {
                            let dedupe_key = request.dedupe_key().to_string();
                            PendingWalletRequest {
                                request,
                                chain_anchor: chain_anchor.clone(),
                                created_at: created_at_for_request(lineage, &dedupe_key),
                            }
                        })
                        .collect(),
                });
            }
            crate::infrastructure::chain::bitcoin::wallet::ObservedTxState::InMempool => {
                let partition = partition_surviving_lineage(
                    lineage,
                    ReconciliationSurvivor::InMempool {
                        txid: candidate_txid,
                    },
                );
                let plan = ReconciliationPersistencePlan {
                    lineage_id: lineage.lineage_id,
                    kind: ReconciliationPersistenceKind::InMempool {
                        surviving_txid: candidate_txid,
                    },
                    survivor_request_keys: partition
                        .survivor_requests
                        .iter()
                        .map(|request| request.dedupe_key().to_string())
                        .collect(),
                    requeued_request_keys: partition
                        .requeued_requests
                        .iter()
                        .map(|request| request.dedupe_key().to_string())
                        .collect(),
                    chain_anchor: None,
                };
                // For mempool survivors we keep the lineage inflight and only
                // requeue the requests that no longer belong to that tx.
                store.persist_reconciliation(scope, &plan).await?;

                return Ok(ReconciliationResult {
                    state: ReconciledLineageState::InMempool {
                        surviving_txid: candidate_txid,
                    },
                    survivor_requests: partition.survivor_requests,
                    requeued_requests: partition
                        .requeued_requests
                        .into_iter()
                        .map(|request| {
                            let dedupe_key = request.dedupe_key().to_string();
                            PendingWalletRequest {
                                request,
                                chain_anchor: None,
                                created_at: created_at_for_request(lineage, &dedupe_key),
                            }
                        })
                        .collect(),
                });
            }
            crate::infrastructure::chain::bitcoin::wallet::ObservedTxState::Missing => {}
        }
    }

    let partition = partition_surviving_lineage(lineage, ReconciliationSurvivor::NoSurvivor);
    let plan = ReconciliationPersistencePlan {
        lineage_id: lineage.lineage_id,
        kind: ReconciliationPersistenceKind::NoSurvivor,
        survivor_request_keys: Vec::new(),
        requeued_request_keys: partition
            .requeued_requests
            .iter()
            .map(|request| request.dedupe_key().to_string())
            .collect(),
        chain_anchor: None,
    };
    // No surviving tx means every request returns to pending work.
    store.persist_reconciliation(scope, &plan).await?;

    Ok(ReconciliationResult {
        state: ReconciledLineageState::NoSurvivor,
        survivor_requests: Vec::new(),
        requeued_requests: partition
            .requeued_requests
            .into_iter()
            .map(|request| {
                let dedupe_key = request.dedupe_key().to_string();
                PendingWalletRequest {
                    request,
                    chain_anchor: None,
                    created_at: created_at_for_request(lineage, &dedupe_key),
                }
            })
            .collect(),
    })
}

fn created_at_for_request(lineage: &LiveLineage, dedupe_key: &str) -> Timestamp {
    if let Some(created_at) = lineage
        .requests
        .iter()
        .find(|request| request.request.dedupe_key() == dedupe_key)
        .map(|request| request.created_at)
    {
        created_at
    } else {
        // This should not normally happen; preserve progress by requeueing with
        // a fresh timestamp instead of dropping the request entirely.
        tracing::warn!(
            lineage_id = %lineage.lineage_id,
            dedupe_key,
            "reconciliation request missing from lineage metadata; falling back to current timestamp",
        );
        Timestamp::now()
    }
}

#[cfg(test)]
mod tests {
    use crate::timestamp::Timestamp;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, Txid};

    use super::{ReconciliationPartition, ReconciliationSurvivor, partition_surviving_lineage};
    use crate::infrastructure::chain::bitcoin::wallet::{
        LineageId, LiveLineage, LiveLineageRequest, WalletRequest,
    };

    fn regtest_address(seed: u8) -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn send_request(key: &str, seed: u8) -> WalletRequest {
        WalletRequest::send(key, regtest_address(seed), 10_000).expect("wallet request")
    }

    fn live_request(key: &str, seed: u8, txid_history: Vec<Txid>) -> LiveLineageRequest {
        LiveLineageRequest {
            request: send_request(key, seed),
            txid_history,
            created_at: Timestamp::default(),
        }
    }

    #[test]
    fn confirmed_survivor_partitions_requests_by_txid_history() {
        let txid_1 = Txid::from_byte_array([1u8; 32]);
        let txid_2 = Txid::from_byte_array([2u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid: txid_2,
            all_txids: vec![txid_1, txid_2],
            requests: vec![
                live_request("winner", 1, vec![txid_1, txid_2]),
                live_request("orphan", 2, vec![txid_2]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };

        let partition = partition_surviving_lineage(
            &lineage,
            ReconciliationSurvivor::Confirmed { txid: txid_1 },
        );

        assert_eq!(
            partition,
            ReconciliationPartition {
                survivor: ReconciliationSurvivor::Confirmed { txid: txid_1 },
                survivor_requests: vec![send_request("winner", 1)],
                requeued_requests: vec![send_request("orphan", 2)],
            }
        );
    }

    #[test]
    fn mempool_survivor_partitions_requests_by_txid_history() {
        let txid_1 = Txid::from_byte_array([3u8; 32]);
        let txid_2 = Txid::from_byte_array([4u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid: txid_2,
            all_txids: vec![txid_1, txid_2],
            requests: vec![
                live_request("survivor", 1, vec![txid_1]),
                live_request("newer-only", 2, vec![txid_2]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };

        let partition = partition_surviving_lineage(
            &lineage,
            ReconciliationSurvivor::InMempool { txid: txid_1 },
        );

        assert_eq!(
            partition.survivor_requests,
            vec![send_request("survivor", 1)]
        );
        assert_eq!(
            partition.requeued_requests,
            vec![send_request("newer-only", 2)]
        );
    }

    #[test]
    fn no_survivor_requeues_every_request() {
        let txid = Txid::from_byte_array([5u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid: txid,
            all_txids: vec![txid],
            requests: vec![
                live_request("req-1", 1, vec![txid]),
                live_request("req-2", 2, vec![txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };

        let partition = partition_surviving_lineage(&lineage, ReconciliationSurvivor::NoSurvivor);

        assert!(partition.survivor_requests.is_empty());
        assert_eq!(
            partition.requeued_requests,
            vec![send_request("req-1", 1), send_request("req-2", 2)]
        );
    }

    #[test]
    fn created_at_for_request_falls_back_when_request_is_missing() {
        let txid = Txid::from_byte_array([6u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid: txid,
            all_txids: vec![txid],
            requests: vec![live_request("present", 1, vec![txid])],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };
        let before = Timestamp::now();

        let created_at = super::created_at_for_request(&lineage, "missing");

        let after = Timestamp::now();
        assert!(created_at >= before);
        assert!(created_at <= after);
    }
}
