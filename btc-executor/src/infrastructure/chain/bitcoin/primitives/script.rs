//! Leaf script builders for Bitcoin HTLC Taproot spending paths.
//!
//! Vendored from garden-rs `crates/bitcoin/src/htlc/script.rs`.

use bitcoin::{opcodes, script::Builder, secp256k1::XOnlyPublicKey, ScriptBuf};

/// Creates a Bitcoin script that allows spending with a secret preimage and
/// redeemer's signature.
///
/// Script: `OP_SHA256 <secret_hash> OP_EQUALVERIFY <redeemer_pubkey> OP_CHECKSIG`
///
/// # Arguments
/// * `secret_hash` - SHA256 hash of the secret (32 bytes)
/// * `redeemer_pubkey` - Public key of the redeemer
pub fn redeem_leaf(secret_hash: &[u8; 32], redeemer_pubkey: &XOnlyPublicKey) -> ScriptBuf {
    Builder::new()
        .push_opcode(opcodes::all::OP_SHA256)
        .push_slice(secret_hash)
        .push_opcode(opcodes::all::OP_EQUALVERIFY)
        .push_slice(redeemer_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .into_script()
}

/// Creates a Bitcoin script that allows refunding after a timelock expires.
///
/// Script: `<timelock> OP_CSV OP_DROP <initiator_pubkey> OP_CHECKSIG`
///
/// # Arguments
/// * `timelock` - Number of blocks to lock the funds
/// * `initiator_pubkey` - Public key of the initiator who can claim the refund
pub fn refund_leaf(timelock: u64, initiator_pubkey: &XOnlyPublicKey) -> ScriptBuf {
    Builder::new()
        .push_int(timelock as i64)
        .push_opcode(opcodes::all::OP_CSV)
        .push_opcode(opcodes::all::OP_DROP)
        .push_slice(initiator_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .into_script()
}

/// Creates a Bitcoin script that requires both initiator and redeemer signatures
/// for instant refund.
///
/// Script: `<initiator_pubkey> OP_CHECKSIG <redeemer_pubkey> OP_CHECKSIGADD 2 OP_NUMEQUAL`
///
/// # Arguments
/// * `initiator_pubkey` - Public key of the initiator
/// * `redeemer_pubkey` - Public key of the redeemer
pub fn instant_refund_leaf(
    initiator_pubkey: &XOnlyPublicKey,
    redeemer_pubkey: &XOnlyPublicKey,
) -> ScriptBuf {
    Builder::new()
        .push_slice(initiator_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .push_slice(redeemer_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIGADD)
        .push_int(2)
        .push_opcode(opcodes::all::OP_NUMEQUAL)
        .into_script()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    const TEST_SECRET_HASH: &str =
        "c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6";
    const TEST_INITIATOR_PUBKEY: &str =
        "c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66f";
    const TEST_REDEEMER_PUBKEY: &str =
        "1db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fb";
    const TEST_TIMELOCK: u64 = 144;

    fn get_secret_hash_bytes() -> [u8; 32] {
        let secret_hash = hex::decode(TEST_SECRET_HASH).expect("Valid hex string");
        let mut secret_hash_bytes = [0u8; 32];
        secret_hash_bytes.copy_from_slice(&secret_hash);
        secret_hash_bytes
    }

    #[test]
    fn redeem_leaf_contains_op_sha256() {
        let secret_hash = get_secret_hash_bytes();
        let redeemer_pubkey =
            XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY).expect("Valid redeemer public key");

        let script = redeem_leaf(&secret_hash, &redeemer_pubkey);
        let bytes = script.as_bytes();

        // OP_SHA256 is 0xa8
        assert!(
            bytes.contains(&0xa8),
            "Redeem leaf must contain OP_SHA256 (0xa8)"
        );
    }

    #[test]
    fn redeem_leaf_matches_garden_rs() {
        let secret_hash = get_secret_hash_bytes();
        let redeemer_pubkey =
            XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY).expect("Valid redeemer public key");

        let script = redeem_leaf(&secret_hash, &redeemer_pubkey);

        assert_eq!(
            script.as_bytes(),
            ScriptBuf::from_hex(
                "a820c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c688201db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fbac"
            )
            .expect("Valid script hex")
            .as_bytes()
        );
    }

    #[test]
    fn refund_leaf_contains_op_csv() {
        let initiator_pubkey =
            XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY).expect("Valid initiator public key");

        let script = refund_leaf(TEST_TIMELOCK, &initiator_pubkey);
        let bytes = script.as_bytes();

        // OP_CSV is 0xb2
        assert!(
            bytes.contains(&0xb2),
            "Refund leaf must contain OP_CSV (0xb2)"
        );
    }

    #[test]
    fn refund_leaf_matches_garden_rs() {
        let initiator_pubkey =
            XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY).expect("Valid initiator public key");

        let script = refund_leaf(TEST_TIMELOCK, &initiator_pubkey);

        assert_eq!(
            script.as_bytes(),
            ScriptBuf::from_hex(
                "029000b27520c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66fac"
            )
            .expect("Valid script hex")
            .as_bytes()
        );
    }

    #[test]
    fn instant_refund_leaf_contains_op_checksigadd() {
        let initiator_pubkey =
            XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY).expect("Valid initiator public key");
        let redeemer_pubkey =
            XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY).expect("Valid redeemer public key");

        let script = instant_refund_leaf(&initiator_pubkey, &redeemer_pubkey);
        let bytes = script.as_bytes();

        // OP_CHECKSIGADD is 0xba
        assert!(
            bytes.contains(&0xba),
            "Instant refund leaf must contain OP_CHECKSIGADD (0xba)"
        );
    }

    #[test]
    fn instant_refund_leaf_matches_garden_rs() {
        let initiator_pubkey =
            XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY).expect("Valid initiator public key");
        let redeemer_pubkey =
            XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY).expect("Valid redeemer public key");

        let script = instant_refund_leaf(&initiator_pubkey, &redeemer_pubkey);

        assert_eq!(
            script.as_bytes(),
            ScriptBuf::from_hex(
                "20c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66fac201db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fbba529c"
            )
            .expect("Valid script hex")
            .as_bytes()
        );
    }
}
