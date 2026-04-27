use super::{
    primitives::{HTLCLeaf, HTLCParams, InstantRefundSignatures},
    script::{instant_refund_leaf, redeem_leaf, refund_leaf},
    tx::{build_tx, sort_utxos},
};
use crate::{
    batcher::{
        fee::{adjust_outputs_for_fee, estimate_fee},
        sign::add_signature_request,
    },
    htlc::{
        hash::TapScriptSpendSigHashGenerator,
        tx::{
            create_inputs_from_utxos, create_outputs, create_previous_outputs, get_output_values,
            DEFAULT_TX_LOCKTIME, DEFAULT_TX_VERSION,
        },
        witness::get_refund_witness,
    },
    indexer::primitives::Utxo,
    ArcFeeRateEstimator, ArcIndexer, FeeLevel, FeeRate,
};
use alloy::hex;
use bitcoin::{
    key::{Keypair, Secp256k1, XOnlyPublicKey},
    secp256k1::{PublicKey, SecretKey},
    taproot::{ControlBlock, LeafVersion, TaprootBuilder, TaprootSpendInfo},
    Address, KnownHrp, Network, ScriptBuf, Sequence, TapSighashType, Transaction, Witness,
};
use eyre::{bail, eyre, Result};
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};

/// Unipay internal key for Taproot HTLC addresses
///
/// Generates a deterministic internal key using:
/// 1. SHA256("GardenHTLC") as scalar r (protocol tag — do not change, breaks Bitcoin HTLC compatibility)
/// 2. BIP-341 H point
/// 3. r*G + H for final public key
///
/// This follows BIP-341's key generation scheme to create
/// a provably uncontrollable internal key for better security.
pub static UNIPAY_NUMS: Lazy<XOnlyPublicKey> = Lazy::new(|| {
    // Step 1: Hash "GardenHTLC" → r (protocol tag kept as-is for Bitcoin script compatibility)
    let r = Sha256::digest(b"GardenHTLC");

    // Step 2: Parse the H point from BIP-341
    const H_HEX: &str = "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";
    let h_bytes = hex::decode(H_HEX).expect("Invalid hex in UNIPAY_NUMS_KEY");
    let h = PublicKey::from_slice(&h_bytes).expect("Invalid H point in UNIPAY_NUMS_KEY");

    // Step 3: r * G
    let secp = Secp256k1::new();
    let r_scalar = SecretKey::from_slice(&r).expect("Invalid scalar in UNIPAY_NUMS_KEY");
    let r_g = PublicKey::from_secret_key(&secp, &r_scalar);

    // Step 4: H + r*G
    let nums = h
        .combine(&r_g)
        .expect("Point addition failed in UNIPAY_NUMS_KEY");

    // Step 5: Convert to x-only
    let (xonly, _) = nums.x_only_public_key();
    xonly
});

/// Weight assigned to redeem leaf in the Taproot tree
const REDEEM_LEAF_WEIGHT: u8 = 1;

/// Weight assigned to refund and instant refund leaves in the Taproot tree
const OTHER_LEAF_WEIGHT: u8 = 2;

/// Bitcoin HTLC implementation that handles transaction creation and signing
///
/// Provides functionality to create, redeem and refund Hash Time Locked Contract (HTLC)
/// transactions on the Bitcoin network using Taproot script paths.
pub struct BitcoinHTLC {
    ///  Keypair for signing transactions
    keypair: Keypair,

    /// Client for interacting with Bitcoin network
    indexer: ArcIndexer,

    /// Provider for fee rate estimation
    fee_rate_estimator: ArcFeeRateEstimator,

    /// Fee level to use for transaction creation
    fee_level: FeeLevel,

    /// Bitcoin network (mainnet, testnet, regtest)
    network: Network,
}

impl BitcoinHTLC {
    /// Creates a new BitcoinHTLC instance
    ///
    /// # Arguments
    /// * `keypair` - Private key for signing transactions
    /// * `indexer` - Client for Bitcoin network interaction
    /// * `fee_rate_estimator` -  Provider for fee rate estimation
    /// * `network` - Bitcoin network to operate on
    pub fn new(
        keypair: Keypair,
        indexer: ArcIndexer,
        fee_rate_estimator: ArcFeeRateEstimator,
        fee_level: FeeLevel,
        network: Network,
    ) -> Self {
        BitcoinHTLC {
            keypair,
            indexer,
            fee_rate_estimator,
            fee_level,
            network,
        }
    }

    /// Refunds the HTLC by sending the funds to the initiator's address
    ///
    /// # Arguments
    /// * `htlc_params` - HTLC parameters including keys and timelock
    /// * `recipient` - Address to receive the refunded funds
    pub async fn refund(&self, htlc_params: &HTLCParams, recipient: &Address) -> Result<String> {
        // Get the htlc address
        let htlc_address = get_htlc_address(htlc_params, self.network)?;

        // Fetch the utxos from the htlc address
        let utxos = self.indexer.get_utxos(&htlc_address).await?;
        if utxos.is_empty() {
            bail!("No UTXOs available to refund from HTLC address");
        }

        // Get the witness for the refund transaction
        let witness = get_refund_witness(htlc_params)?;

        let sighash_type = TapSighashType::All;

        // Create inputs and outputs of the transaction.
        let inputs = {
            let sequence = Sequence(htlc_params.timelock as u32);
            create_inputs_from_utxos(&utxos, &witness, sequence)
        };

        let output_values = get_output_values(&utxos, sighash_type)?;
        let mut outputs = create_outputs(output_values, recipient, None)?; // Fee will be adjusted later.

        // Get the current network fee rate and tx fee
        let fee_estimate = self.fee_rate_estimator.get_fee_estimates().await?;
        let fee_rate = FeeRate::new(self.fee_level.from(&fee_estimate))?;
        let fee = estimate_fee(&inputs, &outputs, fee_rate);

        // Adjust the outputs for the fee
        adjust_outputs_for_fee(&mut outputs, fee)?;

        let mut refund_tx = Transaction {
            version: DEFAULT_TX_VERSION,
            lock_time: DEFAULT_TX_LOCKTIME,
            input: inputs,
            output: outputs,
        };

        // Generate sighashes for all inputs.
        let sighashes = {
            let refund_leaf_hash = {
                let refund_leaf = refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey);
                refund_leaf.tapscript_leaf_hash()
            };

            let previous_outputs = create_previous_outputs(&utxos, &htlc_address);

            let mut sighash_generator =
                TapScriptSpendSigHashGenerator::new(refund_tx.clone(), refund_leaf_hash);

            sighash_generator.with_all_prevouts(&previous_outputs, sighash_type)?
        };

        if sighashes.len() != refund_tx.input.len() {
            bail!("Number of sighashes does not match number of inputs for refund transaction");
        }

        // Add signatures to the transaction
        let secp = Secp256k1::new();
        for (input, sighash) in refund_tx.input.iter_mut().zip(sighashes) {
            add_signature_request(&secp, input, &self.keypair, &sighash, sighash_type)?;
        }

        // Submit the transaction
        self.indexer.submit_tx(&refund_tx).await?;

        let refund_txid = refund_tx.compute_txid().to_string();

        Ok(refund_txid)
    }
}

/// Builds a Bitcoin transaction for instant refund using SinglePlusAnyoneCanPay signature hash type.
/// This allows both parties to cooperatively cancel the HTLC by providing their signatures.
///
/// # Arguments
/// * `htlc_params` - Swap parameters including keys and amount
/// * `utxos` - Slice of UTXOs to spend from the HTLC address
/// * `signatures` - Vector of signature pairs from both parties for each UTXO
/// * `recipient` - Address to receive funds
/// * `fee` - Optional fee in satoshis to be deducted from the largest output
///
/// # Returns
/// * `Result<Transaction>` - The constructed Bitcoin transaction
///
/// # Errors
/// * When number of signatures doesn't match number of UTXOs
/// * When fee exceeds the swap amount
/// * When signature decoding fails
pub async fn build_instant_refund_sacp(
    htlc_params: &HTLCParams,
    utxos: &[Utxo],
    signatures: Vec<InstantRefundSignatures>,
    recipient: &Address,
    fee: Option<u64>,
) -> Result<Transaction> {
    if utxos.is_empty() {
        bail!("No UTXOs available to spend from HTLC address")
    }

    if signatures.len() != utxos.len() {
        bail!(
            "Signature count mismatch: expected {}, got {}",
            utxos.len(),
            signatures.len()
        )
    }

    let sighash_type = TapSighashType::SinglePlusAnyoneCanPay;

    let utxos = sort_utxos(utxos);

    let mut instant_refund_sacp = build_tx(
        &utxos,
        recipient,
        &Witness::new(),
        Sequence::MAX,
        sighash_type,
        fee,
    )?;

    // Get script leaf and control block for instant refund path
    let instant_refund_leaf =
        instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey);
    let instant_refund_control_block = get_control_block(htlc_params, HTLCLeaf::InstantRefund)?;
    let control_block_serialized = instant_refund_control_block.serialize();

    // Build transaction witnesses
    for (i, sig) in signatures.iter().enumerate() {
        let initiator_signature = hex::decode(&sig.initiator).map_err(|_| {
            eyre!(format!(
                "Failed to decode hex initiator signature at index {}: '{}'",
                i, sig.initiator
            ))
        })?;

        let redeemer_signature = hex::decode(&sig.redeemer).map_err(|_| {
            eyre!(format!(
                "Failed to decode hex redeemer signature at index {}: '{}'",
                i, sig.redeemer
            ))
        })?;

        let mut witness = Witness::new();
        witness.push(redeemer_signature);
        witness.push(initiator_signature);
        witness.push(instant_refund_leaf.clone());
        witness.push(&control_block_serialized);

        instant_refund_sacp.input[i].witness = witness;
    }

    Ok(instant_refund_sacp)
}

/// Generates a Taproot HTLC address with three spending conditions:
/// 1. Redeem path: Requires the secret and redeemer's signature
/// 2. Refund path: Allows initiator to claim funds after timelock expires
/// 3. Instant refund: Enables cooperative cancellation by both parties
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys, timelock, and amount
/// * `network` - Bitcoin network (Mainnet, Testnet, etc    .)
///
/// # Returns
/// * `Result<Address>` - The generated Taproot HTLC address or an error
///
/// # Errors
/// * When failing to generate the internal key
/// * When failing to construct the Taproot spending info
pub fn get_htlc_address(htlc_params: &HTLCParams, network: Network) -> Result<Address> {
    let secp = Secp256k1::new();
    let internal_key = *UNIPAY_NUMS;
    let taproot_spend_info = construct_taproot_spend_info(htlc_params)?;

    let htlc_address = Address::p2tr(
        &secp,
        internal_key,
        taproot_spend_info.merkle_root(),
        KnownHrp::from(network),
    );

    Ok(htlc_address)
}

/// Generates the HTLC script for the specified leaf condition.
///
/// This function creates a script for the HTLC address, based on the specified leaf condition.
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including the secret hash, redeemer's public key, initiator's public key, and timelock.
/// * `leaf` - The specific HTLC leaf condition (Redeem, Refund, or InstantRefund).
///
/// # Returns
/// * `ScriptBuf` - The generated HTLC script for the specified leaf condition.
pub fn get_htlc_leaf_script(htlc_params: &HTLCParams, leaf: HTLCLeaf) -> ScriptBuf {
    let script = match leaf {
        HTLCLeaf::Redeem => redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey),
        HTLCLeaf::Refund => refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey),
        HTLCLeaf::InstantRefund => {
            instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey)
        }
    };

    script
}

/// Gets the control block for a specific spending path in the Taproot tree.
/// This function retrieves the control block needed to spend funds using one of the three
/// available spending conditions: redeem, refund, or instant refund.
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys, timelock, secret hash, and amount
/// * `leaf` - Enum specifying which spending path to get the control block for
///
/// # Returns
/// * `Result<ControlBlock>` - Control block needed for spending with the specified condition
///
/// # Errors
/// * When failing to construct the Taproot spending info
/// * When failing to get the control block for the specified leaf
pub fn get_control_block(htlc_params: &HTLCParams, leaf: HTLCLeaf) -> Result<ControlBlock> {
    let spend_info = construct_taproot_spend_info(htlc_params)?;

    let script = get_htlc_leaf_script(htlc_params, leaf);

    spend_info
        .control_block(&(script, LeafVersion::TapScript))
        .ok_or_else(|| eyre!("Failed to get control block for '{:?}'", leaf))
}

/// Constructs a Taproot tree with three spending conditions in a Huffman tree structure.
/// This function creates the script leaves and combines them into a Taproot spending structure
/// with optimized weights for each condition.
///
/// # Arguments
/// * `htlc_params` - HTLC parameters including keys, timelock, secret hash, and amount
///
/// # Returns
/// * `Result<TaprootSpendInfo>` - Information needed to construct the Taproot output
///
/// # Errors
/// * When failing to add any of the script leaves to the Taproot builder
/// * When the Taproot builder is not finalizable
/// * When failing to generate the internal key
/// * When failing to finalize the Taproot builder
pub fn construct_taproot_spend_info(htlc_params: &HTLCParams) -> Result<TaprootSpendInfo> {
    // Create the script leaves
    let redeem_leaf = redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey);
    let refund_leaf = refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey);
    let instant_refund_leaf =
        instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey);

    let secp = Secp256k1::new();
    let mut taproot_builder = TaprootBuilder::new();

    // Add leaves to the Taproot tree with weights (1 for redeem, 2 for others)
    // This creates a Huffman-tree-like structure optimizing for the most common spending path
    taproot_builder = taproot_builder
        .add_leaf(REDEEM_LEAF_WEIGHT, redeem_leaf)
        .map_err(|e| eyre!("Unable to add redeem leaf to Taproot tree: {e}"))?
        .add_leaf(OTHER_LEAF_WEIGHT, refund_leaf)
        .map_err(|e| eyre!("Unable to add refund leaf to Taproot tree: {e}"))?
        .add_leaf(OTHER_LEAF_WEIGHT, instant_refund_leaf)
        .map_err(|e| eyre!("Unable to add instant refund leaf to Taproot tree: {e}"))?;

    if !taproot_builder.is_finalizable() {
        return Err(eyre!("Taproot builder is not in a finalizable state"));
    }
    let internal_key = *UNIPAY_NUMS;

    taproot_builder
        .finalize(&secp, internal_key)
        .map_err(|_| eyre!("Failed to finalize Taproot spend info"))
}

#[cfg(test)]
mod tests {
    const TEST_SECRET_HASH: &str =
        "c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6";
    const TEST_INITIATOR_PUBKEY: &str =
        "c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66f";
    const TEST_REDEEMER_PUBKEY: &str =
        "1db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fb";
    const TEST_FEE: u64 = 1000;

    use super::*;
    use crate::{
        generate_instant_refund_hash,
        merry::fund_btc,
        test_utils::{
            generate_bitcoin_random_keypair, get_test_bitcoin_htlc, get_test_bitcoin_indexer,
            get_test_htlc_params, TEST_NETWORK,
        },
    };
    use bitcoin::XOnlyPublicKey;
    use eyre::{bail, Context, Result};
    use std::{str::FromStr, time::Duration};
    use tokio::time::sleep;
    use utils::gen_secret;

    // Reference transactions for HTLC script validation:
    // 1. Initiation TX: 1ee94f3c68aa3cfee6911bc2bd28899b2981cf2a877d9883fcd532aa548b43e5
    // 2. Redeem TX: 2c90e80c038f8ef1748d196c96fa5a07849f5ef54da9412c534578b0755db5a3
    #[test]
    fn test_get_control_block() -> Result<()> {
        let initiator_pubkey = XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY)?;
        let redeemer_pubkey = XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY)?;
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&hex::decode(TEST_SECRET_HASH)?);

        let htlc_params = get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash);

        let redeem_control_block = get_control_block(&htlc_params, HTLCLeaf::Redeem)?;
        let refund_control_block = get_control_block(&htlc_params, HTLCLeaf::Refund)?;
        let instant_refund_control_block =
            get_control_block(&htlc_params, HTLCLeaf::InstantRefund)?;

        // Test redeem path control block
        assert!(!redeem_control_block.serialize().is_empty());

        // Test refund path control block
        assert!(!refund_control_block.serialize().is_empty());

        // Test instant refund path control block
        assert!(!instant_refund_control_block.serialize().is_empty());

        // Test control blocks with expected serialized values
        assert_eq!(
            hex::encode(redeem_control_block.serialize()),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f09ccdba96cfd33ad72291a7087b12f4b0c4ab4b571cd91d31de6169c33e166621"
        );
        assert_eq!(
            hex::encode(refund_control_block.serialize()),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f09baa85dee2660f052938ad9556d87dd31dc9e809919ed0d0d2b3b1a75dcf8aa5f7ad405ea98aad269ae6466ebcd47587ac0e8f61bc8909470bb5171c63c4e6e7"
        );
        assert_eq!(
            hex::encode(instant_refund_control_block.serialize()),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f072f6eb147eed8bbf8b166d31b631110f6de09c6e69ae766e92dd42d6549174b0f7ad405ea98aad269ae6466ebcd47587ac0e8f61bc8909470bb5171c63c4e6e7"
        );

        Ok(())
    }

    #[test]
    fn test_construct_taproot_spend_info() -> Result<()> {
        let initiator_pubkey = XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY)?;
        let redeemer_pubkey = XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY)?;
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&hex::decode(TEST_SECRET_HASH)?);

        let htlc_params = get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash);

        let taproot_spend_info = construct_taproot_spend_info(&htlc_params)?;

        let internal_key = *UNIPAY_NUMS;
        let redeem_leaf = redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey);
        let refund_leaf = refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey);
        let instant_refund_leaf =
            instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey);

        // Verify the merkle root is not empty
        assert!(!taproot_spend_info.merkle_root().is_none());

        assert_eq!(taproot_spend_info.internal_key(), internal_key);

        // Verify control blocks match expected values
        assert_eq!(
            hex::encode(
                taproot_spend_info
                    .control_block(&(redeem_leaf, LeafVersion::TapScript))
                    .unwrap()
                    .serialize()
            ),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f09ccdba96cfd33ad72291a7087b12f4b0c4ab4b571cd91d31de6169c33e166621"
        );
        assert_eq!(
            hex::encode(
                taproot_spend_info
                    .control_block(&(refund_leaf, LeafVersion::TapScript))
                    .unwrap()
                    .serialize()
            ),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f09baa85dee2660f052938ad9556d87dd31dc9e809919ed0d0d2b3b1a75dcf8aa5f7ad405ea98aad269ae6466ebcd47587ac0e8f61bc8909470bb5171c63c4e6e7"
        );
        assert_eq!(
            hex::encode(
                taproot_spend_info
                    .control_block(&(instant_refund_leaf, LeafVersion::TapScript))
                    .unwrap()
                    .serialize()
            ),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f072f6eb147eed8bbf8b166d31b631110f6de09c6e69ae766e92dd42d6549174b0f7ad405ea98aad269ae6466ebcd47587ac0e8f61bc8909470bb5171c63c4e6e7"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_build_instant_refund_sacp() -> Result<()> {
        let secp = Secp256k1::new();

        let indexer = get_test_bitcoin_indexer()?;

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();

        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&hex::decode(TEST_SECRET_HASH)?);

        let htlc_params = get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash);

        let htlc_address = get_htlc_address(&htlc_params, Network::Regtest)
            .context("Failed to generate HTLC address")?;

        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        if utxos.is_empty() {
            bail!("No UTXOs found for the HTLC address");
        }

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, Network::Regtest);

        let hashes = generate_instant_refund_hash(
            &htlc_params,
            &utxos,
            &recipient,
            Network::Regtest,
            Some(TEST_FEE),
        )?;

        // Create signatures for each hash
        let signatures: Vec<InstantRefundSignatures> = hashes
            .iter()
            .map(|hash| {
                // Convert to Message type for signing
                let message = bitcoin::secp256k1::Message::from_digest_slice(hash)
                    .context("Failed to create message from hash")
                    .unwrap();

                // Sign with both keys
                let initiator_sig = bitcoin::taproot::Signature {
                    signature: secp.sign_schnorr_no_aux_rand(&message, &initiator_key_pair),
                    sighash_type: bitcoin::TapSighashType::SinglePlusAnyoneCanPay,
                };

                let redeemer_sig = bitcoin::taproot::Signature {
                    signature: secp.sign_schnorr_no_aux_rand(&message, &redeemer_key_pair),
                    sighash_type: bitcoin::TapSighashType::SinglePlusAnyoneCanPay,
                };

                InstantRefundSignatures {
                    initiator: hex::encode(initiator_sig.serialize()),
                    redeemer: hex::encode(redeemer_sig.serialize()),
                }
            })
            .collect();

        let tx =
            build_instant_refund_sacp(&htlc_params, &utxos, signatures, &recipient, Some(TEST_FEE))
                .await
                .context("Failed to build instant refund transaction")?;

        indexer
            .submit_tx(&tx)
            .await
            .context("Failed to submit transaction to network")?;

        println!("Submitted instant refund tx: {}", tx.compute_txid());

        Ok(())
    }

    #[tokio::test]
    async fn test_bitcoin_htlc_refund() -> Result<()> {
        let secp = Secp256k1::new();

        let indexer = get_test_bitcoin_indexer()?;

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();

        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let (_, secret_hash) = gen_secret();

        let mut htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());

        // Setting the timelock to 0, to bypass the timelock.
        htlc_params.timelock = 0;

        let htlc_address = get_htlc_address(&htlc_params, TEST_NETWORK)?;

        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        if utxos.is_empty() {
            bail!("No UTXOs found for the HTLC address");
        }

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, TEST_NETWORK);

        sleep(Duration::from_secs(10)).await;

        let bitcoin_htlc = get_test_bitcoin_htlc(initiator_key_pair).await?;

        let refund_txid = bitcoin_htlc.refund(&htlc_params, &recipient).await?;

        println!("Refund txid: {}", refund_txid);

        Ok(())
    }
}
