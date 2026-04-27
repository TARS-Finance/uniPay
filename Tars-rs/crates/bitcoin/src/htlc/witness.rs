use crate::{
    batcher::sign::SignaturePlaceholder,
    htlc::{
        htlc::get_control_block,
        script::{redeem_leaf, refund_leaf},
    },
    HTLCLeaf, HTLCParams,
};
use bitcoin::Witness;
use eyre::Result;

/// Builds a witness for a refund transaction.
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys and timelock
/// * `signature` - Signature of the initiator for the refund transaction
///
/// # Returns
/// * `Witness` - The witness for the refund transaction
pub fn get_refund_witness(htlc_params: &HTLCParams) -> Result<Witness> {
    // Get the refund leaf
    let refund_leaf = refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey);

    // Serialize the control block for the refund leaf
    let control_block_serialized = {
        let control_block = get_control_block(htlc_params, HTLCLeaf::Refund)?;
        control_block.serialize()
    };

    // Build the refund witness
    let mut witness = Witness::new();

    witness.push(SignaturePlaceholder::TaprootSchnorr.as_bytes());
    witness.push(refund_leaf);
    witness.push(control_block_serialized);

    Ok(witness)
}

/// Builds a witness for a redeem transaction.
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys and secret hash
/// * `secret` - Secret used to redeem the HTLC
///
/// # Returns
/// * `Witness` - The witness for the redeem transaction
pub fn get_redeem_witness(htlc_params: &HTLCParams, secret: &[u8]) -> Result<Witness> {
    // Get the redeem script.
    let redeem_script = redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey);

    // Serialize the redeem control block into bytes.
    let control_block_serialized = {
        let control_block = get_control_block(htlc_params, HTLCLeaf::Redeem)?;
        control_block.serialize()
    };

    // Build the redeem witness
    let mut witness = Witness::new();

    witness.push(SignaturePlaceholder::TaprootSchnorr.as_bytes());
    witness.push(secret);
    witness.push(redeem_script);
    witness.push(control_block_serialized);

    Ok(witness)
}
