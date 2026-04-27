use bitcoin::{opcodes, script::Builder, secp256k1::XOnlyPublicKey, ScriptBuf};

/// Creates a Bitcoin script that allows spending with a secret preimage and redeemer's signature.
///
/// # Arguments
/// * `secret_hash` - SHA256 hash of the secret (32 bytes)
/// * `redeemer_pubkey` - Public key of the redeemer
///
/// # Returns
/// A script that verifies the preimage hash and checks the redeemer's signature
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
/// # Arguments
/// * `timelock` - Number of blocks to lock the funds
/// * `initiator_pubkey` - Public key of the initiator who can claim the refund
///
/// # Returns
/// A script that enforces the timelock and verifies the initiator's signature
pub fn refund_leaf(timelock: u64, initiator_pubkey: &XOnlyPublicKey) -> ScriptBuf {
    Builder::new()
        .push_int(timelock as i64)
        .push_opcode(opcodes::all::OP_CSV)
        .push_opcode(opcodes::all::OP_DROP)
        .push_slice(&initiator_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .into_script()
}

/// Creates a Bitcoin script that requires both initiator and redeemer signatures for instant refund.
///
/// # Arguments
/// * `initiator_pubkey` - Public key of the initiator
/// * `redeemer_pubkey` - Public key of the redeemer
///
/// # Returns
/// A script that enforces both parties must sign to execute the refund
pub fn instant_refund_leaf(
    initiator_pubkey: &XOnlyPublicKey,
    redeemer_pubkey: &XOnlyPublicKey,
) -> ScriptBuf {
    Builder::new()
        .push_slice(&initiator_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIG)
        .push_slice(&redeemer_pubkey.serialize())
        .push_opcode(opcodes::all::OP_CHECKSIGADD)
        .push_int(2)
        .push_opcode(opcodes::all::OP_NUMEQUAL)
        .into_script()
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;
    use alloy::hex;
    use super::*;

    // Test constants
    const TEST_SECRET_HASH: &str =
        "c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6";
    const TEST_INITIATOR_PUBKEY: &str =
        "c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66f";
    const TEST_REDEEMER_PUBKEY: &str =
        "1db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fb";
    const TEST_TIMELOCK: u64 = 144;

    // Reference transactions for HTLC script validation:
    // 1. Initiation TX: 1ee94f3c68aa3cfee6911bc2bd28899b2981cf2a877d9883fcd532aa548b43e5
    // 2. Redeem TX: 2c90e80c038f8ef1748d196c96fa5a07849f5ef54da9412c534578b0755db5a3

    /// Helper function to convert hex string to byte array
    fn get_secret_hash_bytes() -> [u8; 32] {
        let secret_hash = hex::decode(TEST_SECRET_HASH).expect("Valid hex string");
        let mut secret_hash_bytes = [0u8; 32];
        secret_hash_bytes.copy_from_slice(&secret_hash);
        secret_hash_bytes
    }

    #[test]
    fn test_redeem_leaf() {
        let secret_hash = get_secret_hash_bytes();
        let redeemer_pubkey = XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY)
            .expect("Valid redeemer public key");

        let script = redeem_leaf(&secret_hash, &redeemer_pubkey);

        assert!(!script.is_empty());
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
    fn test_refund_leaf() {
        let initiator_pubkey = XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY)
            .expect("Valid initiator public key");

        let script = refund_leaf(TEST_TIMELOCK, &initiator_pubkey);

        assert!(!script.is_empty());
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
    fn test_instant_refund_leaf() {
        let initiator_pubkey = XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY)
            .expect("Valid initiator public key");
        let redeemer_pubkey = XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY)
            .expect("Valid redeemer public key");

        let script = instant_refund_leaf(&initiator_pubkey, &redeemer_pubkey);

        assert!(!script.is_empty());
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
