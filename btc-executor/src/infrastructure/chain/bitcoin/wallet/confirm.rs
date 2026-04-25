//! Helpers for turning a confirmed lineage winner into persistence updates.

use bitcoin::{ScriptBuf, Txid};

use super::{LiveLineage, WalletRequest};
use crate::errors::{ChainError, ExecutorError};
use crate::infrastructure::chain::bitcoin::clients::EsploraTx;

/// Partition of requests after a lineage winner is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfirmationPartition {
    pub confirmed_txid: Txid,
    pub confirmed_requests: Vec<WalletRequest>,
    pub orphaned_requests: Vec<WalletRequest>,
}

/// Split a lineage into confirmed winners versus orphaned requests.
///
/// A request is considered confirmed if the winning txid appears anywhere in
/// its `txid_history`, which is how the runtime tracks requests that survived
/// across multiple RBF attempts.
pub fn partition_confirmed_lineage(
    lineage: &LiveLineage,
    confirmed_txid: Txid,
) -> ConfirmationPartition {
    let mut confirmed_requests = Vec::new();
    let mut orphaned_requests = Vec::new();

    for request in &lineage.requests {
        if request.txid_history.contains(&confirmed_txid) {
            confirmed_requests.push(request.request.clone());
        } else {
            orphaned_requests.push(request.request.clone());
        }
    }

    ConfirmationPartition {
        confirmed_txid,
        confirmed_requests,
        orphaned_requests,
    }
}

/// Extract the reusable chain anchor from a confirmed transaction.
///
/// Assumes the transaction has exactly one wallet-owned change output when the
/// runtime still needs to chain follow-up requests onto it. Multiple matching
/// outputs would make the downstream anchor ambiguous, so this function rejects
/// that case explicitly.
pub fn extract_chain_anchor(
    confirmed_txid: Txid,
    confirmed_height: u64,
    confirmed_tx: &EsploraTx,
    wallet_script_pubkey: &ScriptBuf,
) -> Result<super::ChainAnchor, ExecutorError> {
    let matches = confirmed_tx
        .vout
        .iter()
        .enumerate()
        .filter_map(|(index, output)| {
            let script_bytes = hex::decode(&output.scriptpubkey).ok()?;
            let script_pubkey = ScriptBuf::from_bytes(script_bytes);
            (script_pubkey == *wallet_script_pubkey).then_some((
                index as u32,
                output.value,
                script_pubkey,
            ))
        })
        .collect::<Vec<_>>();

    match matches.len() {
        1 => {
            let (vout, value, script_pubkey) = matches.into_iter().next().ok_or_else(|| {
                ExecutorError::Chain(ChainError::ValidationFailed(format!(
                    "confirmed tx {confirmed_txid} change output unexpectedly disappeared"
                )))
            })?;
            Ok(super::ChainAnchor {
                confirmed_txid,
                change_outpoint: bitcoin::OutPoint {
                    txid: confirmed_txid,
                    vout,
                },
                change_value: value,
                change_script_pubkey: script_pubkey,
                confirmed_height,
            })
        },
        0 => Err(ExecutorError::Chain(ChainError::ValidationFailed(format!(
            "confirmed tx {confirmed_txid} has no wallet change output"
        )))),
        _ => Err(ExecutorError::Chain(ChainError::ValidationFailed(format!(
            "confirmed tx {confirmed_txid} has multiple wallet change outputs"
        )))),
    }
}

#[cfg(test)]
mod tests {
    use crate::timestamp::Timestamp;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::ScriptBuf;
    use bitcoin::{Address, Network, Txid};

    use super::{extract_chain_anchor, partition_confirmed_lineage, ConfirmationPartition};
    use crate::infrastructure::chain::bitcoin::clients::{EsploraTx, TxStatus, TxVout};
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

    fn tx_output(script_pubkey: &ScriptBuf, value: u64) -> TxVout {
        TxVout {
            scriptpubkey: hex::encode(script_pubkey.as_bytes()),
            scriptpubkey_address: None,
            value,
        }
    }

    fn confirmed_tx(outputs: Vec<TxVout>) -> EsploraTx {
        EsploraTx {
            txid: "unused".to_string(),
            version: 2,
            locktime: 0,
            vin: Vec::new(),
            vout: outputs,
            size: 200,
            weight: 800,
            fee: 500,
            status: TxStatus {
                confirmed: true,
                block_height: Some(870_000),
                block_hash: None,
                block_time: None,
            },
        }
    }

    #[test]
    fn head_confirmed_marks_all_requests_as_winners_when_all_were_included() {
        let confirmed_txid = Txid::from_byte_array([1u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid: confirmed_txid,
            all_txids: vec![confirmed_txid],
            requests: vec![
                live_request("req-1", 1, vec![confirmed_txid]),
                live_request("req-2", 2, vec![confirmed_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };

        let partition = partition_confirmed_lineage(&lineage, confirmed_txid);

        assert_eq!(
            partition,
            ConfirmationPartition {
                confirmed_txid,
                confirmed_requests: vec![send_request("req-1", 1), send_request("req-2", 2)],
                orphaned_requests: Vec::new(),
            }
        );
    }

    #[test]
    fn earlier_sibling_confirmed_partitions_winners_and_orphans_by_txid_history() {
        let winning_txid = Txid::from_byte_array([2u8; 32]);
        let head_txid = Txid::from_byte_array([3u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![winning_txid, head_txid],
            requests: vec![
                live_request("req-1", 1, vec![winning_txid, head_txid]),
                live_request("req-2", 2, vec![winning_txid, head_txid]),
                live_request("req-3", 3, vec![head_txid]),
                live_request("req-4", 4, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };

        let partition = partition_confirmed_lineage(&lineage, winning_txid);

        assert_eq!(
            partition.confirmed_requests,
            vec![send_request("req-1", 1), send_request("req-2", 2)]
        );
        assert_eq!(
            partition.orphaned_requests,
            vec![send_request("req-3", 3), send_request("req-4", 4)]
        );
    }

    #[test]
    fn requests_not_containing_winning_txid_are_never_treated_as_confirmed() {
        let winning_txid = Txid::from_byte_array([4u8; 32]);
        let head_txid = Txid::from_byte_array([5u8; 32]);
        let lineage = LiveLineage {
            lineage_id: LineageId::new(),
            head_txid,
            all_txids: vec![winning_txid, head_txid],
            requests: vec![
                live_request("winner", 1, vec![winning_txid, head_txid]),
                live_request("orphan-a", 2, vec![head_txid]),
                live_request("orphan-b", 3, vec![head_txid]),
            ],
            cover_utxos: Vec::new(),
            chain_anchor: None,
        };

        let partition = partition_confirmed_lineage(&lineage, winning_txid);

        assert_eq!(
            partition.confirmed_requests,
            vec![send_request("winner", 1)]
        );
        assert_eq!(
            partition.orphaned_requests,
            vec![send_request("orphan-a", 2), send_request("orphan-b", 3)]
        );
    }

    #[test]
    fn extract_chain_anchor_uses_exact_wallet_change_output() {
        let confirmed_txid = Txid::from_byte_array([6u8; 32]);
        let wallet_script_pubkey = regtest_address(9).script_pubkey();
        let tx = confirmed_tx(vec![
            tx_output(&regtest_address(1).script_pubkey(), 15_000),
            tx_output(&wallet_script_pubkey, 24_000),
        ]);

        let anchor = extract_chain_anchor(confirmed_txid, 870_123, &tx, &wallet_script_pubkey)
            .expect("extract anchor");

        assert_eq!(anchor.confirmed_txid, confirmed_txid);
        assert_eq!(anchor.change_outpoint.vout, 1);
        assert_eq!(anchor.change_value, 24_000);
        assert_eq!(anchor.change_script_pubkey, wallet_script_pubkey);
        assert_eq!(anchor.confirmed_height, 870_123);
    }

    #[test]
    fn extract_chain_anchor_errors_when_wallet_change_output_is_missing() {
        let confirmed_txid = Txid::from_byte_array([7u8; 32]);
        let wallet_script_pubkey = regtest_address(9).script_pubkey();
        let tx = confirmed_tx(vec![tx_output(&regtest_address(1).script_pubkey(), 15_000)]);

        let error = extract_chain_anchor(confirmed_txid, 870_124, &tx, &wallet_script_pubkey)
            .expect_err("missing change output must error");

        assert!(error.to_string().contains("has no wallet change output"));
    }

    #[test]
    fn extract_chain_anchor_errors_when_multiple_wallet_change_outputs_exist() {
        let confirmed_txid = Txid::from_byte_array([8u8; 32]);
        let wallet_script_pubkey = regtest_address(9).script_pubkey();
        let tx = confirmed_tx(vec![
            tx_output(&wallet_script_pubkey, 12_000),
            tx_output(&wallet_script_pubkey, 13_000),
        ]);

        let error = extract_chain_anchor(confirmed_txid, 870_125, &tx, &wallet_script_pubkey)
            .expect_err("ambiguous change output must error");

        assert!(error.to_string().contains("multiple wallet change outputs"));
    }
}
