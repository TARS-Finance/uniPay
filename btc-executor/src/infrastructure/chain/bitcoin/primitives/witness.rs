//! Witness construction for Bitcoin HTLC Taproot spending paths.
//!
//! Vendored from garden-rs `crates/bitcoin/src/htlc/witness.rs`, adapted to
//! remove `SignaturePlaceholder` dependency and use a local placeholder constant.

use super::{
    error::BitcoinPrimitivesError,
    htlc::{get_control_block, HTLCLeaf, HTLCParams},
    script::{instant_refund_leaf, redeem_leaf, refund_leaf},
};
use bitcoin::Witness;

/// A 65-byte zero array used as a placeholder for Schnorr signatures.
///
/// 65 bytes = 64-byte Schnorr signature + 1-byte sighash type.
/// This matches the final signed size so that fee estimation during the
/// iterative fee-builder loop is accurate.
///
/// When building a witness template (before signing), this placeholder is
/// inserted where a real Schnorr signature will later be placed. The
/// `add_signature_to_witness` function in `sig.rs` replaces this placeholder
/// with a real signature.
const SCHNORR_SIG_PLACEHOLDER: [u8; 65] = [0u8; 65];

/// Builds a witness for a redeem transaction.
///
/// The witness stack layout (bottom to top):
/// 1. Signature placeholder (to be replaced with real sig)
/// 2. Secret preimage
/// 3. Redeem script
/// 4. Control block
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys and secret hash
/// * `secret` - Secret preimage used to redeem the HTLC
pub fn get_redeem_witness(
    htlc_params: &HTLCParams,
    secret: &[u8],
) -> Result<Witness, BitcoinPrimitivesError> {
    let redeem_script = redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey);

    let control_block_serialized = {
        let control_block = get_control_block(htlc_params, HTLCLeaf::Redeem)?;
        control_block.serialize()
    };

    let mut witness = Witness::new();
    witness.push(SCHNORR_SIG_PLACEHOLDER);
    witness.push(secret);
    witness.push(redeem_script);
    witness.push(control_block_serialized);

    Ok(witness)
}

/// Builds a witness for a refund transaction.
///
/// The witness stack layout (bottom to top):
/// 1. Signature placeholder (to be replaced with real sig)
/// 2. Refund script
/// 3. Control block
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys and timelock
pub fn get_refund_witness(htlc_params: &HTLCParams) -> Result<Witness, BitcoinPrimitivesError> {
    let refund_script = refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey);

    let control_block_serialized = {
        let control_block = get_control_block(htlc_params, HTLCLeaf::Refund)?;
        control_block.serialize()
    };

    let mut witness = Witness::new();
    witness.push(SCHNORR_SIG_PLACEHOLDER);
    witness.push(refund_script);
    witness.push(control_block_serialized);

    Ok(witness)
}

/// Builds a witness for an instant refund transaction.
///
/// The witness stack layout (bottom to top):
/// 1. Redeemer signature (provided)
/// 2. Initiator signature (provided)
/// 3. Instant refund script
/// 4. Control block
///
/// Unlike the other witness builders, this takes actual signature bytes
/// rather than placeholders because both parties sign cooperatively.
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including both parties' keys
/// * `initiator_sig` - Raw initiator signature bytes
/// * `redeemer_sig` - Raw redeemer signature bytes
pub fn get_instant_refund_witness(
    htlc_params: &HTLCParams,
    initiator_sig: &[u8],
    redeemer_sig: &[u8],
) -> Result<Witness, BitcoinPrimitivesError> {
    let instant_refund_script =
        instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey);

    let control_block_serialized = {
        let control_block = get_control_block(htlc_params, HTLCLeaf::InstantRefund)?;
        control_block.serialize()
    };

    let mut witness = Witness::new();
    witness.push(redeemer_sig);
    witness.push(initiator_sig);
    witness.push(instant_refund_script);
    witness.push(control_block_serialized);

    Ok(witness)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::XOnlyPublicKey;
    use std::str::FromStr;

    const TEST_SECRET_HASH: &str =
        "c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6";
    const TEST_INITIATOR_PUBKEY: &str =
        "c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66f";
    const TEST_REDEEMER_PUBKEY: &str =
        "1db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fb";

    fn make_test_params() -> HTLCParams {
        let initiator_pubkey = XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY).unwrap();
        let redeemer_pubkey = XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY).unwrap();
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&hex::decode(TEST_SECRET_HASH).unwrap());

        HTLCParams {
            initiator_pubkey,
            redeemer_pubkey,
            amount: 50_000,
            secret_hash,
            timelock: 144,
        }
    }

    #[test]
    fn redeem_witness_has_four_elements() {
        let params = make_test_params();
        let secret = [0xab_u8; 32];
        let witness = get_redeem_witness(&params, &secret).unwrap();
        // sig placeholder, secret, script, control block
        assert_eq!(witness.len(), 4, "Redeem witness must have 4 elements");
    }

    #[test]
    fn refund_witness_has_three_elements() {
        let params = make_test_params();
        let witness = get_refund_witness(&params).unwrap();
        // sig placeholder, script, control block
        assert_eq!(witness.len(), 3, "Refund witness must have 3 elements");
    }

    #[test]
    fn instant_refund_witness_has_four_elements() {
        let params = make_test_params();
        let fake_sig = [0xcd_u8; 64];
        let witness = get_instant_refund_witness(&params, &fake_sig, &fake_sig).unwrap();
        // redeemer sig, initiator sig, script, control block
        assert_eq!(
            witness.len(),
            4,
            "Instant refund witness must have 4 elements"
        );
    }

    #[test]
    fn redeem_witness_first_element_is_placeholder() {
        let params = make_test_params();
        let secret = [0xab_u8; 32];
        let witness = get_redeem_witness(&params, &secret).unwrap();
        let first = witness.nth(0).unwrap();
        assert_eq!(
            first.len(),
            65,
            "Placeholder must be 65 bytes (64 sig + 1 sighash type)"
        );
        assert!(
            first.iter().all(|&b| b == 0),
            "Placeholder must be all zeros"
        );
    }
}
