use crate::{
    batcher::validate::validate_utxos_and_inputs,
    htlc::{hash::TapScriptSpendSigHashGenerator, tx::create_previous_outputs},
    ValidSpendRequests,
};
use bitcoin::{
    key::{Keypair, Secp256k1},
    secp256k1::{All, Message},
    TapSighashType, Transaction, TxIn, Witness,
};
use eyre::{bail, eyre, Result};

/// The TapSighashType used for SACP (Signature Adaptable Commitment Protocol).
pub const SACP_SIGHASH_TYPE: TapSighashType = TapSighashType::SinglePlusAnyoneCanPay;

/// The size of a Schnorr signature, which is 64 bytes plus 1 byte for the sighash type.
pub const SCHNORR_SIGNATURE_SIZE: usize = 65;

/// A placeholder for different signature types.
///
/// This enum represents the different placeholder types used for signatures in the
/// signing process, allowing for easy serialization and deserialization of signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignaturePlaceholder {
    /// A Taproot Schnorr signature placeholder.
    TaprootSchnorr,
}

impl SignaturePlaceholder {
    /// Converts the signature placeholder into a byte slice.
    ///
    /// # Returns
    /// * `&'static [u8]` - The byte representation of the placeholder.
    pub fn as_bytes(&self) -> &'static [u8] {
        match self {
            SignaturePlaceholder::TaprootSchnorr => b"add_signature_segwit_v1",
        }
    }

    /// Converts a byte slice into a `SignaturePlaceholder`.
    ///
    /// # Arguments
    /// * `bytes` - The byte slice to be converted into a `SignaturePlaceholder`.
    ///
    /// # Returns
    /// * `Some(SignaturePlaceholder)` if the byte slice matches a valid signature placeholder.
    /// * `None` if the byte slice doesn't match any placeholder.
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        match bytes {
            b"add_signature_segwit_v1" => Some(SignaturePlaceholder::TaprootSchnorr),
            _ => None,
        }
    }

    /// Returns the size of the signature placeholder in bytes.
    ///
    /// # Returns
    /// * `usize` - The size of the signature placeholder.
    pub fn size(&self) -> usize {
        match self {
            Self::TaprootSchnorr => SCHNORR_SIGNATURE_SIZE,
        }
    }
}

/// Signs a batch transaction by generating and applying signatures to all inputs.
///
/// This function signs each input in the batch transaction using the appropriate keypair
/// and signature scheme. It generates a SIGHASH for each input and applies the corresponding
/// signature. The function iterates through all spend requests, validates UTXOs, and applies
/// the necessary signatures for each input.
///
/// # Arguments
/// * `tx` - A mutable reference to the transaction that needs to be signed.
/// * `spend_requests` - A slice of spend requests that contain the UTXOs, scripts, and keypairs for signing.
///
/// # Returns
/// * `Ok(())` if the transaction is signed successfully.
/// * `Err` if any error occurs during signing or validation.
pub fn sign_batch_tx(tx: &mut Transaction, spend_requests: &ValidSpendRequests) -> Result<()> {
    let spend_requests = spend_requests.as_ref();

    let secp = Secp256k1::new();

    let mut input_index = 0;

    for spend_request in spend_requests.iter() {
        let utxos_len = spend_request.utxos.len();

        validate_utxos_and_inputs(
            &spend_request.utxos,
            &tx.input[input_index..input_index + utxos_len],
        )?;

        let mut sighash_generator = TapScriptSpendSigHashGenerator::new(
            tx.clone(),
            spend_request.script.tapscript_leaf_hash(),
        );

        let previous_outputs =
            create_previous_outputs(&spend_request.utxos, &spend_request.htlc_address);

        // Sanity check
        if previous_outputs.len() != utxos_len {
            bail!(
                "Number of previous outputs does not match number of UTXOs for spend request : {}",
                spend_request.id
            );
        }

        for (i, previous_output) in previous_outputs.iter().enumerate() {
            let current_input_index = input_index + i;
            let sighash = sighash_generator.with_prevout(
                current_input_index,
                previous_output,
                SACP_SIGHASH_TYPE,
            )?;

            add_signature_request(
                &secp,
                &mut tx.input[current_input_index],
                &spend_request.keypair,
                &sighash,
                SACP_SIGHASH_TYPE,
            )?;
        }

        input_index += spend_request.utxos.len();
    }

    Ok(())
}

/// Adds a signature to the transaction input's witness based on the provided sighash and signature type.
///
/// This function signs a given input by generating the appropriate signature using the provided keypair
/// and applying the corresponding sighash type. The signature is added to the input's witness stack.
/// Currently, it supports Schnorr signatures for Taproot, but other signature types can be added in the future.
///
/// # Arguments
/// * `secp` - A reference to the `Secp256k1` context used for signing (required for Schnorr signatures).
/// * `input` - A mutable reference to the transaction input that needs to be signed.
/// * `keypair` - The keypair used to sign the transaction.
/// * `sighash` - The pre-generated sighash that needs to be signed.
/// * `sighash_type` - The type of the sighash used for signing.
///
/// # Returns
/// * `Ok(())` if the signature is successfully added to the input's witness.
/// * `Err` if an error occurs during signing or adding the signature.
pub fn add_signature_request(
    secp: &Secp256k1<All>,
    input: &mut TxIn,
    keypair: &Keypair,
    sighash: &[u8; 32],
    sighash_type: TapSighashType,
) -> Result<()> {
    let witness = &input.witness;
    if witness.is_empty() {
        bail!("Input witness is empty, cannot sign")
    }

    let mut signed_witness = Witness::new();

    for item in witness.iter() {
        match SignaturePlaceholder::from_bytes(item) {
            Some(SignaturePlaceholder::TaprootSchnorr) => {
                let msg = Message::from_digest_slice(sighash)
                    .map_err(|e| eyre!("Failed to create Schnorr message from sig_hash: {e}"))?;

                let sig = secp.sign_schnorr(&msg, keypair);

                let schnorr_sig = bitcoin::taproot::Signature {
                    signature: sig,
                    sighash_type,
                };

                signed_witness.push(schnorr_sig.serialize().to_vec());
            }
            None => {
                signed_witness.push(item.to_vec());
            }
        }
    }

    // Add signed witness to the input
    input.witness = signed_witness;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        batcher::fee::{adjust_outputs_for_fee, estimate_fee},
        htlc::tx::{
            create_inputs_from_utxos, create_outputs, get_output_values, DEFAULT_TX_LOCKTIME,
            DEFAULT_TX_VERSION,
        },
        test_utils::{
            generate_bitcoin_random_keypair, get_dummy_sighash, get_test_bitcoin_indexer,
            get_test_spend_request, TEST_FEE_RATE,
        },
        validate_schnorr_signature, FeeRate,
    };
    use bitcoin::{Sequence, Transaction, Witness};
    use eyre::bail;

    #[tokio::test]
    async fn test_sign_batch_tx() -> Result<()> {
        let spend_requests = {
            let requests =
                futures::future::join_all((0..2).map(|_| async { get_test_spend_request().await }))
                    .await;
            let mut spend_requests = Vec::new();
            for request in requests {
                let spend_request = request?;
                spend_requests.push(spend_request);
            }
            spend_requests
        };

        let indexer = get_test_bitcoin_indexer()?;
        let spend_requests = ValidSpendRequests::validate(spend_requests, &indexer).await?;

        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        for spend_request in spend_requests.as_ref().iter() {
            if spend_request.utxos.is_empty() {
                bail!(
                    "No UTXOs found for the spend request : {:?}",
                    spend_request.id
                );
            }

            inputs.extend(create_inputs_from_utxos(
                &spend_request.utxos,
                &spend_request.witness,
                Sequence::MAX,
            ));

            let output_values = get_output_values(&spend_request.utxos, SACP_SIGHASH_TYPE)?;
            outputs.extend(create_outputs(
                output_values,
                &spend_request.recipient,
                None,
            )?);
        }

        let fee = estimate_fee(&inputs, &outputs, FeeRate::new(TEST_FEE_RATE)?);

        adjust_outputs_for_fee(&mut outputs, fee)?;

        let mut tx = Transaction {
            version: DEFAULT_TX_VERSION,
            lock_time: DEFAULT_TX_LOCKTIME,
            input: inputs,
            output: outputs,
        };

        sign_batch_tx(&mut tx, &spend_requests)?;

        let indexer = get_test_bitcoin_indexer()?;
        indexer.submit_tx(&tx).await?;

        Ok(())
    }

    #[test]
    fn test_signature_placeholder_to_bytes() {
        let placeholder = SignaturePlaceholder::TaprootSchnorr;
        assert_eq!(placeholder.as_bytes(), b"add_signature_segwit_v1");
    }

    #[test]
    fn test_signature_placeholder_from_bytes() {
        let bytes = b"add_signature_segwit_v1";
        assert_eq!(
            SignaturePlaceholder::from_bytes(bytes),
            Some(SignaturePlaceholder::TaprootSchnorr)
        );

        let invalid_bytes = b"invalid_placeholder";
        assert_eq!(SignaturePlaceholder::from_bytes(invalid_bytes), None);
    }

    #[test]
    fn test_signature_placeholder_size() {
        let placeholder = SignaturePlaceholder::TaprootSchnorr;
        assert_eq!(placeholder.size(), SCHNORR_SIGNATURE_SIZE);
    }

    #[test]
    fn test_add_signature_request_with_placeholder() -> Result<()> {
        let secp = Secp256k1::new();
        let keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let witness = Witness::from_slice(&[b"add_signature_segwit_v1"]);
        let mut input = TxIn::default();
        input.witness = witness;
        let result =
            add_signature_request(&secp, &mut input, &keypair, &sighash, SACP_SIGHASH_TYPE);

        assert!(result.is_ok());

        let signed_witness = input.witness;

        // Ensure the signed witness has a signature at the correct index
        let signature = &signed_witness[0];
        assert_eq!(signature.len(), SCHNORR_SIGNATURE_SIZE);

        let verifying_pubkey = keypair.x_only_public_key().0;
        assert!(validate_schnorr_signature(
            &verifying_pubkey,
            &signature,
            &sighash,
            SACP_SIGHASH_TYPE
        )
        .is_ok());

        Ok(())
    }

    #[test]
    fn test_add_signature_request_without_placeholder() -> Result<()> {
        let secp = Secp256k1::new();
        let keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let witness = Witness::from_slice(&[b"some_data".to_vec()]);
        let mut input = TxIn::default();
        input.witness = witness.clone();
        let result =
            add_signature_request(&secp, &mut input, &keypair, &sighash, SACP_SIGHASH_TYPE);

        assert!(result.is_ok());
        let signed_witness = input.witness;

        // Ensure the witness is unchanged
        assert_eq!(signed_witness, witness);

        Ok(())
    }

    #[test]
    fn test_add_signature_request_with_empty_witness() {
        let secp = Secp256k1::new();
        let keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let empty_witness = Witness::new();
        let mut input = TxIn::default();
        input.witness = empty_witness.clone();
        let result =
            add_signature_request(&secp, &mut input, &keypair, &sighash, SACP_SIGHASH_TYPE);

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Input witness is empty, cannot sign"
        );
    }

    #[test]
    fn test_add_signature_request_with_invalid_keypair() -> Result<()> {
        let secp = Secp256k1::new();
        let valid_keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let invalid_keypair = generate_bitcoin_random_keypair();

        let witness = Witness::from_slice(&[b"add_signature_segwit_v1"]);
        let mut input = TxIn::default();
        input.witness = witness.clone();
        let result = add_signature_request(
            &secp,
            &mut input,
            &invalid_keypair,
            &sighash,
            SACP_SIGHASH_TYPE,
        );

        assert!(result.is_ok());

        let signed_witness = input.witness;

        let signature = &signed_witness[0];

        let valid_verifying_key = valid_keypair.x_only_public_key().0;

        // Should be invalid as the sigh hash is signed by invalid_keypair
        assert!(validate_schnorr_signature(
            &valid_verifying_key,
            &signature,
            &sighash,
            SACP_SIGHASH_TYPE
        )
        .is_err());

        Ok(())
    }

    #[test]
    fn test_add_signature_request_with_sighash_type() -> Result<()> {
        let secp = Secp256k1::new();
        let keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let witness = Witness::from_slice(&[b"add_signature_segwit_v1"]);
        let mut input = TxIn::default();
        input.witness = witness.clone();
        let result =
            add_signature_request(&secp, &mut input, &keypair, &sighash, SACP_SIGHASH_TYPE);

        assert!(result.is_ok());
        let signed_witness = input.witness;

        // Check that the sighash type is correct (SACP_SIGHASH_TYPE)
        let signature: &[u8] = &signed_witness[0];
        assert_eq!(signature[signature.len() - 1], SACP_SIGHASH_TYPE as u8);

        Ok(())
    }

    #[test]
    fn test_add_siganture_request_verifying_witness() -> Result<()> {
        let secp = Secp256k1::new();
        let keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let witness =
            Witness::from_slice(&[b"add_signature_segwit_v1", b"add_signature_segwit_v2"]);

        let mut input = TxIn::default();
        input.witness = witness.clone();

        let result =
            add_signature_request(&secp, &mut input, &keypair, &sighash, SACP_SIGHASH_TYPE);

        assert!(result.is_ok());
        let signed_witness = input.witness;

        assert!(signed_witness.len() == witness.len());

        Ok(())
    }
}
