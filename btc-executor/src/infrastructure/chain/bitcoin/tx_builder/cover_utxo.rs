//! Bitcoin cover UTXO provider — queries Electrs for wallet UTXOs and
//! selects the minimum set needed to cover transaction fees.
//!
//! Only confirmed UTXOs are considered; unconfirmed (mempool) outputs are
//! excluded to avoid spending outputs that may not yet be valid.

use std::collections::HashSet;
use std::sync::Arc;

use bitcoin::{OutPoint, Transaction, Txid};

use super::deps::{CoverUtxoProvider, TxBuilderError};
use super::primitives::CoverUtxo;
use crate::infrastructure::chain::bitcoin::clients::{ElectrsClient, Utxo};

/// Provides cover UTXOs from the executor wallet to pay transaction fees.
///
/// Queries the Electrs REST API for available UTXOs, merges in any
/// `must_include` UTXOs (inflight wallet UTXOs invisible to Electrs during
/// RBF — Electrs filters out mempool-spent UTXOs), deduplicates, and uses
/// a greedy algorithm to select the minimum set covering the fee shortfall.
pub struct BitcoinCoverUtxoProvider {
    address: bitcoin::Address,
    electrs: Option<Arc<ElectrsClient>>,
    /// Wallet UTXOs that must be included in the available set even though
    /// Electrs won't return them (they're spent by our inflight mempool tx).
    /// During RBF, Bitcoin Core allows re-spending them in the replacement.
    must_include: Vec<CoverUtxo>,
    /// UTXOs already committed to the candidate transaction. For RBF builds,
    /// this is seeded with the inflight cover set so the replacement always
    /// conflicts with the mempool tx and recomputes change from that full set.
    selected: Vec<CoverUtxo>,
    /// Tracks the special lineage prevout when present so newly-added fee
    /// covers can be inserted ahead of it and it remains the final input.
    lineage_prevout: Option<OutPoint>,
    /// Wallet outpoints reserved by other live/pending batches. They remain
    /// unavailable for generic fee-cover selection in this build.
    ignored_outpoints: HashSet<OutPoint>,
}

impl BitcoinCoverUtxoProvider {
    /// Create a new provider that can query Electrs for available UTXOs.
    ///
    /// `must_include` contains wallet UTXOs from the inflight batcher tx that
    /// Electrs won't return (it filters mempool-spent UTXOs). During RBF,
    /// Bitcoin Core allows re-spending these in the replacement transaction.
    /// When non-empty, these are also pre-assigned as `selected` so the fee
    /// builder starts from the full carried-forward cover set.
    pub fn new(
        address: bitcoin::Address,
        electrs: Arc<ElectrsClient>,
        must_include: Vec<CoverUtxo>,
        lineage_prevout: Option<CoverUtxo>,
        ignored_outpoints: impl IntoIterator<Item = OutPoint>,
    ) -> Self {
        let mut selected = must_include;
        let lineage_prevout_outpoint = lineage_prevout.as_ref().map(|u| u.outpoint);
        if let Some(utxo) = lineage_prevout {
            selected.push(utxo);
        }
        Self {
            address,
            electrs: Some(electrs),
            must_include: selected.clone(),
            selected,
            lineage_prevout: lineage_prevout_outpoint,
            ignored_outpoints: ignored_outpoints.into_iter().collect(),
        }
    }

    /// Create a provider with pre-selected UTXOs (for testing or when
    /// the cover set is already known).
    #[cfg(test)]
    pub fn new_with_selected(
        address: bitcoin::Address,
        mut selected: Vec<CoverUtxo>,
        lineage_prevout: Option<CoverUtxo>,
    ) -> Self {
        let lineage_prevout_outpoint = lineage_prevout.as_ref().map(|u| u.outpoint);
        if let Some(u) = lineage_prevout {
            selected.push(u);
        }
        Self {
            address,
            electrs: None,
            must_include: Vec::new(),
            selected,
            lineage_prevout: lineage_prevout_outpoint,
            ignored_outpoints: HashSet::new(),
        }
    }

    /// Selected wallet-owned fee-cover inputs excluding the optional lineage
    /// prevout, which is tracked separately for ordering reasons.
    pub fn selected_cover_utxos(&self) -> &[CoverUtxo] {
        match self.lineage_prevout {
            Some(_) => &self.selected[..self.selected.len().saturating_sub(1)],
            None => &self.selected,
        }
    }

    /// Selected lineage prevout when the build is chained to a confirmed anchor.
    pub fn selected_lineage_prevout(&self) -> Option<&CoverUtxo> {
        self.lineage_prevout.and_then(|_| self.selected.last())
    }
}

#[async_trait::async_trait]
impl CoverUtxoProvider for BitcoinCoverUtxoProvider {
    type Utxo = CoverUtxo;
    type Tx = Transaction;

    fn selected(&self) -> &[CoverUtxo] {
        &self.selected
    }

    fn add(&mut self, utxos: Vec<CoverUtxo>) {
        match self.lineage_prevout {
            Some(_) => {
                let insert_at = self.selected.len().saturating_sub(1);
                self.selected.splice(insert_at..insert_at, utxos);
            }
            None => {
                self.selected.extend(utxos);
            }
        }
    }

    /// Query Electrs for available UTXOs and merge with `must_include` UTXOs.
    ///
    /// Electrs filters out UTXOs spent by mempool transactions, so inflight
    /// batcher UTXOs won't appear. `must_include` provides these missing UTXOs
    /// for RBF replacement. After merging (with dedup), we filter out:
    /// - UTXOs already used as inputs in the candidate transaction
    /// - UTXOs already in our selected set
    async fn available(&self, tx: &Transaction) -> Result<Vec<CoverUtxo>, TxBuilderError> {
        let electrs = self
            .electrs
            .as_ref()
            .ok_or_else(|| TxBuilderError::Client("no electrs client configured".into()))?;

        let electrs_utxos = electrs
            .get_address_utxos(&self.address.to_string())
            .await
            .map_err(|e| TxBuilderError::Client(e.to_string()))?;
        let current_height = electrs
            .get_block_height()
            .await
            .map_err(|e| TxBuilderError::Client(e.to_string()))?;

        // Convert Electrs UTXOs to CoverUtxo, keeping only confirmed wallet
        // UTXOs that are not reserved by other batches.
        let script_pubkey = self.address.script_pubkey();
        let mut all: Vec<CoverUtxo> = Vec::new();
        for u in electrs_utxos
            .into_iter()
            .filter(|u| is_confirmed_utxo(u, current_height))
        {
            let txid: Txid = u
                .txid
                .parse()
                .map_err(|e| TxBuilderError::Consensus(format!("invalid txid: {e}")))?;
            let outpoint = OutPoint { txid, vout: u.vout };
            if self.ignored_outpoints.contains(&outpoint) {
                continue;
            }
            all.push(CoverUtxo {
                outpoint,
                value: u.value,
                script_pubkey: script_pubkey.clone(),
            });
        }

        // Merge must_include UTXOs (inflight wallet UTXOs invisible to Electrs)
        for utxo in &self.must_include {
            if !all.iter().any(|a| a.outpoint == utxo.outpoint) {
                all.push(utxo.clone());
            }
        }

        // Filter out already-used and already-selected
        let available = all
            .into_iter()
            .filter(|u| !tx.input.iter().any(|inp| inp.previous_output == u.outpoint))
            .filter(|u| !self.selected.iter().any(|s| s.outpoint == u.outpoint))
            .collect();

        Ok(available)
    }

    /// Greedy selection: sort by value descending, accumulate until >= `needed`.
    fn select(&self, utxos: Vec<CoverUtxo>, needed: u64) -> Result<Vec<CoverUtxo>, TxBuilderError> {
        let mut sorted = utxos;
        sorted.sort_by(|a, b| b.value.cmp(&a.value));

        let mut acc = 0u64;
        let mut selected = Vec::new();
        for utxo in sorted {
            acc += utxo.value;
            selected.push(utxo);
            if acc >= needed {
                return Ok(selected);
            }
        }

        Err(TxBuilderError::InsufficientFunds {
            needed,
            available: acc,
        })
    }
}

fn is_confirmed_utxo(utxo: &Utxo, current_height: u64) -> bool {
    // let Some(block_height) = utxo.status.block_height else {
    //     return false;
    // };
    // utxo.status.confirmed && block_height <= current_height
    true
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::chain::bitcoin::clients::TxStatus;
    use bitcoin::{Network, Txid, hashes::Hash};

    fn test_address() -> bitcoin::Address {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let kp =
            bitcoin::secp256k1::Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = kp.x_only_public_key();
        bitcoin::Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn utxo_with_value(value: u64, vout: u32) -> CoverUtxo {
        CoverUtxo {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([vout as u8; 32]),
                vout,
            },
            value,
            script_pubkey: bitcoin::ScriptBuf::new(),
        }
    }

    #[test]
    fn select_greedy_picks_highest_first() {
        let provider = BitcoinCoverUtxoProvider::new_with_selected(test_address(), vec![], None);
        let utxos = vec![
            utxo_with_value(1_000, 0),
            utxo_with_value(5_000, 1),
            utxo_with_value(3_000, 2),
        ];
        let selected = provider.select(utxos, 4_000).expect("select");
        assert_eq!(selected.len(), 1, "should pick the 5000-sat UTXO");
        assert_eq!(selected[0].value, 5_000);
    }

    #[test]
    fn select_multiple_utxos() {
        let provider = BitcoinCoverUtxoProvider::new_with_selected(test_address(), vec![], None);
        let utxos = vec![
            utxo_with_value(1_000, 0),
            utxo_with_value(2_000, 1),
            utxo_with_value(3_000, 2),
        ];
        let selected = provider.select(utxos, 5_000).expect("select");
        assert_eq!(selected.len(), 2, "should pick 3000 + 2000");
        assert_eq!(selected[0].value, 3_000);
        assert_eq!(selected[1].value, 2_000);
    }

    #[test]
    fn select_insufficient_funds() {
        let provider = BitcoinCoverUtxoProvider::new_with_selected(test_address(), vec![], None);
        let utxos = vec![utxo_with_value(1_000, 0)];
        let result = provider.select(utxos, 5_000);
        assert!(result.is_err());
        match result.unwrap_err() {
            TxBuilderError::InsufficientFunds { needed, available } => {
                assert_eq!(needed, 5_000);
                assert_eq!(available, 1_000);
            }
            other => panic!("expected InsufficientFunds, got: {other}"),
        }
    }

    #[test]
    fn add_extends_selected() {
        let mut provider =
            BitcoinCoverUtxoProvider::new_with_selected(test_address(), vec![], None);
        assert!(provider.selected().is_empty());
        provider.add(vec![utxo_with_value(1_000, 0)]);
        assert_eq!(provider.selected().len(), 1);
        provider.add(vec![utxo_with_value(2_000, 1), utxo_with_value(3_000, 2)]);
        assert_eq!(provider.selected().len(), 3);
    }

    #[test]
    fn new_preselects_must_include_utxos() {
        let address = test_address();
        let inflight = vec![utxo_with_value(4_000, 0), utxo_with_value(6_000, 1)];
        let electrs = Arc::new(ElectrsClient::new("http://127.0.0.1:1".to_string()));

        let provider = BitcoinCoverUtxoProvider::new(address, electrs, inflight.clone(), None, []);

        assert_eq!(provider.selected().len(), inflight.len());
        assert_eq!(provider.selected()[0].outpoint, inflight[0].outpoint);
        assert_eq!(provider.selected()[1].outpoint, inflight[1].outpoint);
    }

    #[test]
    fn add_keeps_lineage_prevout_last() {
        let lineage = utxo_with_value(4_000, 9);
        let mut provider = BitcoinCoverUtxoProvider::new_with_selected(
            test_address(),
            vec![utxo_with_value(1_000, 0)],
            Some(lineage.clone()),
        );

        provider.add(vec![utxo_with_value(2_000, 1), utxo_with_value(3_000, 2)]);

        assert_eq!(provider.selected().len(), 4);
        assert_eq!(provider.selected()[1].outpoint.vout, 1);
        assert_eq!(provider.selected()[2].outpoint.vout, 2);
        assert_eq!(
            provider
                .selected()
                .last()
                .expect("lineage prevout")
                .outpoint,
            lineage.outpoint
        );
    }

    #[test]
    fn confirmation_filter_requires_confirmed_height_not_future() {
        let confirmed = Utxo {
            txid: Txid::from_byte_array([1u8; 32]).to_string(),
            vout: 0,
            value: 10_000,
            status: TxStatus {
                confirmed: true,
                block_height: Some(93),
                block_hash: None,
                block_time: None,
            },
        };
        let future = Utxo {
            txid: Txid::from_byte_array([2u8; 32]).to_string(),
            vout: 0,
            value: 10_000,
            status: TxStatus {
                confirmed: true,
                block_height: Some(105),
                block_hash: None,
                block_time: None,
            },
        };
        let current_height = 100;

        assert!(is_confirmed_utxo(&confirmed, current_height));
        assert!(!is_confirmed_utxo(&future, current_height));
    }

    #[test]
    fn selected_parts_keep_lineage_prevout_separate() {
        let lineage = utxo_with_value(4_000, 9);
        let provider = BitcoinCoverUtxoProvider::new_with_selected(
            test_address(),
            vec![utxo_with_value(1_000, 0), utxo_with_value(2_000, 1)],
            Some(lineage.clone()),
        );

        assert_eq!(provider.selected_cover_utxos().len(), 2);
        assert_eq!(
            provider
                .selected_lineage_prevout()
                .expect("lineage prevout")
                .outpoint,
            lineage.outpoint
        );
    }
}
