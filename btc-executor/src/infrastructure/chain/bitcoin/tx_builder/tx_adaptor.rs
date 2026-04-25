//! Bitcoin implementation of the generic UTXO transaction builder traits.
//!
//! `BitcoinTxAdaptor` implements `UtxoChainTxCodec`, `UtxoChainTxFeeEstimator`,
//! and `UtxoChainTxAdaptor` for Bitcoin Taproot transactions.
//!
//! Transaction layout:
//!   Inputs:  [SACP inputs..., regular spend inputs..., cover inputs...]
//!   Outputs: [SACP paired outputs (1:1)..., spend recipient outputs...,
//!             send outputs..., change]
//!
//! This ordering is part of the adaptor contract:
//!
//! - SACP inputs stay first because each one signs only its paired output.
//! - regular spend inputs sign the full transaction and therefore follow the
//!   SACP section once paired outputs are fixed.
//! - cover inputs are last because they only provide wallet-owned fee funding.

use std::sync::Arc;

use bitcoin::consensus::{deserialize, serialize};
use bitcoin::hashes::Hash;
use bitcoin::sighash::{Prevouts, SighashCache};
use bitcoin::{
    absolute::LockTime, transaction::Version, Amount, ScriptBuf, Sequence,
    TapSighashType, Transaction, TxIn, TxOut, Witness,
};

use super::cover_utxo::BitcoinCoverUtxoProvider;
use super::deps::{
    CoverUtxoProvider, TxBuilderError, UtxoChainTxAdaptor, UtxoChainTxCodec,
    UtxoChainTxFeeEstimator,
};
use super::fee::estimate_vsize;
use super::primitives::{BitcoinTxAdaptorParams, CoverUtxo, RbfFeeContext, DUST_LIMIT};
use crate::infrastructure::keys::BitcoinWallet;

/// P2TR key-path witness placeholder: 65 zero bytes (64-byte Schnorr sig +
/// 1-byte sighash type).  Replaced with real signatures during final signing.
const P2TR_KEYPATH_WITNESS_PLACEHOLDER_LEN: usize = 65;

/// Bitcoin-specific implementation of the generic UTXO transaction builder traits.
///
/// Holds a reference to the wallet (for change addresses) and an optional
/// `RbfFeeContext` that alters how `target()` computes the minimum fee.
pub struct BitcoinTxAdaptor {
    wallet: Arc<BitcoinWallet>,
    network: bitcoin::Network,
    rbf_context: Option<RbfFeeContext>,
}

impl BitcoinTxAdaptor {
    /// Create a new adaptor for fresh (non-RBF) transactions.
    pub fn new(wallet: Arc<BitcoinWallet>, network: bitcoin::Network) -> Self {
        Self {
            wallet,
            network,
            rbf_context: None,
        }
    }

    /// Attach an RBF context for building replacement transactions.
    #[must_use]
    pub fn with_rbf_context(mut self, ctx: Option<RbfFeeContext>) -> Self {
        self.rbf_context = ctx;
        self
    }

    /// Returns the network this adaptor targets.
    #[allow(dead_code)]
    pub fn network(&self) -> bitcoin::Network {
        self.network
    }

    /// Collect all prevouts in input order: [SACP, spends, covers].
    ///
    /// Taproot sighash computation requires all prevouts regardless of
    /// sighash type (even SIGHASH_SINGLE|ANYONECANPAY needs them for
    /// the annex commitment).
    fn collect_all_prevouts(
        &self,
        params: &BitcoinTxAdaptorParams,
        covers: &[CoverUtxo],
    ) -> Vec<TxOut> {
        let mut prevouts = Vec::new();
        // SACP inputs
        for spend in &params.sacps {
            prevouts.push(TxOut {
                value: Amount::from_sat(spend.value),
                script_pubkey: spend.script_pubkey.clone(),
            });
        }
        // Regular spend inputs
        for spend in &params.spends {
            prevouts.push(TxOut {
                value: Amount::from_sat(spend.value),
                script_pubkey: spend.script_pubkey.clone(),
            });
        }
        // Cover inputs
        for utxo in covers {
            prevouts.push(TxOut {
                value: Amount::from_sat(utxo.value),
                script_pubkey: utxo.script_pubkey.clone(),
            });
        }
        prevouts
    }

    /// Sign SACP inputs using Taproot script-path spend with
    /// `SIGHASH_SINGLE|ANYONECANPAY`.
    ///
    /// Each SACP input commits only to its paired output at the same index.
    fn sign_sacp_inputs(
        &self,
        tx: &mut Transaction,
        params: &BitcoinTxAdaptorParams,
        all_prevouts: &[TxOut],
    ) -> Result<(), TxBuilderError> {
        for (input_idx, spend) in params.sacps.iter().enumerate() {
            // Recompute the sighash from the final tx shape so the template
            // witness becomes the fully signed witness used on chain.
            let leaf_hash = spend.leaf_hash;
            let sighash = {
                let mut sighash_cache = SighashCache::new(&*tx);
                sighash_cache
                    .taproot_script_spend_signature_hash(
                        input_idx,
                        &Prevouts::All(all_prevouts),
                        leaf_hash,
                        TapSighashType::SinglePlusAnyoneCanPay,
                    )
                    .map_err(|e| TxBuilderError::Sighash(e.to_string()))?
            };

            let tap_sig = self
                .wallet
                .sign_taproot_script_spend(
                    &sighash.to_raw_hash().to_byte_array(),
                    TapSighashType::SinglePlusAnyoneCanPay,
                )
                .map_err(|e| TxBuilderError::Sighash(e.to_string()))?;

            let mut witness_elements: Vec<Vec<u8>> = (0..spend.witness_template.len())
                .map(|i| spend.witness_template.nth(i).unwrap_or_default().to_vec())
                .collect();

            for elem in &mut witness_elements {
                // The template already contains every non-signature witness
                // element. Only the zeroed signature slot is replaced here.
                if elem.len() == 65 && elem.iter().all(|&b| b == 0) {
                    *elem = tap_sig.serialize().to_vec();
                    break;
                }
            }

            let mut new_witness = Witness::new();
            for elem in &witness_elements {
                new_witness.push(elem);
            }
            tx.input[input_idx].witness = new_witness;
        }
        Ok(())
    }

    /// Sign regular spend inputs using Taproot script-path spend with
    /// `SIGHASH_ALL` / `Default`.
    ///
    /// All regular spend inputs commit to the full set of outputs.
    fn sign_spend_inputs(
        &self,
        tx: &mut Transaction,
        params: &BitcoinTxAdaptorParams,
        sacp_input_count: usize,
        all_prevouts: &[TxOut],
    ) -> Result<(), TxBuilderError> {
        let mut input_idx = sacp_input_count;

        for spend in &params.spends {
            // Spend inputs use the same template-replacement approach, but the
            // sighash type comes from the specific HTLC branch semantics.
            let leaf_hash = spend.leaf_hash;
            let sighash_type = spend.sighash_type;
            let sighash = {
                let mut sighash_cache = SighashCache::new(&*tx);
                sighash_cache
                    .taproot_script_spend_signature_hash(
                        input_idx,
                        &Prevouts::All(all_prevouts),
                        leaf_hash,
                        sighash_type,
                    )
                    .map_err(|e| TxBuilderError::Sighash(e.to_string()))?
            };

            let tap_sig = self
                .wallet
                .sign_taproot_script_spend(&sighash.to_raw_hash().to_byte_array(), sighash_type)
                .map_err(|e| TxBuilderError::Sighash(e.to_string()))?;

            let mut witness_elements: Vec<Vec<u8>> = (0..spend.witness_template.len())
                .map(|i| spend.witness_template.nth(i).unwrap_or_default().to_vec())
                .collect();

            for elem in &mut witness_elements {
                // Keeping the template shape intact means fee estimation and
                // final signing operate over the same witness structure.
                if elem.len() == 65 && elem.iter().all(|&b| b == 0) {
                    *elem = tap_sig.serialize().to_vec();
                    break;
                }
            }

            let mut new_witness = Witness::new();
            for elem in &witness_elements {
                new_witness.push(elem);
            }
            tx.input[input_idx].witness = new_witness;

            input_idx += 1;
        }
        Ok(())
    }

    /// Sign cover inputs using P2TR key-path spend with `SIGHASH_ALL`.
    ///
    /// These are the executor wallet's own fee-paying inputs, so key-path
    /// signing is sufficient and keeps witness size predictable for fee
    /// estimation.
    fn sign_cover_inputs(
        &self,
        tx: &mut Transaction,
        cover_start_idx: usize,
        cover_count: usize,
        all_prevouts: &[TxOut],
    ) -> Result<(), TxBuilderError> {
        for i in 0..cover_count {
            let input_idx = cover_start_idx + i;
            let sighash = {
                let mut sighash_cache = SighashCache::new(&*tx);
                sighash_cache
                    .taproot_key_spend_signature_hash(
                        input_idx,
                        &Prevouts::All(all_prevouts),
                        TapSighashType::All,
                    )
                    .map_err(|e| TxBuilderError::Sighash(e.to_string()))?
            };

            let tap_sig = self
                .wallet
                .sign_taproot_key_spend(&sighash.to_raw_hash().to_byte_array(), TapSighashType::All)
                .map_err(|e| TxBuilderError::Sighash(e.to_string()))?;

            // P2TR key-path witness: just the signature
            let mut witness = Witness::new();
            witness.push(tap_sig.serialize());
            tx.input[input_idx].witness = witness;
        }
        Ok(())
    }

    /// Sign all inputs in a fully-constructed transaction.
    ///
    /// Signing order matches input order: SACP, regular spends, covers.
    pub fn sign(
        &self,
        tx: &mut Transaction,
        params: &BitcoinTxAdaptorParams,
        cover_utxos: &[CoverUtxo],
    ) -> Result<(), TxBuilderError> {
        // All signers share one prevout list so every sighash commits to the
        // exact final ordering chosen by the builder.
        let all_prevouts = self.collect_all_prevouts(params, cover_utxos);

        let sacp_input_count = params.sacps.len();
        let spend_input_count = params.spends.len();
        let cover_start_idx = sacp_input_count + spend_input_count;

        self.sign_sacp_inputs(tx, params, &all_prevouts)?;
        self.sign_spend_inputs(tx, params, sacp_input_count, &all_prevouts)?;
        self.sign_cover_inputs(tx, cover_start_idx, cover_utxos.len(), &all_prevouts)?;

        Ok(())
    }
}

// ── UtxoChainTxCodec ────────────────────────────────────────────────────────

impl UtxoChainTxCodec for BitcoinTxAdaptor {
    type Tx = Transaction;

    fn encode(&self, tx: &Transaction) -> Result<Vec<u8>, TxBuilderError> {
        Ok(serialize(tx))
    }

    fn decode(&self, data: &[u8]) -> Result<Transaction, TxBuilderError> {
        deserialize(data).map_err(|e| TxBuilderError::Consensus(e.to_string()))
    }
}

// ── UtxoChainTxFeeEstimator ────────────────────────────────────────────────

impl UtxoChainTxFeeEstimator for BitcoinTxAdaptor {
    type Params = BitcoinTxAdaptorParams;
    type CoverUtxoProvider = BitcoinCoverUtxoProvider;

    /// Compute the fee currently embedded in the transaction.
    ///
    /// `current_fee = sum(SACP values) + sum(spend values) + sum(cover values) - sum(output values)`
    ///
    /// The result may be negative when cover UTXOs are insufficient.
    fn current(
        &self,
        params: &Self::Params,
        cover: &Self::CoverUtxoProvider,
        tx: &Self::Tx,
    ) -> Result<i64, TxBuilderError> {
        let sacp_value: u64 = params.sacps.iter().map(|s| s.value).sum();

        let spend_value: u64 = params.spends.iter().map(|s| s.value).sum();

        let cover_value: u64 = cover.selected().iter().map(|u| u.value).sum();

        let output_value: u64 = tx.output.iter().map(|o| o.value.to_sat()).sum();

        let input_total = sacp_value as i64 + spend_value as i64 + cover_value as i64;
        Ok(input_total - output_value as i64)
    }

    /// Compute the target fee the transaction should pay.
    ///
    /// - **Fresh tx** (no RBF context): `ceil(vsize * fee_rate)`.
    /// - **RBF replacement**: `max(fee1, fee2, fee3)` where:
    ///   - fee1 = `ceil((prev_fee_rate + 0.001) * vsize)` — strictly higher rate
    ///   - fee2 = `ceil(prev_total_fee + descendant_fee + vsize)` — BIP-125 Rule 3+4
    ///   - fee3 = `ceil(fee_rate * vsize)` — market rate
    fn target(&self, params: &Self::Params, tx: &Self::Tx) -> Result<u64, TxBuilderError> {
        // Add 1 vbyte safety margin: witness finalization (signature replacement)
        // can change the tx weight by a few bytes after the fee builder runs.
        let vsize = estimate_vsize(tx) + 1;

        match &self.rbf_context {
            None => {
                let fee = (vsize as f64 * params.fee_rate).ceil() as u64;
                tracing::debug!(
                    fee_rate = params.fee_rate,
                    vsize,
                    target_fee = fee,
                    "bitcoin tx adaptor computed fresh target fee",
                );
                Ok(fee)
            },
            Some(rbf) => {
                // Replacements must satisfy both market feerate expectations
                // and relay-policy requirements inherited from the old head.
                // Fee 1: strictly higher fee rate than previous tx
                let fee1 = ((rbf.previous_fee_rate + 0.001) * vsize as f64).ceil() as u64;

                // Fee 2: BIP-125 Rule 3 (higher absolute fee) + Rule 4 (incremental relay)
                let fee2 =
                    (rbf.previous_total_fee as f64 + rbf.descendant_fee as f64 + vsize as f64)
                        .ceil() as u64;

                // Fee 3: current market rate
                let fee3 = (params.fee_rate * vsize as f64).ceil() as u64;

                let target_fee = fee1.max(fee2).max(fee3);
                tracing::info!(
                    vsize,
                    market_fee_rate = params.fee_rate,
                    previous_fee_rate = rbf.previous_fee_rate,
                    previous_total_fee = rbf.previous_total_fee,
                    descendant_fee = rbf.descendant_fee,
                    fee1,
                    fee2,
                    fee3,
                    target_fee,
                    "bitcoin tx adaptor computed rbf target fee candidates",
                );
                Ok(target_fee)
            },
        }
    }
}

// ── UtxoChainTxAdaptor ─────────────────────────────────────────────────────

impl UtxoChainTxAdaptor for BitcoinTxAdaptor {
    type Utxo = CoverUtxo;

    /// Build a Bitcoin transaction from SACP spends, regular spends, sends,
    /// cover UTXOs, and a change amount.
    ///
    /// The transaction structure is:
    ///
    /// **Inputs** (in order):
    /// 1. SACP inputs (HTLC spends, sequence = 0xFFFFFFFD for RBF)
    /// 2. Regular spend inputs (HTLC redeems/refunds, sequence = 0xFFFFFFFD)
    /// 3. Cover inputs (wallet UTXOs, sequence = 0xFFFFFFFD)
    ///
    /// **Outputs** (in order):
    /// 1. SACP-paired outputs (1:1 per SACP spend group, value = sum of spend's UTXOs)
    /// 2. Regular spend recipient outputs (one per spend group, value = sum of spend's UTXOs)
    /// 3. Send outputs (HTLC initiations)
    /// 4. Change output (if value > dust limit)
    ///
    /// Witnesses use placeholders: SACP/spend inputs get the pre-built witness
    /// from `SpendRequest`, cover inputs get a 65-byte zero placeholder.
    fn build(
        &self,
        params: &Self::Params,
        cover_utxos: &[Self::Utxo],
        change: u64,
    ) -> Result<Self::Tx, TxBuilderError> {
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        // 1. SACP inputs are inserted first so output pairing by index remains valid.
        for spend in &params.sacps {
            inputs.push(TxIn {
                previous_output: spend.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: spend.sequence,
                witness: spend.witness_template.clone(),
            });
        }

        // 2. Regular spend inputs follow and later sign the full output set.
        for spend in &params.spends {
            inputs.push(TxIn {
                previous_output: spend.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: spend.sequence,
                witness: spend.witness_template.clone(),
            });
        }

        // 3. Each SACP spend gets its paired recipient output at the same index.
        for spend in &params.sacps {
            let recipient = spend.recipient.as_ref().ok_or_else(|| {
                TxBuilderError::Validation(
                    "SACP spend requires a recipient for its paired output".into(),
                )
            })?;
            outputs.push(TxOut {
                value: Amount::from_sat(recipient.amount),
                script_pubkey: recipient.address.script_pubkey(),
            });
        }

        // 4. Regular spends have NO paired outputs — their input value joins the
        //    general pool (covers + change). No outputs created here.

        // 5. Send outputs represent fresh transfers such as new HTLC funding.
        for send in &params.sends {
            outputs.push(TxOut {
                value: Amount::from_sat(send.amount),
                script_pubkey: send.address.script_pubkey(),
            });
        }

        // 6. Cover inputs are appended after outputs are known so their fee-only
        // placeholders can be signed against the final transaction.
        for utxo in cover_utxos {
            let mut placeholder_witness = Witness::new();
            placeholder_witness.push([0u8; P2TR_KEYPATH_WITNESS_PLACEHOLDER_LEN]);
            inputs.push(TxIn {
                previous_output: utxo.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: placeholder_witness,
            });
        }

        // 7. Change is only created above dust; otherwise the residue becomes
        // additional miner fee instead of an uneconomic output.
        if change > DUST_LIMIT {
            outputs.push(TxOut {
                value: Amount::from_sat(change),
                script_pubkey: self.wallet.address().script_pubkey(),
            });
        }

        let mut tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: inputs.clone(),
            output: outputs.clone(),
        };

        self.sign(&mut tx, params, cover_utxos)?;

        Ok(tx)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::chain::bitcoin::wallet::{SendRequest, SpendRequest};
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Keypair, Secp256k1};
    use bitcoin::taproot::TapLeafHash;
    use bitcoin::{Address, Network, OutPoint, Txid};

    const TEST_BTC_PRIVKEY_HEX: &str =
        "e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35";

    fn test_wallet() -> Arc<BitcoinWallet> {
        Arc::new(
            BitcoinWallet::from_private_key(TEST_BTC_PRIVKEY_HEX, Network::Regtest)
                .expect("test wallet"),
        )
    }

    fn test_adaptor() -> BitcoinTxAdaptor {
        BitcoinTxAdaptor::new(test_wallet(), Network::Regtest)
    }

    fn dummy_cover_utxo(value: u64) -> CoverUtxo {
        CoverUtxo {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([0xaa; 32]),
                vout: 0,
            },
            value,
            script_pubkey: test_wallet().address().script_pubkey(),
        }
    }

    /// Create a SACP spend (SIGHASH_SINGLE|ANYONECANPAY) with a witness
    /// containing a 65-byte zero placeholder for the signature.
    fn dummy_sacp_spend(value: u64) -> SpendRequest {
        let secp = Secp256k1::new();
        let keypair = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = keypair.x_only_public_key();
        let address = Address::p2tr(&secp, xonly, None, Network::Regtest);

        let mut witness = Witness::new();
        witness.push([0u8; 65]); // signature placeholder (64 sig + 1 sighash type)
        witness.push([0xcc_u8; 32]); // simulated script data

        SpendRequest {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([0xbb; 32]),
                vout: 0,
            },
            value,
            script_pubkey: address.script_pubkey(),
            witness_template: witness,
            recipient: Some(SendRequest {
                address: address.clone(),
                amount: value,
            }),
            script: ScriptBuf::new(),
            leaf_hash: TapLeafHash::all_zeros(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
        }
    }

    /// Create a regular spend (SIGHASH_ALL) with a witness containing a
    /// 64-byte zero placeholder for the signature.
    fn dummy_regular_spend(value: u64) -> SpendRequest {
        let secp = Secp256k1::new();
        let keypair = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = keypair.x_only_public_key();
        let address = Address::p2tr(&secp, xonly, None, Network::Regtest);

        let mut witness = Witness::new();
        witness.push([0u8; 65]); // signature placeholder (64 sig + 1 sighash type)
        witness.push([0xdd_u8; 32]); // simulated script data

        SpendRequest {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([0xcc; 32]),
                vout: 0,
            },
            value,
            script_pubkey: address.script_pubkey(),
            witness_template: witness,
            recipient: None, // Regular spend: no paired output
            script: ScriptBuf::new(),
            leaf_hash: TapLeafHash::all_zeros(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            sighash_type: TapSighashType::All,
        }
    }

    fn empty_params() -> BitcoinTxAdaptorParams {
        BitcoinTxAdaptorParams {
            sacps: vec![],
            spends: vec![],
            sends: vec![],
            fee_rate: 5.0,
        }
    }

    // ── Build tests ────────────────────────────────────────────────────────

    #[test]
    fn codec_roundtrip() {
        let adaptor = test_adaptor();
        let params = empty_params();
        let tx = adaptor
            .build(&params, &[dummy_cover_utxo(10_000)], 0)
            .expect("build");
        let encoded = adaptor.encode(&tx).expect("encode");
        let decoded = adaptor.decode(&encoded).expect("decode");
        assert_eq!(tx.compute_txid(), decoded.compute_txid());
    }

    #[test]
    fn build_with_sends_only() {
        let adaptor = test_adaptor();
        let secp = Secp256k1::new();
        let kp = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = kp.x_only_public_key();
        let addr = Address::p2tr(&secp, xonly, None, Network::Regtest);

        let mut params = empty_params();
        params.sends = vec![SendRequest {
            address: addr,
            amount: 5_000,
        }];
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 4_000).expect("build");

        // 1 cover input
        assert_eq!(tx.input.len(), 1);
        // 1 send output + 1 change output (4000 > DUST_LIMIT)
        assert_eq!(tx.output.len(), 2);
    }

    #[test]
    fn build_with_sacp_spends() {
        let adaptor = test_adaptor();
        let spend = dummy_sacp_spend(50_000);

        let mut params = empty_params();
        params.sacps = vec![spend];
        let covers = vec![dummy_cover_utxo(1_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        // 1 SACP input + 1 cover input
        assert_eq!(tx.input.len(), 2);
        // 1 SACP-paired output (no change since change=0 <= DUST_LIMIT)
        assert_eq!(tx.output.len(), 1);
        // SACP output value should be the HTLC UTXO value
        assert_eq!(tx.output[0].value.to_sat(), 50_000);
    }

    #[test]
    fn build_with_regular_spends_no_output() {
        // Regular spends don't produce outputs — value joins the pool
        let adaptor = test_adaptor();
        let spend = dummy_regular_spend(50_000);

        let mut params = empty_params();
        params.spends = vec![spend];
        let covers = vec![dummy_cover_utxo(1_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        // 1 spend input + 1 cover input
        assert_eq!(tx.input.len(), 2);
        // No outputs (spend value goes to pool, change=0 ≤ dust)
        assert_eq!(tx.output.len(), 0);
    }

    #[test]
    fn change_output_omitted_below_dust() {
        let adaptor = test_adaptor();
        let params = empty_params();
        let covers = vec![dummy_cover_utxo(10_000)];
        // change = 500, which is <= DUST_LIMIT (546)
        let tx = adaptor.build(&params, &covers, 500).expect("build");
        // No outputs (no sends, no spends, change below dust)
        assert_eq!(tx.output.len(), 0);
    }

    #[test]
    fn change_output_included_above_dust() {
        let adaptor = test_adaptor();
        let params = empty_params();
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 1_000).expect("build");
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].value.to_sat(), 1_000);
    }

    #[test]
    fn current_fee_with_sacp_only() {
        let adaptor = test_adaptor();
        let spend = dummy_sacp_spend(50_000);

        let mut params = empty_params();
        params.sacps = vec![spend];
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        let cover_provider = BitcoinCoverUtxoProvider::new_with_selected(
            test_wallet().address().clone(),
            covers,
            None,
        );

        // current fee = (50_000 SACP + 10_000 cover) - 50_000 output = 10_000
        let fee = adaptor
            .current(&params, &cover_provider, &tx)
            .expect("current fee");
        assert_eq!(fee, 10_000);
    }

    #[test]
    fn current_fee_with_regular_spend() {
        let adaptor = test_adaptor();
        let spend = dummy_regular_spend(50_000);

        let mut params = empty_params();
        params.spends = vec![spend];
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        let cover_provider = BitcoinCoverUtxoProvider::new_with_selected(
            test_wallet().address().clone(),
            covers,
            None,
        );

        // Regular spend produces no output, so:
        // current fee = (50_000 spend + 10_000 cover) - 0 outputs = 60_000
        let fee = adaptor
            .current(&params, &cover_provider, &tx)
            .expect("current fee");
        assert_eq!(fee, 60_000);
    }

    #[test]
    fn current_fee_with_both_sacp_and_spend() {
        let adaptor = test_adaptor();
        let sacp = dummy_sacp_spend(30_000);
        let spend = dummy_regular_spend(20_000);

        let mut params = empty_params();
        params.sacps = vec![sacp];
        params.spends = vec![spend];
        let covers = vec![dummy_cover_utxo(5_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        let cover_provider = BitcoinCoverUtxoProvider::new_with_selected(
            test_wallet().address().clone(),
            covers,
            None,
        );

        // Only SACP produces output (30k). Regular spend has no output.
        // current fee = (30_000 + 20_000 + 5_000) - 30_000 = 25_000
        let fee = adaptor
            .current(&params, &cover_provider, &tx)
            .expect("current fee");
        assert_eq!(fee, 25_000);
    }

    #[test]
    fn target_fee_fresh_tx() {
        let adaptor = test_adaptor();
        let mut params = empty_params();
        params.fee_rate = 10.0;
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        let target = adaptor.target(&params, &tx).expect("target fee");
        // target() adds +1 vbyte safety margin for witness finalization
        let vsize = estimate_vsize(&tx) + 1;
        let expected = (vsize as f64 * 10.0).ceil() as u64;
        assert_eq!(target, expected);
    }

    #[test]
    fn target_fee_rbf_takes_maximum() {
        let wallet = test_wallet();
        let adaptor = BitcoinTxAdaptor::new(wallet.clone(), Network::Regtest).with_rbf_context(
            Some(RbfFeeContext {
                previous_fee_rate: 5.0,
                previous_total_fee: 1000,
                descendant_fee: 200,
            }),
        );

        let mut params = empty_params();
        params.fee_rate = 3.0; // lower market rate
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        let target = adaptor.target(&params, &tx).expect("target fee");
        // target() adds +1 vbyte safety margin
        let vsize = estimate_vsize(&tx) + 1;

        // fee1: (5.0 + 0.001) * vsize
        let fee1 = ((5.001) * vsize as f64).ceil() as u64;
        // fee2: 1000 + 200 + vsize
        let fee2 = (1000.0 + 200.0 + vsize as f64).ceil() as u64;
        // fee3: 3.0 * vsize
        let fee3 = (3.0 * vsize as f64).ceil() as u64;

        let expected = fee1.max(fee2).max(fee3);
        assert_eq!(target, expected);
    }

    #[test]
    fn all_inputs_use_rbf_sequence() {
        let adaptor = test_adaptor();
        let sacp = dummy_sacp_spend(50_000);
        let spend = dummy_regular_spend(30_000);
        let mut params = empty_params();
        params.sacps = vec![sacp];
        params.spends = vec![spend];
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        for input in &tx.input {
            assert_eq!(
                input.sequence,
                Sequence::ENABLE_RBF_NO_LOCKTIME,
                "all inputs must signal RBF"
            );
        }
    }

    #[test]
    fn cover_inputs_have_witness_placeholder() {
        let adaptor = test_adaptor();
        let params = empty_params();
        let covers = vec![dummy_cover_utxo(10_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        assert_eq!(tx.input.len(), 1);
        let witness = &tx.input[0].witness;
        assert_eq!(witness.len(), 1, "cover witness should have 1 element");
        let elem = witness.nth(0).expect("witness element");
        assert_eq!(elem.len(), P2TR_KEYPATH_WITNESS_PLACEHOLDER_LEN);
    }

    // ── Input ordering tests ───────────────────────────────────────────────

    #[test]
    fn input_ordering_sacps_spends_covers() {
        let adaptor = test_adaptor();
        let sacp = dummy_sacp_spend(50_000);
        let spend = dummy_regular_spend(30_000);

        // Capture expected outpoints
        let sacp_outpoint = sacp.outpoint;
        let spend_outpoint = spend.outpoint;
        let cover = dummy_cover_utxo(5_000);
        let cover_outpoint = cover.outpoint;

        let mut params = empty_params();
        params.sacps = vec![sacp];
        params.spends = vec![spend];
        let tx = adaptor.build(&params, &[cover], 0).expect("build");

        assert_eq!(tx.input.len(), 3, "should have 3 inputs");
        assert_eq!(
            tx.input[0].previous_output, sacp_outpoint,
            "first input must be SACP"
        );
        assert_eq!(
            tx.input[1].previous_output, spend_outpoint,
            "second input must be regular spend"
        );
        assert_eq!(
            tx.input[2].previous_output, cover_outpoint,
            "third input must be cover"
        );
    }

    // ── Output ordering tests ──────────────────────────────────────────────

    #[test]
    fn output_ordering_sacp_send_change() {
        // Regular spends do NOT produce outputs — their value joins the pool.
        // Output ordering: [SACP paired outputs..., sends..., change]
        let adaptor = test_adaptor();
        let sacp = dummy_sacp_spend(50_000);
        let _spend = dummy_regular_spend(30_000);
        let sacp_recipient_spk = sacp.recipient.as_ref().unwrap().address.script_pubkey();

        let secp = Secp256k1::new();
        let kp = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = kp.x_only_public_key();
        let send_addr = Address::p2tr(&secp, xonly, None, Network::Regtest);
        let send_spk = send_addr.script_pubkey();

        let mut params = empty_params();
        params.sacps = vec![sacp];
        params.spends = vec![_spend];
        params.sends = vec![SendRequest {
            address: send_addr,
            amount: 10_000,
        }];
        let covers = vec![dummy_cover_utxo(5_000)];
        let tx = adaptor.build(&params, &covers, 1_000).expect("build");

        // Outputs: [SACP(50k), Send(10k), Change(1k)] — no spend output
        assert_eq!(tx.output.len(), 3);
        assert_eq!(tx.output[0].value.to_sat(), 50_000);
        assert_eq!(tx.output[0].script_pubkey, sacp_recipient_spk);
        assert_eq!(tx.output[1].value.to_sat(), 10_000);
        assert_eq!(tx.output[1].script_pubkey, send_spk);
        assert_eq!(tx.output[2].value.to_sat(), 1_000);
    }

    #[test]
    fn sacp_outputs_are_one_to_one_paired() {
        let adaptor = test_adaptor();
        let sacp1 = dummy_sacp_spend(25_000);
        let sacp2 = dummy_sacp_spend(35_000);
        let r1_spk = sacp1.recipient.as_ref().unwrap().address.script_pubkey();
        let r2_spk = sacp2.recipient.as_ref().unwrap().address.script_pubkey();

        let mut params = empty_params();
        params.sacps = vec![sacp1, sacp2];
        let covers = vec![dummy_cover_utxo(5_000)];
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        // 2 SACP inputs + 1 cover
        assert_eq!(tx.input.len(), 3);
        // 2 SACP outputs (no change, no sends)
        assert_eq!(tx.output.len(), 2);
        // Output 0 paired with SACP 0
        assert_eq!(tx.output[0].value.to_sat(), 25_000);
        assert_eq!(tx.output[0].script_pubkey, r1_spk);
        // Output 1 paired with SACP 1
        assert_eq!(tx.output[1].value.to_sat(), 35_000);
        assert_eq!(tx.output[1].script_pubkey, r2_spk);
    }

    #[test]
    fn regular_spend_produces_no_output() {
        // Regular spends' value joins the pool — no dedicated output.
        let adaptor = test_adaptor();
        let spend = dummy_regular_spend(50_000);

        let mut params = empty_params();
        params.spends = vec![spend];
        let covers = vec![dummy_cover_utxo(5_000)];
        // All input value (50k spend + 5k cover = 55k) goes to change (0) → no outputs
        let tx = adaptor.build(&params, &covers, 0).expect("build");

        assert_eq!(
            tx.output.len(),
            0,
            "regular spend should not produce outputs"
        );
    }

    // ── Signing tests ──────────────────────────────────────────────────────

    #[test]
    fn sign_sacp_replaces_placeholder() {
        let adaptor = test_adaptor();
        let sacp = dummy_sacp_spend(50_000);

        let mut params = empty_params();
        params.sacps = vec![sacp];
        let covers = vec![dummy_cover_utxo(5_000)];
        let mut tx = adaptor.build(&params, &covers, 0).expect("build");

        // Before signing, SACP input witness[0] is the 65-byte placeholder
        let pre_sig = tx.input[0].witness.nth(0).expect("witness element");
        assert_eq!(pre_sig.len(), 65, "placeholder must be 65 bytes");

        adaptor.sign(&mut tx, &params, &covers).expect("sign");

        // After signing, SACP input witness[0] should be a 65-byte sig
        // (64 Schnorr + 1 sighash_type byte for SACP)
        let post_sig = tx.input[0].witness.nth(0).expect("witness element");
        assert_eq!(
            post_sig.len(),
            65,
            "SACP signature should be 65 bytes (64 sig + 1 sighash type)"
        );
        assert!(
            !post_sig.iter().all(|&b| b == 0),
            "signature must not be all zeros"
        );
        // Last byte should be SIGHASH_SINGLE|ANYONECANPAY (0x83)
        assert_eq!(
            post_sig[64], 0x83,
            "sighash type byte must be SIGHASH_SINGLE|ANYONECANPAY"
        );
    }

    #[test]
    fn sign_regular_spend_replaces_placeholder() {
        let adaptor = test_adaptor();
        let spend = dummy_regular_spend(50_000);

        let mut params = empty_params();
        params.spends = vec![spend];
        let covers = vec![dummy_cover_utxo(5_000)];
        let mut tx = adaptor.build(&params, &covers, 0).expect("build");

        // Before signing, spend input witness[0] is 65-byte placeholder
        let pre_sig = tx.input[0].witness.nth(0).expect("witness element");
        assert_eq!(pre_sig.len(), 65);

        adaptor.sign(&mut tx, &params, &covers).expect("sign");

        // After signing, spend input witness[0] should be a 65-byte sig
        // (64 Schnorr + 1 sighash_type byte for SIGHASH_ALL)
        let post_sig = tx.input[0].witness.nth(0).expect("witness element");
        assert_eq!(
            post_sig.len(),
            65,
            "regular spend signature should be 65 bytes (64 sig + 1 sighash type)"
        );
        assert!(
            !post_sig.iter().all(|&b| b == 0),
            "signature must not be all zeros"
        );
        // Last byte should be SIGHASH_ALL (0x01)
        assert_eq!(post_sig[64], 0x01, "sighash type byte must be SIGHASH_ALL");
    }

    #[test]
    fn sign_cover_inputs_keypath() {
        let adaptor = test_adaptor();
        let params = empty_params();
        let covers = vec![dummy_cover_utxo(10_000)];
        let mut tx = adaptor.build(&params, &covers, 0).expect("build");

        // Before signing, cover input has 65-byte zero placeholder
        let pre_wit = &tx.input[0].witness;
        assert_eq!(pre_wit.len(), 1);
        let pre_elem = pre_wit.nth(0).expect("witness element");
        assert_eq!(pre_elem.len(), P2TR_KEYPATH_WITNESS_PLACEHOLDER_LEN);

        adaptor.sign(&mut tx, &params, &covers).expect("sign");

        // After signing, cover input should have a real signature
        let post_wit = &tx.input[0].witness;
        assert_eq!(post_wit.len(), 1, "P2TR key-path witness has 1 element");
        let post_elem = post_wit.nth(0).expect("witness element");
        assert_eq!(
            post_elem.len(),
            65,
            "P2TR key-path signature should be 65 bytes"
        );
        assert!(
            !post_elem.iter().all(|&b| b == 0),
            "cover signature must not be all zeros"
        );
    }

    // ── Mixed batch test ───────────────────────────────────────────────────

    #[test]
    fn mixed_batch_sacp_spend_send() {
        let adaptor = test_adaptor();
        let sacp = dummy_sacp_spend(40_000);
        let spend = dummy_regular_spend(25_000);

        let secp = Secp256k1::new();
        let kp = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = kp.x_only_public_key();
        let send_addr = Address::p2tr(&secp, xonly, None, Network::Regtest);

        let sacp_recipient_spk = sacp.recipient.as_ref().unwrap().address.script_pubkey();
        let send_spk = send_addr.script_pubkey();

        let mut params = empty_params();
        params.sacps = vec![sacp];
        params.spends = vec![spend];
        params.sends = vec![SendRequest {
            address: send_addr,
            amount: 8_000,
        }];
        let covers = vec![dummy_cover_utxo(10_000)];
        let mut tx = adaptor.build(&params, &covers, 2_000).expect("build");

        // Inputs: [SACP, spend, cover] = 3
        assert_eq!(tx.input.len(), 3);
        // Outputs: [SACP(40k), send(8k), change(2k)] = 3 (no spend output)
        assert_eq!(tx.output.len(), 3);

        assert_eq!(tx.output[0].value.to_sat(), 40_000);
        assert_eq!(tx.output[0].script_pubkey, sacp_recipient_spk);
        assert_eq!(tx.output[1].value.to_sat(), 8_000);
        assert_eq!(tx.output[1].script_pubkey, send_spk);
        assert_eq!(tx.output[2].value.to_sat(), 2_000);

        // Sign everything and verify it does not error
        adaptor.sign(&mut tx, &params, &covers).expect("sign");

        // Verify SACP input signed with SACP sighash
        let sacp_sig = tx.input[0].witness.nth(0).expect("sacp sig");
        assert_eq!(sacp_sig.len(), 65);
        assert_eq!(sacp_sig[64], 0x83); // SIGHASH_SINGLE|ANYONECANPAY

        // Verify spend input signed with SIGHASH_ALL
        let spend_sig = tx.input[1].witness.nth(0).expect("spend sig");
        assert_eq!(spend_sig.len(), 65);
        assert_eq!(spend_sig[64], 0x01); // SIGHASH_ALL

        // Verify cover input signed (key-path)
        let cover_sig = tx.input[2].witness.nth(0).expect("cover sig");
        assert_eq!(cover_sig.len(), 65);
    }
}
