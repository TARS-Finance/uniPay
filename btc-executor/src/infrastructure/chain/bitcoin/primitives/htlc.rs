//! Core HTLC types and Taproot address generation.
//!
//! Vendored from garden-rs `crates/bitcoin/src/htlc/htlc.rs` and
//! `crates/bitcoin/src/htlc/primitives.rs`, adapted to remove garden-specific
//! dependencies (`eyre`, `once_cell`, orderbook, indexer, batcher, etc.).

use super::{
    error::BitcoinPrimitivesError,
    script::{instant_refund_leaf, redeem_leaf, refund_leaf},
};
use bitcoin::{
    key::{Secp256k1, XOnlyPublicKey},
    secp256k1::{PublicKey, SecretKey},
    taproot::{ControlBlock, LeafVersion, TapLeafHash, TaprootBuilder, TaprootSpendInfo},
    Address, KnownHrp, Network, ScriptBuf,
};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

/// Represents the different spending paths in the HTLC Taproot tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HTLCLeaf {
    /// Requires secret preimage and redeemer signature
    Redeem,
    /// Allows initiator to claim funds after timelock expires
    Refund,
    /// Requires both parties' signatures
    InstantRefund,
}

/// A structure containing all necessary information for a Bitcoin HTLC swap.
///
/// This includes public keys, amount, and other parameters required to create
/// and manage the HTLC.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HTLCParams {
    /// Public key of the party initiating the swap
    pub initiator_pubkey: XOnlyPublicKey,
    /// Public key of the party redeeming the swap
    pub redeemer_pubkey: XOnlyPublicKey,
    /// Amount in satoshis
    pub amount: u64,
    /// SHA256 hash of the secret (32 bytes)
    pub secret_hash: [u8; 32],
    /// Number of blocks before refund path becomes valid
    pub timelock: u64,
}

/// Garden deterministic unspendable internal key for Taproot HTLC addresses.
///
/// Generates a deterministic internal key using:
/// 1. SHA256("GardenHTLC") as scalar r
/// 2. BIP-341 H point
/// 3. r*G + H for final public key
///
/// This follows BIP-341's key generation scheme to create
/// a provably uncontrollable internal key for better security.
pub static GARDEN_NUMS: LazyLock<XOnlyPublicKey> = LazyLock::new(|| {
    // Step 1: Hash "GardenHTLC" -> r
    let r = Sha256::digest(b"GardenHTLC");

    // Step 2: Parse the H point from BIP-341
    const H_HEX: &str = "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";
    let h_bytes = hex::decode(H_HEX).expect("Invalid hex in GARDEN_NUMS");
    let h = PublicKey::from_slice(&h_bytes).expect("Invalid H point in GARDEN_NUMS");

    // Step 3: r * G
    let secp = Secp256k1::new();
    let r_scalar = SecretKey::from_slice(&r).expect("Invalid scalar in GARDEN_NUMS");
    let r_g = PublicKey::from_secret_key(&secp, &r_scalar);

    // Step 4: H + r*G
    let nums = h
        .combine(&r_g)
        .expect("Point addition failed in GARDEN_NUMS");

    // Step 5: Convert to x-only
    let (xonly, _) = nums.x_only_public_key();
    xonly
});

/// Weight assigned to redeem leaf in the Taproot tree (most likely spending path).
const REDEEM_LEAF_WEIGHT: u8 = 1;

/// Weight assigned to refund and instant refund leaves in the Taproot tree.
const OTHER_LEAF_WEIGHT: u8 = 2;

/// Constructs a Taproot tree with three spending conditions in a Huffman tree structure.
///
/// The tree optimizes for the most common spending path (redeem) by assigning it
/// a lower weight (shorter proof).
///
/// # Errors
/// Returns `BitcoinPrimitivesError::Taproot` if building or finalizing the tree fails.
pub fn construct_taproot_spend_info(
    htlc_params: &HTLCParams,
) -> Result<TaprootSpendInfo, BitcoinPrimitivesError> {
    // Create the script leaves
    let redeem = redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey);
    let refund = refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey);
    let instant_refund =
        instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey);

    let secp = Secp256k1::new();
    let mut taproot_builder = TaprootBuilder::new();

    // Add leaves to the Taproot tree with weights (1 for redeem, 2 for others)
    // This creates a Huffman-tree-like structure optimizing for the most common spending path
    taproot_builder = taproot_builder
        .add_leaf(REDEEM_LEAF_WEIGHT, redeem)
        .map_err(|e| {
            BitcoinPrimitivesError::Taproot(format!(
                "Unable to add redeem leaf to Taproot tree: {e}"
            ))
        })?
        .add_leaf(OTHER_LEAF_WEIGHT, refund)
        .map_err(|e| {
            BitcoinPrimitivesError::Taproot(format!(
                "Unable to add refund leaf to Taproot tree: {e}"
            ))
        })?
        .add_leaf(OTHER_LEAF_WEIGHT, instant_refund)
        .map_err(|e| {
            BitcoinPrimitivesError::Taproot(format!(
                "Unable to add instant refund leaf to Taproot tree: {e}"
            ))
        })?;

    if !taproot_builder.is_finalizable() {
        return Err(BitcoinPrimitivesError::Taproot(
            "Taproot builder is not in a finalizable state".to_string(),
        ));
    }

    let internal_key = *GARDEN_NUMS;

    taproot_builder.finalize(&secp, internal_key).map_err(|_| {
        BitcoinPrimitivesError::Taproot("Failed to finalize Taproot spend info".to_string())
    })
}

/// Generates a Taproot HTLC address with three spending conditions:
/// 1. Redeem path: Requires the secret and redeemer's signature
/// 2. Refund path: Allows initiator to claim funds after timelock expires
/// 3. Instant refund: Enables cooperative cancellation by both parties
///
/// # Errors
/// Returns `BitcoinPrimitivesError::Taproot` if taproot construction fails.
pub fn get_htlc_address(
    htlc_params: &HTLCParams,
    network: Network,
) -> Result<Address, BitcoinPrimitivesError> {
    let secp = Secp256k1::new();
    let internal_key = *GARDEN_NUMS;
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
pub fn get_htlc_leaf_script(htlc_params: &HTLCParams, leaf: HTLCLeaf) -> ScriptBuf {
    match leaf {
        HTLCLeaf::Redeem => redeem_leaf(&htlc_params.secret_hash, &htlc_params.redeemer_pubkey),
        HTLCLeaf::Refund => refund_leaf(htlc_params.timelock, &htlc_params.initiator_pubkey),
        HTLCLeaf::InstantRefund => {
            instant_refund_leaf(&htlc_params.initiator_pubkey, &htlc_params.redeemer_pubkey)
        },
    }
}

/// Computes the tapleaf hash for a specific HTLC spending path.
pub fn get_htlc_leaf_hash(htlc_params: &HTLCParams, leaf: HTLCLeaf) -> TapLeafHash {
    let script = get_htlc_leaf_script(htlc_params, leaf);
    TapLeafHash::from_script(&script, LeafVersion::TapScript)
}

/// Gets the control block for a specific spending path in the Taproot tree.
///
/// The control block is needed to spend funds using one of the three available
/// spending conditions: redeem, refund, or instant refund.
///
/// # Errors
/// Returns `BitcoinPrimitivesError::Taproot` if construction or lookup fails.
pub fn get_control_block(
    htlc_params: &HTLCParams,
    leaf: HTLCLeaf,
) -> Result<ControlBlock, BitcoinPrimitivesError> {
    let spend_info = construct_taproot_spend_info(htlc_params)?;
    let script = get_htlc_leaf_script(htlc_params, leaf);

    spend_info
        .control_block(&(script, LeafVersion::TapScript))
        .ok_or_else(|| {
            BitcoinPrimitivesError::Taproot(format!("Failed to get control block for '{:?}'", leaf))
        })
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
    const TEST_AMOUNT: u64 = 50_000;

    fn make_test_params() -> HTLCParams {
        let initiator_pubkey = XOnlyPublicKey::from_str(TEST_INITIATOR_PUBKEY).unwrap();
        let redeemer_pubkey = XOnlyPublicKey::from_str(TEST_REDEEMER_PUBKEY).unwrap();
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&hex::decode(TEST_SECRET_HASH).unwrap());

        HTLCParams {
            initiator_pubkey,
            redeemer_pubkey,
            amount: TEST_AMOUNT,
            secret_hash,
            timelock: TEST_TIMELOCK,
        }
    }

    #[test]
    fn get_htlc_address_is_deterministic() {
        let params = make_test_params();
        let addr1 = get_htlc_address(&params, Network::Regtest).unwrap();
        let addr2 = get_htlc_address(&params, Network::Regtest).unwrap();
        assert_eq!(addr1, addr2, "Same params must produce the same address");
    }

    #[test]
    fn get_htlc_address_regtest_prefix() {
        let params = make_test_params();
        let addr = get_htlc_address(&params, Network::Regtest).unwrap();
        let addr_str = addr.to_string();
        assert!(
            addr_str.starts_with("bcrt1p"),
            "Regtest taproot address must start with 'bcrt1p', got: {addr_str}"
        );
    }

    #[test]
    fn construct_taproot_spend_info_has_merkle_root() {
        let params = make_test_params();
        let spend_info = construct_taproot_spend_info(&params).unwrap();
        assert!(
            spend_info.merkle_root().is_some(),
            "Taproot spend info must have a merkle root"
        );
    }

    #[test]
    fn control_blocks_are_nonempty() {
        let params = make_test_params();
        for leaf in [HTLCLeaf::Redeem, HTLCLeaf::Refund, HTLCLeaf::InstantRefund] {
            let cb = get_control_block(&params, leaf).unwrap();
            assert!(
                !cb.serialize().is_empty(),
                "Control block for {leaf:?} must not be empty"
            );
        }
    }

    #[test]
    fn control_block_values_match_garden_rs() {
        let params = make_test_params();

        let redeem_cb = get_control_block(&params, HTLCLeaf::Redeem).unwrap();
        assert_eq!(
            hex::encode(redeem_cb.serialize()),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f09ccdba96cfd33ad72291a7087b12f4b0c4ab4b571cd91d31de6169c33e166621"
        );

        let refund_cb = get_control_block(&params, HTLCLeaf::Refund).unwrap();
        assert_eq!(
            hex::encode(refund_cb.serialize()),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f09baa85dee2660f052938ad9556d87dd31dc9e809919ed0d0d2b3b1a75dcf8aa5f7ad405ea98aad269ae6466ebcd47587ac0e8f61bc8909470bb5171c63c4e6e7"
        );

        let instant_cb = get_control_block(&params, HTLCLeaf::InstantRefund).unwrap();
        assert_eq!(
            hex::encode(instant_cb.serialize()),
            "c02160e11a135f94e536a5b222e5d09fd9db1be5f5f5e753920290c0410cf388f072f6eb147eed8bbf8b166d31b631110f6de09c6e69ae766e92dd42d6549174b0f7ad405ea98aad269ae6466ebcd47587ac0e8f61bc8909470bb5171c63c4e6e7"
        );
    }

    #[test]
    fn garden_nums_key_is_valid() {
        // Simply accessing the static forces computation; it must not panic.
        let _key = *GARDEN_NUMS;
    }
}
