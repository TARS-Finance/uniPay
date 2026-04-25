//! Shared value types persisted and exchanged across the wallet runtime.

use bitcoin::{OutPoint, ScriptBuf, Txid};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Stable identifier for an RBF lineage.
///
/// A lineage groups one logical batch together across all of its replacement
/// transactions, which lets the runtime recover or reconcile the latest winner
/// after restarts and reorg-like mempool churn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LineageId(pub Uuid);

impl LineageId {
    /// Allocate a new lineage identifier for a freshly created batch.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Wrap an existing UUID loaded from persistence or tests.
    pub fn from_uuid(id: Uuid) -> Self {
        Self(id)
    }
}

impl Default for LineageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for LineageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A confirmed change output that subsequent requests may chain onto.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAnchor {
    /// Confirmed transaction that produced the anchor.
    pub confirmed_txid: Txid,
    /// Specific wallet-owned change outpoint that remains spendable.
    pub change_outpoint: OutPoint,
    /// Value of the change output in sats.
    pub change_value: u64,
    /// Script pubkey of the change output. Stored so the builder can reuse it
    /// without another chain lookup.
    pub change_script_pubkey: ScriptBuf,
    /// Confirmation height used to age out the anchor after enough blocks.
    pub confirmed_height: u64,
}

/// Wallet-owned input that can be used to fund fees/change in a batch build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoverUtxo {
    /// Spendable outpoint selected by the builder.
    pub outpoint: OutPoint,
    /// Value in sats.
    pub value: u64,
    /// Script pubkey needed for signing and input classification.
    pub script_pubkey: ScriptBuf,
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::{OutPoint, ScriptBuf, Txid};
    use uuid::Uuid;

    use super::{ChainAnchor, CoverUtxo, LineageId};

    #[test]
    fn lineage_id_round_trips_uuid() {
        let uuid = Uuid::new_v4();
        let id = LineageId::from_uuid(uuid);
        assert_eq!(id.to_string(), uuid.to_string());
    }

    #[test]
    fn chain_anchor_round_trips_typed_fields() {
        let txid = Txid::from_byte_array([1u8; 32]);
        let anchor = ChainAnchor {
            confirmed_txid: txid,
            change_outpoint: OutPoint { txid, vout: 2 },
            change_value: 42_000,
            change_script_pubkey: ScriptBuf::new(),
            confirmed_height: 321,
        };

        assert_eq!(anchor.confirmed_txid, txid);
        assert_eq!(anchor.change_outpoint.vout, 2);
        assert_eq!(anchor.change_value, 42_000);
        assert_eq!(anchor.confirmed_height, 321);
    }

    #[test]
    fn cover_utxo_round_trips_typed_fields() {
        let txid = Txid::from_byte_array([2u8; 32]);
        let utxo = CoverUtxo {
            outpoint: OutPoint { txid, vout: 3 },
            value: 19_000,
            script_pubkey: ScriptBuf::new(),
        };

        assert_eq!(utxo.outpoint.txid, txid);
        assert_eq!(utxo.outpoint.vout, 3);
        assert_eq!(utxo.value, 19_000);
    }
}
