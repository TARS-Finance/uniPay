use crate::{generate_instant_refund_hash, indexer::primitives::Utxo, HTLCParams};
use alloy::hex::{self, ToHexExt};
use bitcoin::{
    consensus, hashes::Hash, key::Secp256k1, secp256k1::Message, taproot::Signature, Address,
    Network, TapSighash, TapSighashType, Transaction, XOnlyPublicKey,
};
use eyre::{bail, eyre, Result};
use sha2::{Digest, Sha256};
use utils::ToBytes;

/// Verifies a Bitcoin SACP (Single Plus Anyone Can Pay) instant refund transaction
///
/// 1. Validates that the number of inputs matches the number of expected hashes
/// 2. Verifies Schnorr signatures for each input using the initiator's public key
/// 3. Ensures at least one input references the expected initiate transaction hash
///
/// # Arguments
/// * `instant_refund_sacp_hex` - Hex-encoded bytes of the instant refund SACP transaction
/// * `initiate_tx_hash` - Hash of the original initiate transaction
/// * `htlc_params` - HTLC parameters containing public keys and other contract details
/// * `utxos` - List of UTXOs used as inputs for the instant refund transaction
/// * `recipient` - Bitcoin address that will receive the refunded funds
/// * `network` - Bitcoin network (mainnet, testnet, regtest)
///
/// # Returns
/// * `Ok(())` - Transaction is valid and properly signed
pub fn validate_instant_refund_sacp_tx(
    instant_refund_sacp_hex: &str,
    initiate_tx_hash: &str,
    htlc_params: &HTLCParams,
    utxos: &[Utxo],
    recipient: &Address,
    network: Network,
) -> Result<()> {
    let initiate_tx_hash = initiate_tx_hash.to_lowercase();

    let instant_refund_sacp_bytes = hex::decode(instant_refund_sacp_hex).map_err(|e| {
        eyre!(
            "Failed to decode instant refund SACP transaction bytes: {:?}",
            e
        )
    })?;

    let instant_refund_sacp: Transaction =
        bitcoin::consensus::deserialize(&instant_refund_sacp_bytes).map_err(|e| {
            eyre!(
                "Failed to deserialize instant refund SACP transaction: {:?}",
                e
            )
        })?;

    let input_total: u64 = utxos.iter().map(|utxo| utxo.value).sum();
    let output_total: u64 = instant_refund_sacp
        .output
        .iter()
        .map(|output| output.value.to_sat())
        .sum();

    if output_total > input_total {
        return Err(eyre!(
            "Total output value ({}) exceeds input total ({}) in HTLC params",
            output_total,
            input_total
        ));
    }

    let fee = input_total - output_total;

    let hashes = generate_instant_refund_hash(&htlc_params, &utxos, &recipient, network, Some(fee))
        .map_err(|e| eyre!("Failed to generate instant refund hash : {:?}", e))?;

    if instant_refund_sacp.input.len() != hashes.len() {
        bail!("Mismatch between transaction inputs and expected hashes");
    }

    let mut has_matching_input = false;

    for (input, hash) in instant_refund_sacp.input.iter().zip(hashes) {
        if input.witness.len() != 4 {
            bail!("Instant refund SACP transaction input witness must have exactly 4 elements")
        }

        let signature = input
            .witness
            .nth(1)
            .ok_or_else(|| eyre!("Missing initiator's signature in SACP transaction input"))?;

        validate_schnorr_signature(
            &htlc_params.initiator_pubkey,
            &signature,
            &hash,
            TapSighashType::SinglePlusAnyoneCanPay,
        )
        .map_err(|e| eyre!("Invalid Schnorr signature : {:?}", e))?;

        if initiate_tx_hash.eq(&input.previous_output.txid.to_string()) {
            has_matching_input = true;
        }
    }

    if !has_matching_input {
        bail!("Input txid does not match expected transaction hash");
    }

    Ok(())
}

/// Validates a transaction
///
/// 1. Validates that the inputs match the expected transaction hash
/// 2. Validates that the outputs match the expected recipient
///
/// # Arguments
/// * `tx_bytes` - Hex-encoded bytes of the transaction
/// * `input_tx_hash` - Transaction id (txid)
/// * `recipient` - Bitcoin address that will receive the redeemed funds
///
/// # Returns
/// * `Result<Transaction>` - The transaction if valid
/// * `Err` - If the transaction is invalid
pub fn validate_tx(
    tx_bytes: &[u8],
    input_tx_hash: &str,
    recipient: &Address,
) -> Result<Transaction> {
    let tx: Transaction = consensus::deserialize(&tx_bytes)
        .map_err(|e| eyre!("Failed to deserialie tx : {:#?}", e))?;

    // Validate the inputs
    for input in tx.input.iter() {
        if input.previous_output.txid.to_string() != input_tx_hash {
            bail!("Tx has invalid inputs");
        }
    }

    // Validate the outputs
    for output in tx.output.iter() {
        if output.script_pubkey != recipient.script_pubkey() {
            bail!("Tx has invalid outputs");
        }
    }

    Ok(tx)
}

/// Validates a Schnorr signature against a public key for Bitcoin transactions.
///
/// This function:
/// 1. Validates the binary-encoded Schnorr signature and message hash.
/// 2. Performs cryptographic verification against the provided public key.
///
/// # Arguments
/// * `verifying_key` - The XOnly public key used for signature verification.
/// * `signature` - The raw bytes of the taproot Schnorr signature (including sighash type).
/// * `message_hash` - The raw bytes of the message hash to verify.
/// * `hash_type` - The expected sighash type for the signature.
///
/// # Returns
/// * `Ok(())` if the provided signature is valid.
/// * `Err` with a descriptive message if validation fails.
pub fn validate_schnorr_signature(
    verifying_key: &XOnlyPublicKey,
    signature: &[u8],
    message_hash: &[u8],
    hash_type: TapSighashType,
) -> Result<()> {
    let secp = Secp256k1::verification_only();

    let signature = Signature::from_slice(signature)
        .map_err(|e| eyre!(format!("Invalid Schnorr signature format: {}", e)))?;

    if !hash_type.eq(&signature.sighash_type) {
        bail!(
            "Invalid signature hash type: expected {}, got {}",
            hash_type,
            signature.sighash_type
        )
    }

    let message_hash = TapSighash::from_slice(message_hash)
        .map_err(|e| eyre!(format!("Invalid message hash format: {}", e)))?;

    let msg = Message::from(message_hash);

    if let Err(e) = secp.verify_schnorr(&signature.signature, &msg, verifying_key) {
        bail!(format!("Signature verification failed: {}", e))
    }

    Ok(())
}

/// Validates UTXO inputs
///
/// Checks that all UTXOs have positive values and are properly formatted.
///
/// # Arguments
/// * `utxos` - Slice of UTXOs to validate
///
/// # Returns
/// * `Result<()>` - Ok if all UTXOs are valid
///
/// # Errors
/// * When any UTXO has zero or invalid value
pub fn validate_utxos(utxos: &[Utxo]) -> Result<()> {
    if utxos.is_empty() {
        bail!("No UTXOs provided")
    }

    for utxo in utxos {
        if utxo.value == 0 {
            bail!(
                "UTXO with txid {} and vout {} has zero value",
                utxo.txid,
                utxo.vout
            )
        }
    }

    Ok(())
}

/// Validates that the provided secret matches the expected secret hash.
///
/// # Arguments
/// * `secret` - The secret as a hex-encoded string
/// * `secret_hash` - The expected secret hash as a hex-encoded string
///
/// # Returns
/// * `Result<Vec<u8>>` - The decoded secret as a 32-byte vector if valid
pub fn validate_secret(secret: &str, secret_hash: &str) -> Result<Vec<u8>> {
    let secret_bytes = secret.hex_to_bytes()?;

    let hash = Sha256::digest(&secret_bytes);
    if hash.encode_hex() != secret_hash {
        bail!("Secret hash mismatch");
    }

    Ok(secret_bytes.to_vec())
}

/// Validates htlc parameters for production safety.
///
/// # Arguments
/// * `htlc_params` - HTLC params information to validate
/// * `utxos` - UTXOs to validate
/// * `network` - Bitcoin network to validate against
///
/// # Returns
/// * `Result<()>` - Ok if all parameters are valid
pub fn validate_hash_generation_params(
    htlc_params: &HTLCParams,
    utxos: &[Utxo],
    network: Network,
) -> Result<()> {
    if utxos.is_empty() {
        bail!("No UTXOs provided")
    }

    if htlc_params.amount == 0 {
        bail!("Invalid swap amount: must be greater than zero")
    }

    if htlc_params.timelock == 0 {
        bail!("Invalid HTLC timelock: must be greater than zero")
    }

    // Validate UTXOs have positive values
    for utxo in utxos {
        if utxo.value == 0 {
            bail!("UTXO has zero value")
        }
    }

    // Validate network compatibility (you might want to add specific network checks)
    match network {
        Network::Bitcoin
        | Network::Testnet
        | Network::Signet
        | Network::Regtest
        | Network::Testnet4 => Ok(()),
        _ => bail!("Invalid network"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        fund_btc, get_htlc_address,
        htlc::{
            htlc::get_control_block,
            tx::{create_inputs_from_utxos, create_outputs, get_output_values},
        },
        instant_refund_leaf,
        test_utils::{
            generate_bitcoin_random_keypair, get_test_bitcoin_indexer, TEST_AMOUNT, TEST_NETWORK,
        },
        HTLCLeaf, UtxoStatus,
    };
    use bitcoin::{
        absolute::LockTime, consensus::serialize, key::Keypair, transaction::Version, Amount,
        ScriptBuf, Sequence, Txid, Witness,
    };
    use std::str::FromStr;

    #[tokio::test]
    async fn test_validate_schnorr_signature() {
        let secp = Secp256k1::new();

        let key_pair = generate_bitcoin_random_keypair();

        let initiator_pubkey = key_pair.x_only_public_key().0;

        // Random message hash for testing.
        let message_hash =
            hex::decode("a2bd5d6e85cdcfb0461022a87d7f9b7020555ed98c1ef3c49212f87a671eb7ec")
                .unwrap();

        let message = Message::from_digest_slice(&message_hash).unwrap();

        let sig = secp.sign_schnorr_no_aux_rand(&message, &key_pair);

        // Test with valid data
        assert!(validate_schnorr_signature(
            &initiator_pubkey,
            &sig.serialize(),
            &message_hash,
            TapSighashType::Default
        )
        .is_ok());

        // Test with invalid data
        let invalid_message_hash =
            hex::decode("a2bd5d6e85cdcfb0461022a87d7f9b7020555ed98c1ef3c49212f87a671eb7ef")
                .unwrap();

        assert!(validate_schnorr_signature(
            &initiator_pubkey,
            &sig.serialize(),
            &invalid_message_hash,
            TapSighashType::Default
        )
        .is_err());
    }

    #[test]
    fn test_validate_utxos() {
        // Test empty UTXOs
        let empty_utxos: Vec<Utxo> = vec![];
        let result = validate_utxos(&empty_utxos);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No UTXOs provided"),
            "Expected error about empty UTXO set"
        );

        // Test zero value UTXO
        let zero_value_utxos = vec![Utxo {
            txid: Txid::hash(&[1u8; 32]),
            vout: 0,
            value: 0,
            status: UtxoStatus {
                confirmed: true,
                block_height: Some(100),
            },
        }];
        let result = validate_utxos(&zero_value_utxos);
        assert!(result.is_err());
        assert!(
            result.unwrap_err().to_string().contains("zero value"),
            "Expected error about zero value UTXO"
        );

        // Test valid UTXOs
        let valid_utxos = vec![
            Utxo {
                txid: Txid::hash(&[1u8; 32]),
                vout: 0,
                value: 100_000,
                status: UtxoStatus {
                    confirmed: true,
                    block_height: Some(100),
                },
            },
            Utxo {
                txid: Txid::hash(&[2u8; 32]),
                vout: 1,
                value: 200_000,
                status: UtxoStatus {
                    confirmed: true,
                    block_height: Some(100),
                },
            },
            Utxo {
                txid: Txid::hash(&[0u8; 32]),
                vout: 2,
                value: 150_000,
                status: UtxoStatus {
                    confirmed: true,
                    block_height: Some(100),
                },
            },
        ];

        assert!(
            validate_utxos(&valid_utxos).is_ok(),
            "Expected validation to succeed with valid UTXOs"
        );
    }

    #[tokio::test]
    async fn test_validate_instant_refund_sacp_tx() -> Result<()> {
        // Setup test environment with regtest network
        let network = Network::Regtest;
        let secret_hash_str = "c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6";

        let indexer = get_test_bitcoin_indexer()?;

        let secp = Secp256k1::new();

        // Generate keypairs for initiator and redeemer
        let initiator_keypair = Keypair::new(&secp, &mut rand::thread_rng());
        let initiator_pubkey = initiator_keypair.x_only_public_key().0;

        let redeemer_keypair = Keypair::new(&secp, &mut rand::thread_rng());
        let redeemer_pubkey = redeemer_keypair.x_only_public_key().0;

        let mut secret_hash = [0u8; 32];

        // Ensure the string is exactly 32 bytes (or adjust the length).
        secret_hash.copy_from_slice(&secret_hash_str.as_bytes()[0..32]);

        // Create HTLC parameters for the test
        let htlc_params = HTLCParams {
            initiator_pubkey: initiator_pubkey.clone(),
            redeemer_pubkey: redeemer_pubkey.clone(),
            amount: 50000,
            secret_hash: secret_hash.clone(),
            timelock: 144,
        };

        let htlc_address = get_htlc_address(&htlc_params, network)?;
        let recipient = Address::from_str("bcrt1q8v8k050rrn2ggtxk3j57rwz2fnzvsttqy65vnr")?
            .require_network(network)?;

        // Fund the HTLC address with test BTC
        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        let initiate_tx_hash = utxos[0].txid.to_string();

        // Generate hashes that need to be signed for the instant refund transaction
        let hashes = generate_instant_refund_hash(&htlc_params, &utxos, &recipient, network, None)?;

        let mut initiator_signatures = Vec::new();
        let mut redeemer_signatures = Vec::new();

        // Create Schnorr signatures for each hash using both keypairs
        for hash in hashes {
            let msg = Message::from_digest_slice(&hash)?;

            let initiator_signature = bitcoin::taproot::Signature {
                sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
                signature: secp.sign_schnorr_no_aux_rand(&msg, &initiator_keypair),
            };
            let redeemer_signature = bitcoin::taproot::Signature {
                sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
                signature: secp.sign_schnorr_no_aux_rand(&msg, &redeemer_keypair),
            };

            initiator_signatures.push(initiator_signature);
            redeemer_signatures.push(redeemer_signature);
        }

        // Build the instant refund transaction
        let inputs = create_inputs_from_utxos(&utxos, &Witness::new(), Sequence::MAX);
        let output_values = get_output_values(&utxos, TapSighashType::SinglePlusAnyoneCanPay)?;
        let outputs = create_outputs(output_values, &recipient, None)?;

        let instant_refund_leaf_hash =
            instant_refund_leaf(&initiator_pubkey, &redeemer_pubkey).tapscript_leaf_hash();
        let control_block_serialized =
            get_control_block(&htlc_params, HTLCLeaf::InstantRefund)?.serialize();

        let mut instant_refund_sacp = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: inputs.clone(),
            output: outputs.clone(),
        };

        // Add witness data with valid initiator signatures
        for (i, input) in instant_refund_sacp.input.iter_mut().enumerate() {
            let mut witness = Witness::new();
            witness.push(initiator_signatures[i].serialize());
            witness.push(initiator_signatures[i].serialize());
            witness.push(instant_refund_leaf_hash.clone());
            witness.push(control_block_serialized.clone());

            input.witness = witness;
        }

        let instant_refund_tx_bytes = hex::encode(serialize(&instant_refund_sacp));

        // Test 1: Verify transaction with valid initiator signatures (should pass)
        let result = validate_instant_refund_sacp_tx(
            &instant_refund_tx_bytes,
            &initiate_tx_hash,
            &htlc_params,
            &utxos,
            &recipient,
            network,
        );

        assert!(result.is_ok());

        // Test 2: Verify transaction with invalid redeemer signatures (should fail)
        let mut instant_refund_sacp = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: inputs,
            output: outputs,
        };

        for (i, input) in instant_refund_sacp.input.iter_mut().enumerate() {
            let mut witness = Witness::new();
            witness.push(redeemer_signatures[i].serialize());
            witness.push(redeemer_signatures[i].serialize());
            witness.push(instant_refund_leaf_hash.clone());
            witness.push(control_block_serialized.clone());

            input.witness = witness;
        }

        let instant_refund_tx_bytes = hex::encode(serialize(&instant_refund_sacp));

        let result = validate_instant_refund_sacp_tx(
            &instant_refund_tx_bytes,
            &initiate_tx_hash,
            &htlc_params,
            &utxos,
            &recipient,
            network,
        );

        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_validate_tx() -> Result<()> {
        // Create a simple test transaction
        let input_txid = Txid::hash(&[1u8; 32]);
        let recipient =
            Address::from_str("bcrt1plzgfycjjpt4s4c6w3etgmfv6zmnc4yd6kvnwjl79vuxs4uzgchmqk4t4gh")?
                .require_network(TEST_NETWORK)?;

        // Create a transaction with one input and one output
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: bitcoin::OutPoint {
                    txid: input_txid,
                    vout: 0,
                },
                script_sig: ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: Witness::new(),
            }],
            output: vec![bitcoin::TxOut {
                value: Amount::from_sat(TEST_AMOUNT),
                script_pubkey: recipient.script_pubkey(),
            }],
        };

        let tx_bytes = serialize(&tx);

        // Test valid transaction
        let result = validate_tx(&tx_bytes, &input_txid.to_string(), &recipient);
        assert!(result.is_ok(), "Valid transaction should pass validation");

        // Test invalid input transaction hash
        let invalid_input_txid = Txid::hash(&[2u8; 32]);
        let result = validate_tx(&tx_bytes, &invalid_input_txid.to_string(), &recipient);
        assert!(
            result.is_err(),
            "Transaction with invalid input should fail validation"
        );
        assert!(result.unwrap_err().to_string().contains("invalid inputs"));

        // Test invalid recipient
        let different_recipient =
            Address::from_str("bcrt1pj58evdvnqn9s8xyrryy2nlvscjc9mvavxf5ffwy0thx3j6pca6fq26d3vd")?
                .require_network(TEST_NETWORK)?;
        let result = validate_tx(&tx_bytes, &input_txid.to_string(), &different_recipient);
        assert!(
            result.is_err(),
            "Transaction with invalid recipient should fail validation"
        );
        assert!(result.unwrap_err().to_string().contains("invalid outputs"));

        Ok(())
    }
}
