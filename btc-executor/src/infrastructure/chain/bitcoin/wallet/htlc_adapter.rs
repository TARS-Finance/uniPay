//! Adapter that translates HTLC operations into wallet-runner requests.

use bitcoin::key::XOnlyPublicKey;
use bitcoin::{
    Address, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Transaction, Witness,
};
use std::str::FromStr;
use std::sync::Arc;

use super::{SendRequest, WalletRequest, WalletRequestError};
use crate::infrastructure::chain::bitcoin::clients::{BitcoinClientError, ElectrsClient};
use crate::infrastructure::chain::bitcoin::primitives::{
    BitcoinPrimitivesError, HTLCLeaf, HTLCParams, get_htlc_leaf_hash, get_htlc_leaf_script,
    get_redeem_witness, get_refund_witness,
};

/// Failures while translating HTLC operations into wallet requests.
#[derive(Debug, thiserror::Error)]
pub enum HtlcAdapterError {
    #[error(transparent)]
    Primitives(#[from] BitcoinPrimitivesError),
    #[error(transparent)]
    WalletRequest(#[from] WalletRequestError),
    #[error("electrs request failed: {0}")]
    Client(String),
    #[error("invalid txid in electrs response: {0}")]
    InvalidTxid(String),
    #[error("missing HTLC UTXO for {address} with value {expected_value}")]
    MissingHtlcUtxo {
        address: Address,
        expected_value: u64,
    },
    #[error("ambiguous HTLC UTXO for {address} with value {expected_value}")]
    AmbiguousHtlcUtxo {
        address: Address,
        expected_value: u64,
    },
    #[error("missing HTLC UTXO for outpoint {0}")]
    MissingInstantRefundUtxo(OutPoint),
    #[error("executor signer is not authorized for {0}")]
    UnauthorizedRole(&'static str),
    #[error("failed to decode instant refund tx bytes: {0}")]
    InvalidInstantRefundTx(String),
    #[error("instant refund tx must have the same number of inputs and paired outputs")]
    InvalidInstantRefundShape,
    #[error("instant refund tx input {index} is missing witness element {witness_index}")]
    MissingWitnessElement { index: usize, witness_index: usize },
    #[error("failed to parse instant refund recipient address: {0}")]
    InvalidInstantRefundRecipient(String),
}

/// HTLC actions the wallet adapter knows how to materialize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HtlcAction {
    Initiate {
        dedupe_key: String,
        htlc_address: Address,
        amount: u64,
    },
    Redeem {
        dedupe_key: String,
        htlc_address: Address,
        params: HTLCParams,
        secret: Vec<u8>,
    },
    Refund {
        dedupe_key: String,
        htlc_address: Address,
        params: HTLCParams,
    },
    InstantRefund {
        dedupe_key_prefix: String,
        htlc_address: Address,
        params: HTLCParams,
        recipient: Address,
        instant_refund_tx_hex: String,
    },
}

/// Bridges Bitcoin HTLC semantics with the generic wallet runner.
///
/// The adapter resolves on-chain HTLC UTXOs from Electrs and constructs the
/// correct witness template plus request metadata for the batch builder.
pub struct BitcoinHtlcWalletAdapter {
    executor_pubkey: XOnlyPublicKey,
    electrs: Arc<ElectrsClient>,
    network: Network,
}

impl BitcoinHtlcWalletAdapter {
    /// Create an adapter for the executor key that will sign resulting spends.
    pub fn new(
        executor_pubkey: XOnlyPublicKey,
        electrs: Arc<ElectrsClient>,
        network: Network,
    ) -> Self {
        Self {
            executor_pubkey,
            electrs,
            network,
        }
    }

    /// Translate a high-level HTLC action into a [`WalletRequest`].
    ///
    /// Assumptions:
    /// - the executor key must be authorized for the branch being exercised;
    /// - HTLC UTXO lookup by `address + expected_value` is unique;
    /// - the HTLC has already been funded when redeem/refund-style actions are
    ///   prepared.
    pub async fn prepare(
        &self,
        action: HtlcAction,
    ) -> Result<Vec<WalletRequest>, HtlcAdapterError> {
        match action {
            HtlcAction::Initiate {
                dedupe_key,
                htlc_address,
                amount,
            } => Ok(vec![WalletRequest::send(dedupe_key, htlc_address, amount)?]),
            HtlcAction::Redeem {
                dedupe_key,
                htlc_address,
                params,
                secret,
            } => {
                if self.executor_pubkey != params.redeemer_pubkey {
                    return Err(HtlcAdapterError::UnauthorizedRole("redeem"));
                }
                let (outpoint, value, script_pubkey) =
                    self.resolve_htlc_utxo(&htlc_address, params.amount).await?;
                Ok(vec![WalletRequest::spend(
                    dedupe_key,
                    outpoint,
                    value,
                    script_pubkey,
                    get_redeem_witness(&params, &secret)?,
                    get_htlc_leaf_script(&params, HTLCLeaf::Redeem),
                    get_htlc_leaf_hash(&params, HTLCLeaf::Redeem),
                    Sequence::ENABLE_RBF_NO_LOCKTIME,
                    TapSighashType::All,
                    None,
                )?])
            }
            HtlcAction::Refund {
                dedupe_key,
                htlc_address,
                params,
            } => {
                if self.executor_pubkey != params.initiator_pubkey {
                    return Err(HtlcAdapterError::UnauthorizedRole("refund"));
                }
                let (outpoint, value, script_pubkey) =
                    self.resolve_htlc_utxo(&htlc_address, params.amount).await?;
                Ok(vec![WalletRequest::spend(
                    dedupe_key,
                    outpoint,
                    value,
                    script_pubkey,
                    get_refund_witness(&params)?,
                    get_htlc_leaf_script(&params, HTLCLeaf::Refund),
                    get_htlc_leaf_hash(&params, HTLCLeaf::Refund),
                    Sequence::from_consensus(params.timelock as u32),
                    TapSighashType::All,
                    None,
                )?])
            }
            HtlcAction::InstantRefund {
                dedupe_key_prefix,
                htlc_address,
                params,
                recipient,
                instant_refund_tx_hex,
            } => {
                self.prepare_instant_refund_requests(
                    &dedupe_key_prefix,
                    &htlc_address,
                    &params,
                    &recipient,
                    &instant_refund_tx_hex,
                )
                .await
            }
        }
    }

    async fn prepare_instant_refund_requests(
        &self,
        dedupe_key_prefix: &str,
        htlc_address: &Address,
        params: &HTLCParams,
        recipient: &Address,
        instant_refund_tx_hex: &str,
    ) -> Result<Vec<WalletRequest>, HtlcAdapterError> {
        let our_witness_index = if self.executor_pubkey == params.initiator_pubkey {
            1
        } else if self.executor_pubkey == params.redeemer_pubkey {
            0
        } else {
            return Err(HtlcAdapterError::UnauthorizedRole("instant_refund"));
        };

        let tx_bytes = hex::decode(instant_refund_tx_hex.trim_start_matches("0x"))
            .map_err(|e| HtlcAdapterError::InvalidInstantRefundTx(e.to_string()))?;
        let tx: Transaction = bitcoin::consensus::deserialize(&tx_bytes)
            .map_err(|e| HtlcAdapterError::InvalidInstantRefundTx(e.to_string()))?;

        if tx.input.len() != tx.output.len() {
            return Err(HtlcAdapterError::InvalidInstantRefundShape);
        }

        let live_utxos = self
            .electrs
            .get_address_utxos(&htlc_address.to_string())
            .await
            .map_err(map_client_error)?;
        let utxo_values = live_utxos
            .into_iter()
            .map(|utxo| {
                let txid = bitcoin::Txid::from_str(&utxo.txid)
                    .map_err(|err| HtlcAdapterError::InvalidTxid(err.to_string()))?;
                Ok((
                    OutPoint {
                        txid,
                        vout: utxo.vout,
                    },
                    utxo.value,
                ))
            })
            .collect::<Result<std::collections::HashMap<OutPoint, u64>, HtlcAdapterError>>()?;

        let mut requests = Vec::with_capacity(tx.input.len());
        for (index, input) in tx.input.iter().enumerate() {
            let value = *utxo_values.get(&input.previous_output).ok_or(
                HtlcAdapterError::MissingInstantRefundUtxo(input.previous_output),
            )?;
            let recipient_output = tx
                .output
                .get(index)
                .ok_or(HtlcAdapterError::InvalidInstantRefundShape)?;
            let recipient = recipient_from_output(recipient_output, self.network, recipient)
                .map_err(HtlcAdapterError::InvalidInstantRefundRecipient)?;
            let witness_template =
                blank_witness_signature(&input.witness, index, our_witness_index)?;

            requests.push(WalletRequest::spend(
                format!(
                    "{dedupe_key_prefix}:{}:{}",
                    input.previous_output.txid, input.previous_output.vout
                ),
                input.previous_output,
                value,
                htlc_address.script_pubkey(),
                witness_template,
                get_htlc_leaf_script(params, HTLCLeaf::InstantRefund),
                get_htlc_leaf_hash(params, HTLCLeaf::InstantRefund),
                input.sequence,
                TapSighashType::SinglePlusAnyoneCanPay,
                Some(recipient),
            )?);
        }

        Ok(requests)
    }

    /// Locate the unique HTLC UTXO expected to fund a redeem/refund path.
    ///
    /// Electrs can lag immediately after broadcast, so the lookup retries with
    /// exponential backoff before treating the UTXO as missing.
    async fn resolve_htlc_utxo(
        &self,
        htlc_address: &Address,
        expected_value: u64,
    ) -> Result<(OutPoint, u64, ScriptBuf), HtlcAdapterError> {
        let mut retries = 0;
        let max_retries = 5;
        let mut delay_ms = 200;

        loop {
            let utxos = self
                .electrs
                .get_address_utxos(&htlc_address.to_string())
                .await
                .map_err(map_client_error)?;

            tracing::debug!(
                address = %htlc_address,
                expected_value,
                count = utxos.len(),
                "resolve_htlc_utxo: electrs returned utxos",
            );
            for u in &utxos {
                tracing::debug!(
                    address = %htlc_address,
                    txid = %u.txid,
                    vout = u.vout,
                    value = u.value,
                    confirmed = u.status.confirmed,
                    "resolve_htlc_utxo: candidate utxo",
                );
            }

            let value_matches: Vec<_> = utxos
                .iter()
                .filter(|u| u.value == expected_value)
                .cloned()
                .collect();

            let chosen = match value_matches.len() {
                1 => Some(value_matches.into_iter().next().unwrap()),
                n if n > 1 => {
                    return Err(HtlcAdapterError::AmbiguousHtlcUtxo {
                        address: htlc_address.clone(),
                        expected_value,
                    });
                },
                _ => {
                    if utxos.len() == 1 {
                        let u = utxos[0].clone();
                        tracing::warn!(
                            address = %htlc_address,
                            expected_value,
                            actual_value = u.value,
                            confirmed = u.status.confirmed,
                            "resolve_htlc_utxo: value mismatch, using sole utxo at address (incl. unconfirmed)",
                        );
                        Some(u)
                    } else {
                        None
                    }
                },
            };

            if let Some(utxo) = chosen {
                let txid = bitcoin::Txid::from_str(&utxo.txid)
                    .map_err(|err| HtlcAdapterError::InvalidTxid(err.to_string()))?;
                return Ok((
                    OutPoint {
                        txid,
                        vout: utxo.vout,
                    },
                    utxo.value,
                    htlc_address.script_pubkey(),
                ));
            } else {
                // If not found and still have retries, wait and try again.
                if retries < max_retries {
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    retries += 1;
                    // Exponential backoff, capped to 2 seconds
                    delay_ms = std::cmp::min(delay_ms * 2, 2000);
                    continue;
                } else {
                    return Err(HtlcAdapterError::MissingHtlcUtxo {
                        address: htlc_address.clone(),
                        expected_value,
                    });
                }
            }
        }
    }
}

fn map_client_error(err: BitcoinClientError) -> HtlcAdapterError {
    HtlcAdapterError::Client(err.to_string())
}

fn blank_witness_signature(
    witness: &Witness,
    input_index: usize,
    witness_index: usize,
) -> Result<Witness, HtlcAdapterError> {
    let mut elements: Vec<Vec<u8>> = (0..witness.len())
        .map(|i| witness.nth(i).unwrap_or_default().to_vec())
        .collect();

    let element =
        elements
            .get_mut(witness_index)
            .ok_or(HtlcAdapterError::MissingWitnessElement {
                index: input_index,
                witness_index,
            })?;
    *element = vec![0u8; 65];

    let mut rebuilt = Witness::new();
    for element in elements {
        rebuilt.push(element);
    }
    Ok(rebuilt)
}

fn recipient_from_output(
    output: &bitcoin::TxOut,
    network: Network,
    expected_recipient: &Address,
) -> Result<SendRequest, String> {
    let address = Address::from_script(output.script_pubkey.as_script(), network)
        .map_err(|e| e.to_string())?;
    if address != *expected_recipient {
        return Err(format!(
            "expected recipient {expected_recipient}, got {address}"
        ));
    }

    Ok(SendRequest {
        address,
        amount: output.value.to_sat(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::secp256k1::SecretKey;
    use bitcoin::{Network, Sequence};
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use crate::infrastructure::chain::bitcoin::primitives::HTLCLeaf;
    use crate::infrastructure::chain::bitcoin::wallet::WalletRequestKind;

    fn xonly(seed: u8) -> XOnlyPublicKey {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        xonly
    }

    fn regtest_address(seed: u8) -> Address {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn test_params() -> HTLCParams {
        HTLCParams {
            initiator_pubkey: xonly(1),
            redeemer_pubkey: xonly(2),
            amount: 25_000,
            secret_hash: [9u8; 32],
            timelock: 144,
        }
    }

    fn test_client(server: &MockServer) -> Arc<ElectrsClient> {
        Arc::new(ElectrsClient::new(server.uri()))
    }

    #[tokio::test]
    async fn initiate_maps_to_send_request() {
        let server = MockServer::start().await;
        let adapter =
            BitcoinHtlcWalletAdapter::new(xonly(1), test_client(&server), Network::Regtest);
        let prepared = adapter
            .prepare(HtlcAction::Initiate {
                dedupe_key: "init-1".into(),
                htlc_address: regtest_address(9),
                amount: 25_000,
            })
            .await
            .expect("prepare initiate");
        let prepared = prepared.into_iter().next().expect("single request");

        match prepared.kind() {
            WalletRequestKind::Send(send) => {
                assert_eq!(send.address, regtest_address(9));
                assert_eq!(send.amount, 25_000);
            }
            other => panic!("expected send request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn redeem_maps_to_regular_spend_request() {
        let server = MockServer::start().await;
        let htlc_address = regtest_address(7);
        Mock::given(method("GET"))
            .and(path(format!("/address/{}/utxo", htlc_address)))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"txid":"0101010101010101010101010101010101010101010101010101010101010101","vout":0,"value":25000,"status":{"confirmed":true,"block_height":1,"block_hash":null,"block_time":null}}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let adapter =
            BitcoinHtlcWalletAdapter::new(xonly(2), test_client(&server), Network::Regtest);
        let params = test_params();
        let prepared = adapter
            .prepare(HtlcAction::Redeem {
                dedupe_key: "redeem-1".into(),
                htlc_address: htlc_address.clone(),
                params: params.clone(),
                secret: vec![7u8; 32],
            })
            .await
            .expect("prepare redeem");
        let prepared = prepared.into_iter().next().expect("single request");

        match prepared.kind() {
            WalletRequestKind::Spend(spend) => {
                assert_eq!(spend.outpoint.vout, 0);
                assert_eq!(spend.value, params.amount);
                assert_eq!(spend.script_pubkey, htlc_address.script_pubkey());
                assert_eq!(spend.sequence, Sequence::ENABLE_RBF_NO_LOCKTIME);
                assert_eq!(spend.sighash_type, TapSighashType::All);
                assert!(spend.recipient.is_none());
                assert_eq!(
                    spend.script,
                    crate::infrastructure::chain::bitcoin::primitives::get_htlc_leaf_script(
                        &params,
                        HTLCLeaf::Redeem
                    )
                );
            }
            other => panic!("expected spend request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn refund_maps_to_csv_spend_request() {
        let server = MockServer::start().await;
        let htlc_address = regtest_address(8);
        Mock::given(method("GET"))
            .and(path(format!("/address/{}/utxo", htlc_address)))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"txid":"0202020202020202020202020202020202020202020202020202020202020202","vout":1,"value":25000,"status":{"confirmed":true,"block_height":1,"block_hash":null,"block_time":null}}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let adapter =
            BitcoinHtlcWalletAdapter::new(xonly(1), test_client(&server), Network::Regtest);
        let params = test_params();
        let prepared = adapter
            .prepare(HtlcAction::Refund {
                dedupe_key: "refund-1".into(),
                htlc_address: htlc_address.clone(),
                params: params.clone(),
            })
            .await
            .expect("prepare refund");
        let prepared = prepared.into_iter().next().expect("single request");

        match prepared.kind() {
            WalletRequestKind::Spend(spend) => {
                assert_eq!(spend.value, params.amount);
                assert_eq!(
                    spend.sequence,
                    Sequence::from_consensus(params.timelock as u32)
                );
                assert_eq!(spend.sighash_type, TapSighashType::All);
                assert!(spend.recipient.is_none());
            }
            other => panic!("expected spend request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn instant_refund_maps_to_sacp_spend_request() {
        let server = MockServer::start().await;
        let htlc_address = regtest_address(11);
        Mock::given(method("GET"))
            .and(path(format!("/address/{}/utxo", htlc_address)))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"txid":"0303030303030303030303030303030303030303030303030303030303030303","vout":2,"value":25000,"status":{"confirmed":false,"block_height":null,"block_hash":null,"block_time":null}}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let adapter =
            BitcoinHtlcWalletAdapter::new(xonly(1), test_client(&server), Network::Regtest);
        let params = test_params();
        let recipient = regtest_address(10);
        let mut witness = Witness::new();
        witness.push(vec![4u8; 65]);
        witness.push(vec![4u8; 65]);
        witness.push(
            crate::infrastructure::chain::bitcoin::primitives::get_htlc_leaf_script(
                &params,
                HTLCLeaf::InstantRefund,
            ),
        );
        witness.push(vec![9u8; 33]);
        let tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: OutPoint {
                    txid: bitcoin::Txid::from_str(
                        "0303030303030303030303030303030303030303030303030303030303030303",
                    )
                    .expect("txid"),
                    vout: 2,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness,
            }],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(24_500),
                script_pubkey: recipient.script_pubkey(),
            }],
        };
        let prepared = adapter
            .prepare(HtlcAction::InstantRefund {
                dedupe_key_prefix: "cancel-1".into(),
                htlc_address: regtest_address(11),
                params: params.clone(),
                recipient: recipient.clone(),
                instant_refund_tx_hex: bitcoin::consensus::encode::serialize_hex(&tx),
            })
            .await
            .expect("prepare instant refund");
        let prepared = prepared.into_iter().next().expect("single request");

        match prepared.kind() {
            WalletRequestKind::Spend(spend) => {
                assert_eq!(spend.value, params.amount);
                assert_eq!(spend.sighash_type, TapSighashType::SinglePlusAnyoneCanPay);
                assert_eq!(
                    spend.recipient,
                    Some(SendRequest {
                        address: recipient,
                        amount: 24_500,
                    })
                );
                assert_eq!(spend.witness_template.len(), 4);
                assert_eq!(spend.witness_template.nth(0).unwrap(), &[4u8; 65]);
                assert_eq!(spend.witness_template.nth(1).unwrap(), &[0u8; 65]);
                assert_eq!(
                    spend.script,
                    crate::infrastructure::chain::bitcoin::primitives::get_htlc_leaf_script(
                        &params,
                        HTLCLeaf::InstantRefund
                    )
                );
            }
            other => panic!("expected spend request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn redeem_errors_when_matching_utxo_is_missing() {
        let server = MockServer::start().await;
        let htlc_address = regtest_address(13);
        Mock::given(method("GET"))
            .and(path(format!("/address/{}/utxo", htlc_address)))
            .respond_with(ResponseTemplate::new(200).set_body_raw("[]", "application/json"))
            .mount(&server)
            .await;

        let adapter =
            BitcoinHtlcWalletAdapter::new(xonly(2), test_client(&server), Network::Regtest);
        let err = adapter
            .prepare(HtlcAction::Redeem {
                dedupe_key: "redeem-missing".into(),
                htlc_address: htlc_address.clone(),
                params: test_params(),
                secret: vec![3u8; 32],
            })
            .await
            .unwrap_err();

        match err {
            HtlcAdapterError::MissingHtlcUtxo {
                address,
                expected_value,
            } => {
                assert_eq!(address, htlc_address);
                assert_eq!(expected_value, 25_000);
            }
            other => panic!("expected missing utxo error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn redeem_errors_when_matching_utxo_is_ambiguous() {
        let server = MockServer::start().await;
        let htlc_address = regtest_address(14);
        Mock::given(method("GET"))
            .and(path(format!("/address/{}/utxo", htlc_address)))
            .respond_with(ResponseTemplate::new(200).set_body_raw(
                r#"[{"txid":"0404040404040404040404040404040404040404040404040404040404040404","vout":0,"value":25000,"status":{"confirmed":true,"block_height":1,"block_hash":null,"block_time":null}},{"txid":"0505050505050505050505050505050505050505050505050505050505050505","vout":1,"value":25000,"status":{"confirmed":false,"block_height":null,"block_hash":null,"block_time":null}}]"#,
                "application/json",
            ))
            .mount(&server)
            .await;

        let adapter =
            BitcoinHtlcWalletAdapter::new(xonly(2), test_client(&server), Network::Regtest);
        let err = adapter
            .prepare(HtlcAction::Redeem {
                dedupe_key: "redeem-ambiguous".into(),
                htlc_address: htlc_address.clone(),
                params: test_params(),
                secret: vec![5u8; 32],
            })
            .await
            .unwrap_err();

        match err {
            HtlcAdapterError::AmbiguousHtlcUtxo {
                address,
                expected_value,
            } => {
                assert_eq!(address, htlc_address);
                assert_eq!(expected_value, 25_000);
            }
            other => panic!("expected ambiguous utxo error, got {other:?}"),
        }
    }
}
