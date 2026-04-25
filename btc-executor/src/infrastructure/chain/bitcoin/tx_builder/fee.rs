//! Virtual size estimation for Bitcoin transactions.
//!
//! Uses the `bitcoin` crate's weight calculation and converts to vbytes
//! via the standard formula: `vsize = ceil(weight / 4)`.

/// Estimate the virtual size (vsize) of a Bitcoin transaction in vbytes.
///
/// The virtual size is derived from the transaction weight:
///   `vsize = ceil(weight_units / 4)`
///
/// This accounts for the segwit discount: witness data is counted at
/// 1/4 weight compared to non-witness data.
pub fn estimate_vsize(tx: &bitcoin::Transaction) -> u64 {
    tx.weight().to_wu().div_ceil(4)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        absolute::LockTime, hashes::Hash, transaction::Version, Amount, OutPoint, ScriptBuf,
        Sequence, TxIn, TxOut, Txid, Witness,
    };

    fn dummy_txin() -> TxIn {
        TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0u8; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            witness: Witness::new(),
        }
    }

    fn dummy_txout() -> TxOut {
        TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }
    }

    #[test]
    fn vsize_is_positive_for_nonempty_tx() {
        let tx = bitcoin::Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_txin()],
            output: vec![dummy_txout()],
        };
        let vsize = estimate_vsize(&tx);
        assert!(vsize > 0, "vsize must be > 0 for a transaction with I/O");
    }

    #[test]
    fn vsize_increases_with_more_inputs() {
        let tx_one = bitcoin::Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_txin()],
            output: vec![dummy_txout()],
        };
        let tx_two = bitcoin::Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_txin(), dummy_txin()],
            output: vec![dummy_txout()],
        };
        assert!(
            estimate_vsize(&tx_two) > estimate_vsize(&tx_one),
            "more inputs should increase vsize"
        );
    }

    #[test]
    fn vsize_with_witness_data() {
        let mut txin = dummy_txin();
        // Simulate a P2TR key-path witness (65 bytes: 64 sig + 1 sighash type)
        let mut w = Witness::new();
        w.push([0u8; 65]);
        txin.witness = w;

        let tx = bitcoin::Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![txin],
            output: vec![dummy_txout()],
        };
        let vsize = estimate_vsize(&tx);
        // With witness data the transaction should still have a reasonable vsize
        assert!(vsize > 0);
    }
}
