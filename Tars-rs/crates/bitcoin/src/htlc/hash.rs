use super::{
    primitives::HTLCParams,
    tx::{build_tx, create_previous_outputs, sort_utxos},
};
use crate::{
    get_htlc_address, htlc::validate::validate_hash_generation_params, indexer::primitives::Utxo,
    instant_refund_leaf,
};
use bitcoin::{
    hashes::Hash,
    sighash::{Prevouts, SighashCache},
    Address, Network, Sequence, TapLeafHash, TapSighashType, Transaction, TxOut, Witness,
};
use eyre::{bail, eyre, Result};

/// Generates signature hashes for Taproot script path spending
///
/// This struct is used to generate signature message hashes for each transaction
/// input using the specified sighash type. These hashes are used for creating
/// Schnorr signatures for Taproot script path spending.
#[derive(Clone, Debug)]
pub struct TapScriptSpendSigHashGenerator {
    tx: Transaction,
    leaf_hash: TapLeafHash,
}

impl TapScriptSpendSigHashGenerator {
    /// Create a new `TapScriptSpendSigHashGenerator` instance
    ///
    /// # Arguments
    /// * `tx` - Transaction to generate hashes for
    /// * `leaf_hash` - Taproot script leaf hash for the spending path
    ///
    /// # Returns
    /// * `Self` - A new `TapScriptSpendSigHashGenerator` instance
    pub fn new(tx: Transaction, leaf_hash: TapLeafHash) -> Self {
        Self { tx, leaf_hash }
    }

    /// Generate a signature hash for a single input
    ///
    /// Generates a signature message hash for the specified input index using
    /// the specified sighash type.
    ///
    /// # Arguments
    /// * `input_index` - Index of the input to generate the hash for
    /// * `prevouts` - Previous output information for the input
    /// * `sighash_type` - HashType of an input's signature
    ///
    /// # Returns
    /// * `Result<[u8; 32]>` - Raw signature hash for the input
    ///
    /// # Errors
    /// * When signature hash computation fails for the input
    fn generate(
        &mut self,
        input_index: usize,
        prevouts: &Prevouts<TxOut>,
        sighash_type: TapSighashType,
    ) -> Result<[u8; 32]> {
        let mut sighash_cache = SighashCache::new(&mut self.tx);
        let sighash = sighash_cache
            .taproot_script_spend_signature_hash(
                input_index,
                prevouts,
                self.leaf_hash,
                sighash_type,
            )
            .map_err(|e| {
                eyre!(
                    "Failed to generate signature hash for input {}: {e}",
                    input_index
                )
            })?;

        Ok(sighash.to_raw_hash().to_byte_array())
    }

    /// Generate a signature hash for a single input with a single previous output
    ///
    /// Generates a signature message hash for the specified input index using
    /// the specified sighash type and a single previous output.
    ///
    /// # Arguments
    /// * `input_index` - Index of the input to generate the hash for
    /// * `previous_output` - Previous output information for the input
    /// * `sighash_type` - HashType of an input's signature
    ///
    /// # Returns
    /// * `Result<[u8; 32]>` - Raw signature hash for the input
    ///
    /// # Errors
    /// * When signature hash computation fails for the input
    pub fn with_prevout(
        &mut self,
        input_index: usize,
        previous_output: &TxOut,
        sighash_type: TapSighashType,
    ) -> Result<[u8; 32]> {
        let prevouts = Prevouts::One(input_index, previous_output.clone());
        self.generate(input_index, &prevouts, sighash_type)
    }

    /// Generate signature hashes for all inputs with all previous outputs
    ///
    /// Generates signature message hashes for all inputs using the specified
    /// sighash type and all previous outputs.
    ///
    /// # Arguments
    /// * `previous_outputs` - Previous output information for each input
    /// * `sighash_type` - HashType of an input's signature
    ///
    /// # Returns
    /// * `Result<Vec<[u8; 32]>>` - Raw signature hashes for all inputs
    ///
    /// # Errors
    /// * When signature hash computation fails for any input
    pub fn with_all_prevouts(
        &mut self,
        previous_outputs: &[TxOut],
        sighash_type: TapSighashType,
    ) -> Result<Vec<[u8; 32]>> {
        if self.tx.input.len() != previous_outputs.len() {
            bail!(
                "Number of transaction inputs ({}) does not match number of previous outputs ({})",
                self.tx.input.len(),
                previous_outputs.len()
            );
        }

        let mut sighashes = Vec::with_capacity(previous_outputs.len());
        let prevouts = Prevouts::All(previous_outputs);

        for input_index in 0..self.tx.input.len() {
            sighashes.push(self.generate(input_index, &prevouts, sighash_type)?);
        }

        Ok(sighashes)
    }
}

/// Generates transaction hashes that need to be signed for Bitcoin SACP (Signature Adaptable Commitment Protocol)
///
/// This function:
/// 1. Validates input parameters
/// 2. Retrieves and sorts UTXOs to ensure deterministic processing
/// 3. Creates transaction inputs and collects previous outputs using the HTLC address
/// 4. Creates a 1-to-1 mapping between inputs and outputs to conform with SINGLE|ANYONECANPAY sighash
/// 5. Deducts the optional fee from the output which has the max output value.
/// 6. Generates the taproot script leaf for instant refund
/// 7. Generates signature hashes for each input using taproot script spend
///
/// # Arguments
/// * `htlc_params` - Contains swap details including amount and public keys
/// * `utxos` - Slice of UTXOs to spend from the HTLC address
/// * `recipient` - Bitcoin address where funds will be sent
/// * `network` - Bitcoin network (e.g., Testnet, Mainnet)
/// * `fee` - Optional transaction fee to deduct from the output
///
/// # Returns
/// * `Result<Vec<[u8; 32]>>` - Raw signature hashes for each input
/// * `Err` with descriptive message if any step fails
pub fn generate_instant_refund_hash(
    htlc_params: &HTLCParams,
    utxos: &[Utxo],
    recipient: &Address,
    network: Network,
    fee: Option<u64>,
) -> Result<Vec<[u8; 32]>> {
    // Validate all input parameters
    validate_hash_generation_params(htlc_params, utxos, network)?;

    let sighash_type = TapSighashType::SinglePlusAnyoneCanPay;
    let utxos = sort_utxos(utxos);
    let htlc_address = get_htlc_address(htlc_params, network)?;
    let previous_outputs = create_previous_outputs(&utxos, &htlc_address);
    let instant_refund_tx = build_tx(
        &utxos,
        recipient,
        &Witness::new(),
        Sequence::MAX,
        sighash_type,
        fee,
    )?;

    let leaf_hash =
        instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey)
            .tapscript_leaf_hash();

    let mut sighash_generator = TapScriptSpendSigHashGenerator::new(instant_refund_tx, leaf_hash);

    let mut message_hashes = Vec::with_capacity(utxos.len());
    for input_index in 0..utxos.len() {
        message_hashes.push(sighash_generator.with_prevout(
            input_index,
            &previous_outputs[input_index],
            sighash_type,
        )?);
    }

    Ok(message_hashes)
}

#[cfg(test)]
mod tests {
    const TEST_SECRET_HASH: &str =
        "88475604255b1fb9d98c20a6d29d426c9ea5818632c695c6a0db8315b757e1c1";
    const TEST_FEE: u64 = 1000;

    use super::*;
    use crate::htlc::tx::{DEFAULT_TX_LOCKTIME, DEFAULT_TX_VERSION};
    use crate::test_utils::{
        generate_bitcoin_random_keypair, get_dummy_txin, get_dummy_txout, get_test_bitcoin_indexer,
        get_test_htlc_params, TEST_NETWORK,
    };
    use crate::UtxoStatus;
    use crate::{get_htlc_address, merry::fund_btc, validate_schnorr_signature};
    use alloy::hex;
    use bitcoin::hashes::hex::FromHex;
    use bitcoin::Transaction;
    use bitcoin::{key::Secp256k1, secp256k1::Message, taproot, Network, XOnlyPublicKey};
    use eyre::Result;
    use std::str::FromStr;
    use utils::gen_secret;

    #[tokio::test]
    async fn test_generate_instant_refund_hash() -> Result<()> {
        let secp = Secp256k1::new();

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();

        let indexer = get_test_bitcoin_indexer()?;

        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let (_, secret_hash) = gen_secret();

        let htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, Network::Regtest);

        let htlc_address = get_htlc_address(&htlc_params, Network::Regtest).unwrap();

        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        let result = generate_instant_refund_hash(
            &htlc_params,
            &utxos,
            &recipient,
            Network::Regtest,
            Some(TEST_FEE),
        );

        assert!(result.is_ok());

        // Verification of the generated hash with respective to the signature and the pubkey.
        let result = result.unwrap();
        let message_hash = result.get(0).unwrap().clone();
        let message = Message::from_digest_slice(&message_hash)?;

        let signature = taproot::Signature {
            signature: secp.sign_schnorr_no_aux_rand(&message, &initiator_key_pair),
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
        };

        validate_schnorr_signature(
            &initiator_pubkey,
            &signature.serialize(),
            &message_hash,
            TapSighashType::SinglePlusAnyoneCanPay,
        )?;

        Ok(())
    }

    #[tokio::test]
    async fn test_generate_instant_refund_hash_multiple_utxos() -> Result<()> {
        let secp = Secp256k1::new();

        let indexer = get_test_bitcoin_indexer()?;

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();

        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let (_, secret_hash) = gen_secret();

        let htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, Network::Regtest);

        let htlc_address = get_htlc_address(&htlc_params, Network::Regtest).unwrap();

        fund_btc(&htlc_address, &indexer).await?;

        // Fund HTLC address twice to test multiple UTXO handling
        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        let result = generate_instant_refund_hash(
            &htlc_params,
            &utxos,
            &recipient,
            Network::Regtest,
            Some(TEST_FEE),
        );

        assert!(result.is_ok());

        let hashes = result.unwrap();

        assert_eq!(
            hashes.len(),
            utxos.len(),
            "Hash count must match UTXO count"
        );

        // Validate each hash against signatures from both parties
        for (i, hash_hex) in hashes.iter().enumerate() {
            let message = Message::from_digest_slice(hash_hex)?;

            let initiator_sig = taproot::Signature {
                signature: secp.sign_schnorr_no_aux_rand(&message, &initiator_key_pair),
                sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
            };

            validate_schnorr_signature(
                &initiator_pubkey,
                &initiator_sig.serialize(),
                hash_hex,
                TapSighashType::SinglePlusAnyoneCanPay,
            )
            .expect(&format!("Invalid initiator signature for input {}", i));
        }

        Ok(())
    }

    // Reference transaction :
    // e6300e14962c4b210c6c5f8767e0ec9fc2421b7eb8cd97ad17d0b12f9634f23a
    #[tokio::test]
    async fn test_generate_instant_refund_hash_correctness() -> Result<()> {
        let instant_refund_sacp_hex = "020000000001013af234962fb1d017ad97cdb87e1b42c29fece067875f6c0c214b2c96140e30e60000000000ffffffff0134c200000000000016001444603810e8ece52d6fb229579f7a6a05d29a4476044165d7f2ff319ec97d4c895e5f5a1cb752aef42ea262829451368966e606b4c1baa77bc4e2ae616d8af9546c4855e20dce2846cc52ca2a1a43e6daed0607b8f07c834165d7f2ff319ec97d4c895e5f5a1cb752aef42ea262829451368966e606b4c1baa77bc4e2ae616d8af9546c4855e20dce2846cc52ca2a1a43e6daed0607b8f07c834620509d189ac7100b8e92193d15a2d8a73dfcb8d4520452a028e55e389b2b742f96ac20460f2e8ff81fc4e0a8e6ce7796704e3829e3e3eedb8db9390bdc51f4f04cf0a6ba529c61c12160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f089e8a72cc9cd400946d37c37d99ece487b2035563ceb0d56b32eb415eded54238b8b93819b7b34120fcb434e192324f3e6161b0723c83fcf0c994dd5890b5a0500000000";

        let instant_refund_sacp_bytes = Vec::from_hex(instant_refund_sacp_hex)?;
        let instant_refund_sacp: Transaction =
            bitcoin::consensus::deserialize(&instant_refund_sacp_bytes)?;

        // These pubkeys correspond to your test vector (extracted from your data)
        let initiator_pubkey = XOnlyPublicKey::from_str(
            "509d189ac7100b8e92193d15a2d8a73dfcb8d4520452a028e55e389b2b742f96",
        )?;
        let redeemer_pubkey = XOnlyPublicKey::from_str(
            "460f2e8ff81fc4e0a8e6ce7796704e3829e3e3eedb8db9390bdc51f4f04cf0a6",
        )?;

        let bytes = hex::decode(TEST_SECRET_HASH).expect("Invalid hex string");

        // Ensure the byte array is exactly 32 bytes
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&bytes[..32]); // Copy the first 32 bytes

        let mut htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash);

        let recipient = Address::from_str("tb1qg3srsy8ganjj6maj99te77n2qhff53rkm3x6z8")?
            .require_network(Network::Testnet4)?;

        htlc_params.amount = instant_refund_sacp.output[0].value.to_sat();

        let utxos = vec![Utxo {
            txid: instant_refund_sacp.input[0].previous_output.txid,
            value: 50000,
            vout: instant_refund_sacp.input[0].previous_output.vout,
            status: UtxoStatus {
                confirmed: true,
                block_height: Some(100),
            },
        }];

        // Generate the sighash(es) for the transaction inputs
        let hash_vec = generate_instant_refund_hash(
            &htlc_params,
            &utxos,
            &recipient,
            Network::Testnet4,
            Some(284), // Fees used by that transaction.
        )?;

        // There should be one input
        let message_hash = &hash_vec[0];

        // Extract the signature from the witness
        // Witness format for Taproot script spend usually: [<signature>, <script>]
        let witness = &instant_refund_sacp.input[0].witness;
        let signature_with_sighash = witness
            .nth(0)
            .ok_or_else(|| eyre::eyre!("Missing signature in witness"))?;

        validate_schnorr_signature(
            &initiator_pubkey,
            signature_with_sighash,
            message_hash,
            TapSighashType::SinglePlusAnyoneCanPay,
        )?;

        Ok(())
    }

    #[tokio::test]
    async fn test_tapscript_spend_sighash_generator() -> Result<()> {
        let mock_tx = Transaction {
            version: DEFAULT_TX_VERSION,
            lock_time: DEFAULT_TX_LOCKTIME,
            input: vec![get_dummy_txin(), get_dummy_txin()],
            output: vec![get_dummy_txout(), get_dummy_txout()],
        };

        let leaf_hash = TapLeafHash::all_zeros();
        let mut generator = TapScriptSpendSigHashGenerator::new(mock_tx, leaf_hash);

        // Test valid case with correct input and output lengths
        let input_index = 0;
        let previous_output = &get_dummy_txout();
        let sighash_type = TapSighashType::SinglePlusAnyoneCanPay;
        let sig_hash = generator.with_prevout(input_index, previous_output, sighash_type)?;

        assert_eq!(
            sig_hash.len(),
            32,
            "Signature hash length should be 32 bytes"
        );

        // Test the case with all previous outputs (valid lengths)
        let sighash_type = TapSighashType::All;
        let previous_outputs = vec![get_dummy_txout(), get_dummy_txout()];
        let sighashes = generator.with_all_prevouts(&previous_outputs, sighash_type)?;

        assert_eq!(
            sighashes.len(),
            previous_outputs.len(),
            "Sighashes should match number of previous outputs"
        );
        assert_eq!(
            sighashes[0].len(),
            32,
            "Each signature hash should be 32 bytes"
        );

        // Test invalid case: mismatched number of inputs and previous outputs
        let mismatched_outputs = vec![get_dummy_txout()];
        let result = generator.with_all_prevouts(&mismatched_outputs, sighash_type);
        assert!(
            result.is_err(),
            "Expected error due to input/output length mismatch"
        );

        // Test invalid case: out of bounds input index
        let invalid_input_index = 5;
        let result = generator.with_prevout(invalid_input_index, previous_output, sighash_type);
        assert!(result.is_err(), "Expected error due to invalid input index");

        // Test invalid case: empty inputs (edge case for empty transactions)
        let empty_tx = Transaction {
            version: DEFAULT_TX_VERSION,
            lock_time: DEFAULT_TX_LOCKTIME,
            input: vec![],
            output: vec![],
        };
        let mut empty_generator = TapScriptSpendSigHashGenerator::new(empty_tx, leaf_hash);

        let result = empty_generator.with_all_prevouts(&mismatched_outputs, sighash_type);
        assert!(
            result.is_err(),
            "Expected error due to empty transaction inputs"
        );

        // Testing with a localnet transaction
        let secp = Secp256k1::new();
        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();
        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;
        let (_, secret_hash) = gen_secret();
        let htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, Network::Regtest);

        let htlc_address = get_htlc_address(&htlc_params, TEST_NETWORK)?;

        let indexer = get_test_bitcoin_indexer()?;
        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        let transaction = build_tx(
            &utxos,
            &recipient,
            &Witness::new(),
            Sequence::MAX,
            TapSighashType::SinglePlusAnyoneCanPay,
            Some(TEST_FEE),
        )?;

        let leaf_hash =
            instant_refund_leaf(&initiator_pubkey, &redeemer_pubkey).tapscript_leaf_hash();

        let previous_output = create_previous_outputs(&utxos, &htlc_address);

        let mut generator = TapScriptSpendSigHashGenerator::new(transaction, leaf_hash);

        let sig_hash = generator.with_prevout(
            0,
            &previous_output[0],
            TapSighashType::SinglePlusAnyoneCanPay,
        )?;

        assert_eq!(sig_hash.len(), 32);

        // Validate each hash against signatures from both parties
        let message = Message::from_digest_slice(&sig_hash)?;

        let initiator_sig = taproot::Signature {
            signature: secp.sign_schnorr_no_aux_rand(&message, &initiator_key_pair),
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
        };

        validate_schnorr_signature(
            &initiator_pubkey,
            &initiator_sig.serialize(),
            &sig_hash,
            TapSighashType::SinglePlusAnyoneCanPay,
        )
        .expect("Invalid initiator signature");

        Ok(())
    }
}
