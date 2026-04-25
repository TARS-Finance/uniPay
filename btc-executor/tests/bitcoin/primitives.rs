use bitcoin::Network;

use btc_executor::infrastructure::chain::bitcoin::primitives::{
    HTLCLeaf, HTLCParams, get_control_block, get_htlc_address, get_htlc_leaf_script,
    get_instant_refund_witness, get_redeem_witness, get_refund_witness,
};
use super::common::unit_test_htlc_params;
use sha2::{Digest, Sha256};

#[test]
fn htlc_address_is_deterministic_and_p2tr() {
    let params = unit_test_htlc_params(b"test_secret_32_bytes_exactly!!!!!", 100_000, 10);
    let addr1 = get_htlc_address(&params, Network::Regtest).expect("addr 1");
    let addr2 = get_htlc_address(&params, Network::Regtest).expect("addr 2");
    assert_eq!(
        addr1.to_string(),
        addr2.to_string(),
        "same params -> same address"
    );
    assert!(
        addr1.to_string().starts_with("bcrt1p"),
        "regtest P2TR prefix"
    );
}

#[test]
fn htlc_address_changes_with_different_params() {
    let params_a = HTLCParams {
        initiator_pubkey: unit_test_htlc_params(b"seed-a", 1, 1).initiator_pubkey,
        redeemer_pubkey: unit_test_htlc_params(b"seed-b", 1, 1).redeemer_pubkey,
        amount: 100_000,
        secret_hash: Sha256::digest(b"secret_a").into(),
        timelock: 10,
    };
    let params_b = HTLCParams {
        initiator_pubkey: params_a.initiator_pubkey,
        redeemer_pubkey: params_a.redeemer_pubkey,
        amount: 100_000,
        secret_hash: Sha256::digest(b"secret_b").into(),
        timelock: 10,
    };
    let addr_a = get_htlc_address(&params_a, Network::Regtest).expect("addr a");
    let addr_b = get_htlc_address(&params_b, Network::Regtest).expect("addr b");
    assert_ne!(addr_a.to_string(), addr_b.to_string());
}

#[test]
fn all_leaf_scripts_and_control_blocks_are_valid() {
    let params = unit_test_htlc_params(b"test_secret", 50_000, 5);
    for leaf in [HTLCLeaf::Redeem, HTLCLeaf::Refund, HTLCLeaf::InstantRefund] {
        let script = get_htlc_leaf_script(&params, leaf);
        assert!(!script.is_empty(), "{leaf:?} script must not be empty");
        let cb = get_control_block(&params, leaf)
            .unwrap_or_else(|e| panic!("control block {leaf:?}: {e}"));
        assert!(!cb.serialize().is_empty());
    }
}

#[test]
fn witness_builders_produce_correct_structure() {
    let secret = b"test_secret_32_bytes_exactly!!!!!";
    let params = unit_test_htlc_params(secret, 50_000, 10);

    let redeem_w = get_redeem_witness(&params, secret).expect("redeem witness");
    assert_eq!(redeem_w.len(), 4);
    assert_eq!(redeem_w.nth(0).expect("sig").len(), 65);
    assert!(redeem_w.nth(0).expect("sig").iter().all(|&b| b == 0));
    assert_eq!(redeem_w.nth(1).expect("secret"), secret);

    let refund_w = get_refund_witness(&params).expect("refund witness");
    assert_eq!(refund_w.len(), 3);

    let ir_w = get_instant_refund_witness(&params, &[0xab; 64], &[0xab; 64])
        .expect("instant refund witness");
    assert_eq!(ir_w.len(), 4);
}
