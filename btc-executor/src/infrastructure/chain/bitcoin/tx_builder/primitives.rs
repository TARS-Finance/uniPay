//! Bitcoin-specific types for the transaction builder.
//!
//! The builder reuses the wallet's `SendRequest` / `SpendRequest` types
//! directly so the wallet request model remains the single source of truth.

pub use crate::infrastructure::chain::bitcoin::wallet::CoverUtxo;
use crate::infrastructure::chain::bitcoin::wallet::{
    SendRequest, SpendRequest, WalletRequest, WalletRequestKind,
};
use bitcoin::TapSighashType;

/// Dust limit in satoshis.  Outputs below this value are rejected by
/// Bitcoin Core relay policy.
pub const DUST_LIMIT: u64 = 546;

/// Parameters for constructing a Bitcoin batch transaction.
///
/// Follows the standard-rs pattern with three operation types:
/// - `sacps` — SIGHASH_SINGLE|ANYONECANPAY spends (each input paired 1:1 with an output)
/// - `spends` — SIGHASH_ALL/DEFAULT spends (all inputs commit to all outputs)
/// - `sends` — HTLC initiation outputs
pub struct BitcoinTxAdaptorParams {
    /// SACP spends — `SIGHASH_SINGLE|ANYONECANPAY`. Each input has a 1:1
    /// paired output. Used for instant refund (cooperative cancel).
    pub sacps: Vec<SpendRequest>,
    /// Regular spends — `SIGHASH_ALL`/`Default`. All inputs commit to all
    /// outputs. Used for redeem and refund operations.
    pub spends: Vec<SpendRequest>,
    /// Send outputs — HTLC initiations (send BTC to HTLC P2TR address).
    pub sends: Vec<SendRequest>,
    /// Target fee rate in sat/vB.
    pub fee_rate: f64,
}

impl BitcoinTxAdaptorParams {
    /// Partition wallet requests into adaptor inputs/outputs.
    ///
    /// Assumptions:
    /// - SACP requests are identified exclusively by
    ///   `SIGHASH_SINGLE|ANYONECANPAY`;
    /// - regular spends must not carry a paired recipient output;
    /// - SACP spends may carry fee-adjusted paired outputs reconstructed from
    ///   prebuilt instant-refund transaction bytes.
    pub fn from_requests(
        requests: &[WalletRequest],
        fee_rate: f64,
    ) -> Result<Self, crate::infrastructure::chain::bitcoin::tx_builder::TxBuilderError> {
        let mut sacps = Vec::new();
        let mut spends = Vec::new();
        let mut sends = Vec::new();

        for request in requests {
            match request.kind() {
                WalletRequestKind::Send(send) => sends.push(send.clone()),
                WalletRequestKind::Spend(spend) => {
                    if spend.sighash_type != TapSighashType::SinglePlusAnyoneCanPay
                        && spend.recipient.is_some()
                    {
                        return Err(
                            crate::infrastructure::chain::bitcoin::tx_builder::TxBuilderError::Validation(
                                format!(
                                    "regular spend must not set paired recipient for {}",
                                    request.dedupe_key()
                                ),
                            ),
                        );
                    }

                    if spend.sighash_type == TapSighashType::SinglePlusAnyoneCanPay {
                        sacps.push(spend.clone());
                    } else {
                        spends.push(spend.clone());
                    }
                },
            }
        }

        Ok(Self {
            sacps,
            spends,
            sends,
            fee_rate,
        })
    }
}

pub type WalletTxBuilderParams = BitcoinTxAdaptorParams;

/// RBF context from a previous in-mempool transaction.
///
/// Populated by `BitcoindRpcClient::get_rbf_tx_fee_info()` before building a
/// replacement transaction.  The three fields allow `target()` to compute the
/// minimum fee satisfying BIP-125 rules 3 and 4.
#[derive(Debug, Clone)]
pub struct RbfFeeContext {
    /// Previous transaction's effective fee rate (sat/vB).
    pub previous_fee_rate: f64,
    /// Previous transaction's total fee (sats).
    pub previous_total_fee: u64,
    /// Fee paid by descendant transactions only (sats).
    pub descendant_fee: u64,
}

#[cfg(test)]
mod tests {
    use super::WalletTxBuilderParams;
    use crate::infrastructure::chain::bitcoin::wallet::{SendRequest, WalletRequest};
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Txid, Witness};

    fn regtest_address(seed: u8) -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    #[test]
    fn wallet_requests_map_send_and_regular_spend_into_builder_partitions() {
        let send = WalletRequest::send("send-1", regtest_address(1), 12_000).expect("send");
        let mut witness_template = Witness::new();
        witness_template.push([0u8; 65]);
        witness_template.push([1u8; 32]);
        let spend = WalletRequest::spend(
            "spend-1",
            OutPoint {
                txid: Txid::from_byte_array([2u8; 32]),
                vout: 1,
            },
            35_000,
            regtest_address(2).script_pubkey(),
            witness_template.clone(),
            ScriptBuf::from_bytes(vec![0x51]),
            bitcoin::taproot::TapLeafHash::all_zeros(),
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            TapSighashType::All,
            None,
        )
        .expect("spend");

        let params = WalletTxBuilderParams::from_requests(&[send.clone(), spend.clone()], 2.5)
            .expect("params");

        assert_eq!(params.sends.len(), 1);
        assert_eq!(params.spends.len(), 1);
        assert!(params.sacps.is_empty());
        assert_eq!(params.sends[0].address, regtest_address(1));
        assert_eq!(params.sends[0].amount, 12_000);
        assert_eq!(
            params.spends[0].outpoint,
            OutPoint {
                txid: Txid::from_byte_array([2u8; 32]),
                vout: 1,
            }
        );
        assert_eq!(params.spends[0].witness_template, witness_template);
        assert_eq!(params.spends[0].sighash_type, TapSighashType::All);
    }

    #[test]
    fn wallet_requests_map_sacp_spends_into_sacp_partition() {
        let recipient = regtest_address(4);
        let mut witness_template = Witness::new();
        witness_template.push([0u8; 65]);
        witness_template.push([2u8; 32]);
        let sacp = WalletRequest::spend(
            "sacp-1",
            OutPoint {
                txid: Txid::from_byte_array([3u8; 32]),
                vout: 0,
            },
            27_000,
            regtest_address(5).script_pubkey(),
            witness_template,
            ScriptBuf::from_bytes(vec![0x52]),
            bitcoin::taproot::TapLeafHash::all_zeros(),
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            TapSighashType::SinglePlusAnyoneCanPay,
            Some(SendRequest {
                address: recipient.clone(),
                amount: 27_000,
            }),
        )
        .expect("sacp");

        let params = WalletTxBuilderParams::from_requests(&[sacp], 1.0).expect("params");

        assert_eq!(params.sacps.len(), 1);
        assert!(params.spends.is_empty());
        assert!(params.sends.is_empty());
        assert_eq!(
            params.sacps[0].recipient,
            Some(SendRequest {
                address: recipient,
                amount: 27_000,
            })
        );
        assert_eq!(
            params.sacps[0].outpoint,
            OutPoint {
                txid: Txid::from_byte_array([3u8; 32]),
                vout: 0,
            }
        );
        assert_eq!(
            params.sacps[0].sighash_type,
            TapSighashType::SinglePlusAnyoneCanPay
        );
    }

    #[test]
    fn wallet_requests_partition_is_deterministic_for_mixed_order_input() {
        let send = WalletRequest::send("send-1", regtest_address(1), 11_000).expect("send");
        let regular_outpoint = OutPoint {
            txid: Txid::from_byte_array([6u8; 32]),
            vout: 1,
        };
        let regular = WalletRequest::spend(
            "spend-1",
            regular_outpoint,
            22_000,
            regtest_address(2).script_pubkey(),
            Witness::new(),
            ScriptBuf::from_bytes(vec![0x51]),
            bitcoin::taproot::TapLeafHash::all_zeros(),
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            TapSighashType::All,
            None,
        )
        .expect("regular");
        let sacp_outpoint = OutPoint {
            txid: Txid::from_byte_array([7u8; 32]),
            vout: 0,
        };
        let sacp = WalletRequest::spend(
            "sacp-1",
            sacp_outpoint,
            33_000,
            regtest_address(3).script_pubkey(),
            Witness::new(),
            ScriptBuf::from_bytes(vec![0x52]),
            bitcoin::taproot::TapLeafHash::all_zeros(),
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            TapSighashType::SinglePlusAnyoneCanPay,
            Some(SendRequest {
                address: regtest_address(4),
                amount: 33_000,
            }),
        )
        .expect("sacp");

        let params =
            WalletTxBuilderParams::from_requests(&[send, regular.clone(), sacp.clone()], 1.0)
                .expect("params");

        assert_eq!(params.sacps.len(), 1);
        assert_eq!(params.sacps[0].outpoint, sacp_outpoint);
        assert_eq!(params.spends.len(), 1);
        assert_eq!(params.spends[0].outpoint, regular_outpoint);
        assert_eq!(params.sends.len(), 1);
    }
}
