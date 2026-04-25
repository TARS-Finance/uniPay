//! Event payloads emitted by the wallet subsystem.
//!
//! These are informational domain-style events for callers that care about
//! batch lifecycle boundaries rather than low-level store rows.

use bitcoin::{Transaction, Txid};

use super::{ChainAnchor, LineageId, WalletRequest};

/// Observable wallet lifecycle events.
#[derive(Clone, Debug)]
pub enum WalletEvent {
    /// A batch was built, persisted, and treated as submitted.
    ///
    /// `replaces` is set for RBF submissions. `chain_anchor` is set when the
    /// batch was chained from confirmed change and descendants may care about
    /// that lineage relationship.
    BatchBroadcast {
        txid: Txid,
        raw_tx: Box<Transaction>,
        requests: Vec<WalletRequest>,
        lineage_id: LineageId,
        replaces: Option<Txid>,
        chain_anchor: Option<ChainAnchor>,
    },
    /// A lineage winner confirmed and the orphan count is now known.
    BatchConfirmed {
        txid: Txid,
        requests: Vec<WalletRequest>,
        lineage_id: LineageId,
        orphaned_count: usize,
    },
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::{OutPoint, ScriptBuf, Transaction, Txid};

    use crate::infrastructure::chain::bitcoin::wallet::{
        ChainAnchor, LineageId, WalletEvent, WalletRequest,
    };

    #[test]
    fn batch_broadcast_event_carries_spec_fields() {
        let request = WalletRequest::send(sample_key(), sample_address(), 12_000).unwrap();
        let lineage_id = LineageId::new();
        let txid = Txid::from_byte_array([9u8; 32]);
        let raw_tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![],
            output: vec![],
        };
        let anchor = ChainAnchor {
            confirmed_txid: txid,
            change_outpoint: OutPoint { txid, vout: 1 },
            change_value: 24_000,
            change_script_pubkey: ScriptBuf::new(),
            confirmed_height: 42,
        };

        let event = WalletEvent::BatchBroadcast {
            txid,
            raw_tx: Box::new(raw_tx),
            requests: vec![request],
            lineage_id,
            replaces: None,
            chain_anchor: Some(anchor),
        };

        match event {
            WalletEvent::BatchBroadcast {
                txid: got_txid,
                requests,
                lineage_id: got_lineage,
                chain_anchor,
                ..
            } => {
                assert_eq!(got_txid, txid);
                assert_eq!(requests.len(), 1);
                assert_eq!(got_lineage, lineage_id);
                assert!(chain_anchor.is_some());
            },
            WalletEvent::BatchConfirmed { .. } => panic!("expected batch broadcast"),
        }
    }

    fn sample_key() -> &'static str {
        "event-dedupe"
    }

    fn sample_address() -> bitcoin::Address {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let secret_key = bitcoin::secp256k1::SecretKey::from_slice(&[8u8; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        bitcoin::Address::p2tr(&secp, xonly, None, bitcoin::Network::Regtest)
    }
}
