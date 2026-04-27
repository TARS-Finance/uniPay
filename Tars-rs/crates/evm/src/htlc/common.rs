use crate::{
    errors::{EvmError, HTLCError},
    primitives::Refund,
};
use alloy::{
    dyn_abi::Eip712Domain,
    primitives::{Bytes, FixedBytes},
    signers::{k256::ecdsa::SigningKey, local::LocalSigner, Signer},
};

pub async fn get_instant_refund_signature(
    domain: &Eip712Domain,
    signer: &LocalSigner<SigningKey>,
    order_id: FixedBytes<32>,
) -> Result<Bytes, HTLCError> {
    let refund = Refund { orderId: order_id };
    let signature = signer
        .sign_typed_data(&refund, &domain)
        .await
        .map_err(|e| HTLCError::EvmError(EvmError::SignatureError(e)))?;
    Ok(Bytes::from(signature.as_bytes()))
}
