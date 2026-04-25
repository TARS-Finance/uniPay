//! Bitcoin transaction builder orchestrator.
//!
//! `BitcoinTxBuilder` is the public entry point.  It validates inputs,
//! creates the adaptor and cover UTXO provider, then runs the iterative
//! fee-builder to produce a transaction with correct fees.
//!
//! Build modes:
//!
//! ```text
//! Fresh
//!   - no prior mempool lineage
//!   - cover selection starts from electrs-visible wallet UTXOs
//!
//! Rbf
//!   - carries forward wallet fee inputs from the current live head
//!   - uses bitcoind-provided fee context to satisfy replacement policy
//!
//! Chained
//!   - explicitly includes a confirmed change prevout as `lineage_prevout`
//!   - keeps descendants tied to the anchor that made them eligible
//! ```

use std::collections::HashSet;
use std::sync::Arc;

use bitcoin::Transaction;

use super::cover_utxo::BitcoinCoverUtxoProvider;
use super::deps::TxBuilderError;
use super::fee_builder;
use super::primitives::{BitcoinTxAdaptorParams, CoverUtxo, RbfFeeContext};
use super::tx_adaptor::BitcoinTxAdaptor;
use super::validation;
use crate::infrastructure::chain::bitcoin::clients::ElectrsClient;
use crate::infrastructure::chain::bitcoin::wallet::{
    SendRequest, SpendRequest, WalletConfig, WalletRequest,
};
use crate::infrastructure::keys::BitcoinWallet;

/// Result of a successful transaction build.
///
/// Contains the signed transaction and the wallet cover UTXOs that were
/// selected to pay fees. The batcher stores `cover_utxos` so that during
/// RBF replacement the next build can start from the full carried-forward
/// cover set even though Electrs won't return those mempool-spent UTXOs.
pub struct BuildTxReceipt {
    /// The fully-signed Bitcoin transaction.
    pub tx: Transaction,
    /// Actual fee paid by the built transaction, in sats.
    pub fee_paid_sats: u64,
    /// Wallet cover UTXOs selected by the fee builder to pay fees.
    /// Store these for RBF so the next replacement starts with the same
    /// wallet-owned conflict inputs and recomputes change from that base set.
    pub cover_utxos: Vec<CoverUtxo>,
    /// The designated lineage prevout, kept separate so runtime state can
    /// preserve the trailing lineage conflict input explicitly.
    pub lineage_prevout: Option<CoverUtxo>,
}

#[derive(Clone, Debug)]
pub struct WalletBuildOptions {
    /// Optional lineage prevout that must remain as the final wallet-owned
    /// funding input so descendants can refer back to it consistently.
    pub lineage_prevout: Option<CoverUtxo>,
    /// Minimum change value the builder should preserve when possible.
    pub min_change_value: u64,
    /// Wallet outpoints reserved by other live work and therefore unavailable
    /// for generic fee coverage in this build.
    pub ignored_cover_outpoints: HashSet<bitcoin::OutPoint>,
}

impl Default for WalletBuildOptions {
    fn default() -> Self {
        Self {
            lineage_prevout: None,
            min_change_value: WalletConfig::default().min_change_value,
            ignored_cover_outpoints: HashSet::new(),
        }
    }
}

/// High-level Bitcoin transaction builder.
///
/// Combines validation, UTXO selection, and iterative fee adjustment to
/// produce fully-structured Bitcoin transactions.
pub struct BitcoinTxBuilder {
    wallet: Arc<BitcoinWallet>,
    electrs: Arc<ElectrsClient>,
    network: bitcoin::Network,
    /// Wallet cover UTXOs from the current inflight batcher tx. Electrs won't
    /// return these (it filters mempool-spent UTXOs) but Bitcoin Core allows
    /// re-spending them in an RBF replacement. RBF builds must start with this
    /// full set already selected so they definitely conflict with the mempool tx
    /// and derive fresh change from the full carried-forward wallet input set.
    inflight_cover_utxos: Vec<CoverUtxo>,
}

impl BitcoinTxBuilder {
    /// Create a new builder.
    pub fn new(
        wallet: Arc<BitcoinWallet>,
        electrs: Arc<ElectrsClient>,
        network: bitcoin::Network,
    ) -> Self {
        Self {
            wallet,
            electrs,
            network,
            inflight_cover_utxos: Vec::new(),
        }
    }

    /// Set the inflight cover UTXOs that must be included in UTXO selection.
    /// Call this before `build_tx_with_rbf` to provide wallet UTXOs from the
    /// inflight tx that Electrs won't return.
    pub fn set_inflight_cover_utxos(&mut self, utxos: Vec<CoverUtxo>) {
        self.inflight_cover_utxos = utxos;
    }

    /// Clear inflight cover UTXOs (after tx confirmed or fresh build).
    pub fn clear_inflight_cover_utxos(&mut self) {
        self.inflight_cover_utxos.clear();
    }

    /// Build a fresh transaction (no RBF context).
    pub async fn build_tx(
        &self,
        sacps: Vec<SpendRequest>,
        spends: Vec<SpendRequest>,
        sends: Vec<SendRequest>,
        fee_rate: f64,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        self.build_tx_inner(
            BitcoinTxAdaptorParams {
                sacps,
                spends,
                sends,
                fee_rate,
            },
            None,
        )
        .await
    }

    /// Build an RBF replacement transaction.
    pub async fn build_tx_with_rbf(
        &self,
        sacps: Vec<SpendRequest>,
        spends: Vec<SpendRequest>,
        sends: Vec<SendRequest>,
        fee_rate: f64,
        rbf_context: RbfFeeContext,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        self.build_tx_inner(
            BitcoinTxAdaptorParams {
                sacps,
                spends,
                sends,
                fee_rate,
            },
            Some(rbf_context),
        )
        .await
    }

    /// Build a fresh transaction directly from generic wallet requests.
    pub async fn build_wallet_tx(
        &self,
        requests: &[WalletRequest],
        fee_rate: f64,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        let params = BitcoinTxAdaptorParams::from_requests(requests, fee_rate)?;
        self.build_tx_inner(params, None).await
    }

    /// Build a fresh transaction from wallet requests with explicit builder
    /// options such as reserved outpoints or a required lineage prevout.
    pub async fn build_wallet_tx_with_options(
        &self,
        requests: &[WalletRequest],
        fee_rate: f64,
        options: WalletBuildOptions,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        let params = BitcoinTxAdaptorParams::from_requests(requests, fee_rate)?;
        self.build_tx_inner_with_options(params, None, options)
            .await
    }

    /// Build an RBF replacement transaction directly from generic wallet requests.
    pub async fn build_wallet_tx_with_rbf(
        &self,
        requests: &[WalletRequest],
        fee_rate: f64,
        rbf_context: RbfFeeContext,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        let params = BitcoinTxAdaptorParams::from_requests(requests, fee_rate)?;
        self.build_tx_inner(params, Some(rbf_context)).await
    }

    /// Build an RBF replacement from wallet requests with explicit builder
    /// options, preserving carried-forward fee inputs and lineage prevouts.
    pub async fn build_wallet_tx_with_rbf_and_options(
        &self,
        requests: &[WalletRequest],
        fee_rate: f64,
        rbf_context: RbfFeeContext,
        options: WalletBuildOptions,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        let params = BitcoinTxAdaptorParams::from_requests(requests, fee_rate)?;
        self.build_tx_inner_with_options(params, Some(rbf_context), options)
            .await
    }

    async fn build_tx_inner(
        &self,
        params: BitcoinTxAdaptorParams,
        rbf_context: Option<RbfFeeContext>,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        self.build_tx_inner_with_options(params, rbf_context, WalletBuildOptions::default())
            .await
    }

    async fn build_tx_inner_with_options(
        &self,
        params: BitcoinTxAdaptorParams,
        rbf_context: Option<RbfFeeContext>,
        options: WalletBuildOptions,
    ) -> Result<BuildTxReceipt, TxBuilderError> {
        // 1. Validate the logical request set before touching UTXO selection.
        validation::validate(&params)?;
        validation::validate_unique_mandatory_inputs(
            &params,
            &self.inflight_cover_utxos,
            options.lineage_prevout.as_ref(),
        )?;

        // 2. Create the Bitcoin-specific adaptor. The optional RBF context
        // changes fee targeting but not the transaction request semantics.
        let adaptor = BitcoinTxAdaptor::new(Arc::clone(&self.wallet), self.network)
            .with_rbf_context(rbf_context);

        // 3. Prepare the cover source. Fresh builds start from ordinary wallet
        // UTXOs, while RBF builds preload carried-forward fee inputs that no
        // longer appear as spendable in Electrs because the current head uses them.
        let mut cover = BitcoinCoverUtxoProvider::new(
            self.wallet.address().clone(),
            Arc::clone(&self.electrs),
            self.inflight_cover_utxos.clone(),
            options.lineage_prevout,
            options.ignored_cover_outpoints,
        );

        // 4. Run the iterative fee loop. This may add/remove wallet cover
        // inputs and resize change until the final transaction satisfies the
        // adaptor's fee target and min-change constraints.
        let result = fee_builder::build_with_min_change(
            &adaptor,
            &params,
            &mut cover,
            options.min_change_value,
        )
        .await?;

        Ok(BuildTxReceipt {
            tx: result.tx,
            fee_paid_sats: result.fee_paid_sats,
            // Persist the actual selected cover set because future RBF builds
            // must start from these same conflict inputs, not from a fresh UTXO scan.
            cover_utxos: cover.selected_cover_utxos().to_vec(),
            // Chained builds keep the lineage prevout separate from generic
            // fee inputs so runtime state can preserve that semantic distinction.
            lineage_prevout: cover.selected_lineage_prevout().cloned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::chain::bitcoin::wallet::WalletRequest;
    use bitcoin::hashes::Hash;
    use bitcoin::{Amount, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Txid, Witness};
    use std::str::FromStr;

    const TEST_BTC_PRIVKEY_HEX: &str =
        "e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35";
    const COUNTERPARTY_BTC_PRIVKEY_HEX: &str =
        "4d5db4107d237df5d5b4a4d8e4fdbb0f9dc7d1f35d4fbc8f38d4e015135c8db0";

    fn test_wallet(hex_key: &str) -> Arc<BitcoinWallet> {
        Arc::new(BitcoinWallet::from_private_key(hex_key, Network::Regtest).expect("test wallet"))
    }

    fn cover_utxo(txid_hex: &str, vout: u32, value: u64, address: &bitcoin::Address) -> CoverUtxo {
        CoverUtxo {
            outpoint: OutPoint {
                txid: Txid::from_str(txid_hex).expect("valid txid"),
                vout,
            },
            value,
            script_pubkey: address.script_pubkey(),
        }
    }

    fn witness_template(tag: u8) -> Witness {
        let mut witness = Witness::new();
        witness.push([0u8; 65]);
        witness.push([tag; 32]);
        witness
    }

    #[tokio::test]
    async fn rbf_build_uses_sufficient_inflight_cover_without_querying_electrs() {
        let wallet = test_wallet(TEST_BTC_PRIVKEY_HEX);
        let recipient = test_wallet(COUNTERPARTY_BTC_PRIVKEY_HEX).address().clone();
        let inflight_cover = cover_utxo(
            "0101010101010101010101010101010101010101010101010101010101010101",
            0,
            20_000,
            wallet.address(),
        );

        // This address is intentionally unreachable. Fresh builds need Electrs,
        // but an RBF build with sufficient carried-forward cover must not need it.
        let electrs = Arc::new(ElectrsClient::new("http://127.0.0.1:1".to_string()));
        let mut builder = BitcoinTxBuilder::new(wallet.clone(), electrs, Network::Regtest);
        builder.set_inflight_cover_utxos(vec![inflight_cover.clone()]);

        let receipt = builder
            .build_tx_with_rbf(
                vec![],
                vec![],
                vec![SendRequest {
                    address: recipient,
                    amount: 5_000,
                }],
                1.0,
                RbfFeeContext {
                    previous_fee_rate: 1.0,
                    previous_total_fee: 1,
                    descendant_fee: 0,
                },
            )
            .await
            .expect("rbf build");

        assert_eq!(
            receipt.cover_utxos.len(),
            1,
            "replacement should carry forward the single inflight cover input",
        );
        assert_eq!(
            receipt.cover_utxos[0].outpoint, inflight_cover.outpoint,
            "replacement must keep the inflight cover input instead of switching to a fresh one",
        );
        assert!(
            receipt
                .tx
                .input
                .iter()
                .any(|input| input.previous_output == inflight_cover.outpoint),
            "replacement tx must directly conflict with the inflight tx via the carried cover input",
        );

        let change_value = receipt
            .tx
            .output
            .iter()
            .find(|output| output.script_pubkey == wallet.address().script_pubkey())
            .map(|output| output.value.to_sat())
            .expect("change output");
        assert!(
            change_value < 15_000,
            "change must be recomputed from the 20k carried input, not a fresh 30k input",
        );

        let total_outputs: u64 = receipt
            .tx
            .output
            .iter()
            .map(|output| output.value.to_sat())
            .sum();
        let fee_paid = inflight_cover.value - total_outputs;
        assert!(
            fee_paid >= Amount::from_sat(1).to_sat(),
            "replacement must still pay a positive fee",
        );
    }

    #[tokio::test]
    async fn wallet_request_rbf_build_reuses_existing_builder_stack() {
        let wallet = test_wallet(TEST_BTC_PRIVKEY_HEX);
        let recipient = test_wallet(COUNTERPARTY_BTC_PRIVKEY_HEX).address().clone();
        let inflight_cover = cover_utxo(
            "0202020202020202020202020202020202020202020202020202020202020202",
            0,
            20_000,
            wallet.address(),
        );

        let electrs = Arc::new(ElectrsClient::new("http://127.0.0.1:1".to_string()));
        let mut builder = BitcoinTxBuilder::new(wallet.clone(), electrs, Network::Regtest);
        builder.set_inflight_cover_utxos(vec![inflight_cover.clone()]);

        let requests = vec![WalletRequest::send("send-1", recipient.clone(), 5_000).unwrap()];

        let receipt = builder
            .build_wallet_tx_with_rbf(
                &requests,
                1.0,
                RbfFeeContext {
                    previous_fee_rate: 1.0,
                    previous_total_fee: 1,
                    descendant_fee: 0,
                },
            )
            .await
            .expect("wallet request rbf build");

        assert_eq!(receipt.cover_utxos.len(), 1);
        assert_eq!(receipt.cover_utxos[0].outpoint, inflight_cover.outpoint);
        assert!(receipt.lineage_prevout.is_none());
        assert_eq!(
            receipt.tx.output[0].script_pubkey,
            recipient.script_pubkey()
        );
        assert_eq!(receipt.tx.output[0].value.to_sat(), 5_000);
    }

    #[tokio::test]
    async fn wallet_build_options_keep_lineage_prevout_as_final_input() {
        let wallet = test_wallet(TEST_BTC_PRIVKEY_HEX);
        let recipient = test_wallet(COUNTERPARTY_BTC_PRIVKEY_HEX).address().clone();
        let fee_cover = cover_utxo(
            "0303030303030303030303030303030303030303030303030303030303030303",
            0,
            20_000,
            wallet.address(),
        );
        let lineage_prevout = cover_utxo(
            "0404040404040404040404040404040404040404040404040404040404040404",
            1,
            8_000,
            wallet.address(),
        );

        let electrs = Arc::new(ElectrsClient::new("http://127.0.0.1:1".to_string()));
        let mut builder = BitcoinTxBuilder::new(wallet.clone(), electrs, Network::Regtest);
        builder.set_inflight_cover_utxos(vec![fee_cover.clone()]);

        let requests = vec![WalletRequest::send("send-1", recipient, 5_000).unwrap()];

        let receipt = builder
            .build_wallet_tx_with_rbf_and_options(
                &requests,
                1.0,
                RbfFeeContext {
                    previous_fee_rate: 1.0,
                    previous_total_fee: 1,
                    descendant_fee: 0,
                },
                WalletBuildOptions {
                    lineage_prevout: Some(lineage_prevout.clone()),
                    ..WalletBuildOptions::default()
                },
            )
            .await
            .expect("wallet request lineage build");

        assert_eq!(
            receipt
                .tx
                .input
                .last()
                .expect("lineage input")
                .previous_output,
            lineage_prevout.outpoint
        );
        assert_eq!(receipt.lineage_prevout, Some(lineage_prevout));
    }

    #[tokio::test]
    async fn chained_rbf_keeps_same_anchor_input_as_final_input_across_replacements() {
        let wallet = test_wallet(TEST_BTC_PRIVKEY_HEX);
        let first_recipient = test_wallet(COUNTERPARTY_BTC_PRIVKEY_HEX).address().clone();
        let second_recipient =
            test_wallet("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
                .address()
                .clone();
        let carried_cover = cover_utxo(
            "1111111111111111111111111111111111111111111111111111111111111111",
            0,
            20_000,
            wallet.address(),
        );
        let anchor_prevout = cover_utxo(
            "1212121212121212121212121212121212121212121212121212121212121212",
            1,
            9_000,
            wallet.address(),
        );

        let electrs = Arc::new(ElectrsClient::new("http://127.0.0.1:1".to_string()));
        let mut builder = BitcoinTxBuilder::new(wallet.clone(), electrs, Network::Regtest);
        builder.set_inflight_cover_utxos(vec![carried_cover.clone()]);

        let first_receipt = builder
            .build_wallet_tx_with_rbf_and_options(
                &[WalletRequest::send("send-1", first_recipient, 5_000).unwrap()],
                1.0,
                RbfFeeContext {
                    previous_fee_rate: 1.0,
                    previous_total_fee: 1,
                    descendant_fee: 0,
                },
                WalletBuildOptions {
                    lineage_prevout: Some(anchor_prevout.clone()),
                    ..WalletBuildOptions::default()
                },
            )
            .await
            .expect("first chained rbf build");

        builder.set_inflight_cover_utxos(first_receipt.cover_utxos.clone());

        let second_receipt = builder
            .build_wallet_tx_with_rbf_and_options(
                &[WalletRequest::send("send-2", second_recipient, 6_000).unwrap()],
                1.2,
                RbfFeeContext {
                    previous_fee_rate: 1.0,
                    previous_total_fee: 1,
                    descendant_fee: 0,
                },
                WalletBuildOptions {
                    lineage_prevout: Some(anchor_prevout.clone()),
                    ..WalletBuildOptions::default()
                },
            )
            .await
            .expect("second chained rbf build");

        assert_eq!(
            first_receipt
                .tx
                .input
                .last()
                .expect("first anchor input")
                .previous_output,
            anchor_prevout.outpoint
        );
        assert_eq!(
            second_receipt
                .tx
                .input
                .last()
                .expect("second anchor input")
                .previous_output,
            anchor_prevout.outpoint
        );
        assert_eq!(first_receipt.lineage_prevout, Some(anchor_prevout.clone()));
        assert_eq!(second_receipt.lineage_prevout, Some(anchor_prevout));
    }

    #[tokio::test]
    async fn wallet_build_orders_sacp_then_spend_then_cover_and_change_last() {
        let wallet = test_wallet(TEST_BTC_PRIVKEY_HEX);
        let send_recipient = test_wallet(COUNTERPARTY_BTC_PRIVKEY_HEX).address().clone();
        let sacp_recipient =
            test_wallet("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
                .address()
                .clone();
        let fee_cover = cover_utxo(
            "0505050505050505050505050505050505050505050505050505050505050505",
            0,
            30_000,
            wallet.address(),
        );
        let lineage_prevout = cover_utxo(
            "0606060606060606060606060606060606060606060606060606060606060606",
            1,
            12_000,
            wallet.address(),
        );
        let regular_spend_outpoint = OutPoint {
            txid: Txid::from_str(
                "0707070707070707070707070707070707070707070707070707070707070707",
            )
            .expect("regular spend txid"),
            vout: 2,
        };
        let sacp_spend_outpoint = OutPoint {
            txid: Txid::from_str(
                "0808080808080808080808080808080808080808080808080808080808080808",
            )
            .expect("sacp txid"),
            vout: 3,
        };

        let electrs = Arc::new(ElectrsClient::new("http://127.0.0.1:1".to_string()));
        let mut builder = BitcoinTxBuilder::new(wallet.clone(), electrs, Network::Regtest);
        builder.set_inflight_cover_utxos(vec![fee_cover.clone()]);

        let requests = vec![
            WalletRequest::send("send-1", send_recipient.clone(), 5_000).expect("send"),
            WalletRequest::spend(
                "regular-1",
                regular_spend_outpoint,
                15_000,
                wallet.address().script_pubkey(),
                witness_template(0x11),
                ScriptBuf::from_bytes(vec![0x51]),
                bitcoin::taproot::TapLeafHash::all_zeros(),
                Sequence::ENABLE_RBF_NO_LOCKTIME,
                TapSighashType::All,
                None,
            )
            .expect("regular spend"),
            WalletRequest::spend(
                "sacp-1",
                sacp_spend_outpoint,
                8_000,
                wallet.address().script_pubkey(),
                witness_template(0x22),
                ScriptBuf::from_bytes(vec![0x52]),
                bitcoin::taproot::TapLeafHash::all_zeros(),
                Sequence::ENABLE_RBF_NO_LOCKTIME,
                TapSighashType::SinglePlusAnyoneCanPay,
                Some(crate::infrastructure::chain::bitcoin::wallet::SendRequest {
                    address: sacp_recipient.clone(),
                    amount: 8_000,
                }),
            )
            .expect("sacp spend"),
        ];

        let receipt = builder
            .build_wallet_tx_with_rbf_and_options(
                &requests,
                1.0,
                RbfFeeContext {
                    previous_fee_rate: 1.0,
                    previous_total_fee: 1,
                    descendant_fee: 0,
                },
                WalletBuildOptions {
                    lineage_prevout: Some(lineage_prevout.clone()),
                    ..WalletBuildOptions::default()
                },
            )
            .await
            .expect("wallet request ordered build");

        assert_eq!(receipt.tx.input.len(), 4);
        assert_eq!(receipt.tx.input[0].previous_output, sacp_spend_outpoint);
        assert_eq!(receipt.tx.input[1].previous_output, regular_spend_outpoint);
        assert_eq!(receipt.tx.input[2].previous_output, fee_cover.outpoint);
        assert_eq!(
            receipt.tx.input[3].previous_output,
            lineage_prevout.outpoint
        );
        assert_eq!(receipt.cover_utxos, vec![fee_cover]);
        assert_eq!(receipt.lineage_prevout, Some(lineage_prevout));

        assert_eq!(
            receipt.tx.output[0].script_pubkey,
            sacp_recipient.script_pubkey()
        );
        assert_eq!(receipt.tx.output[0].value.to_sat(), 8_000);
        assert_eq!(
            receipt.tx.output[1].script_pubkey,
            send_recipient.script_pubkey()
        );
        assert_eq!(receipt.tx.output[1].value.to_sat(), 5_000);
        assert_eq!(
            receipt
                .tx
                .output
                .last()
                .expect("change output")
                .script_pubkey,
            wallet.address().script_pubkey()
        );
    }
}
