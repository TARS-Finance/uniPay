use std::sync::Arc;

use btc_executor::infrastructure::chain::bitcoin::primitives::{HTLCParams, get_htlc_address};
use btc_executor::infrastructure::chain::bitcoin::tx_builder::primitives::BitcoinTxAdaptorParams;
use btc_executor::infrastructure::chain::bitcoin::wallet::SendRequest;
use bitcoin::{Sequence, TapSighashType};
use serial_test::serial;

use super::common::{BitcoinTestEnv, DEFAULT_FEE_RATE, NETWORK};

#[serial]
#[tokio::test]
async fn tx_adaptor_and_fee_builder_fund_and_refund_htlc_on_regtest() {
    let env = BitcoinTestEnv::new().await;
    let params = env.htlc_params(b"refund-path-secret", 50_000, 2);
    let htlc_addr = get_htlc_address(&params, NETWORK).expect("htlc address");

    let (_, htlc_utxos) = env
        .fund_htlc(Arc::clone(&env.wallet), &params, DEFAULT_FEE_RATE)
        .await;
    env.mine_blocks(params.timelock).await;

    let refund_tx = env
        .build_with_fee_builder(
            Arc::clone(&env.wallet),
            BitcoinTxAdaptorParams {
                sacps: vec![],
                spends: vec![env.refund_spend(&params, htlc_utxos)],
                sends: vec![],
                fee_rate: DEFAULT_FEE_RATE,
            },
        )
        .await;
    assert_eq!(
        refund_tx.input.len(),
        1,
        "refund tx must spend the HTLC UTXO"
    );
    assert_eq!(
        refund_tx.input[0].sequence.to_consensus_u32(),
        params.timelock as u32
    );

    let refund_txid = env.broadcast_and_confirm("refund", &refund_tx).await;
    env.wait_for_confirmed_tx(&refund_txid).await;
    env.assert_htlc_consumed(&htlc_addr).await;
}

#[serial]
#[tokio::test]
async fn tx_adaptor_and_fee_builder_fund_and_redeem_htlc_on_regtest() {
    let secret = b"redeem-path-secret";
    let env = BitcoinTestEnv::new().await;
    let params = env.htlc_params(secret, 50_000, 6);
    let htlc_addr = get_htlc_address(&params, NETWORK).expect("htlc address");

    let (_, htlc_utxos) = env
        .fund_htlc(Arc::clone(&env.wallet), &params, DEFAULT_FEE_RATE)
        .await;

    let redeem_tx = env
        .build_with_fee_builder(
            Arc::clone(&env.counterparty),
            BitcoinTxAdaptorParams {
                sacps: vec![],
                spends: vec![env.redeem_spend(&params, secret, htlc_utxos)],
                sends: vec![],
                fee_rate: DEFAULT_FEE_RATE,
            },
        )
        .await;
    assert!(redeem_tx
        .output
        .iter()
        .any(|o| o.script_pubkey == env.counterparty.address().script_pubkey()));

    let redeem_txid = env.broadcast_and_confirm("redeem", &redeem_tx).await;
    env.wait_for_confirmed_tx(&redeem_txid).await;
    env.assert_htlc_consumed(&htlc_addr).await;

    let on_chain = env.tx_from_chain(&redeem_txid).await;
    assert_eq!(on_chain.input[0].witness.nth(1).expect("secret"), secret);
}

#[serial]
#[tokio::test]
async fn tx_adaptor_and_fee_builder_fund_and_instant_refund_htlc_on_regtest() {
    let env = BitcoinTestEnv::new().await;
    let params = env.htlc_params(b"instant-refund-secret", 50_000, 6);
    let htlc_addr = get_htlc_address(&params, NETWORK).expect("htlc address");

    let (_, htlc_utxos) = env
        .fund_htlc(Arc::clone(&env.wallet), &params, DEFAULT_FEE_RATE)
        .await;

    let mut tx = env
        .build_with_fee_builder(
            Arc::clone(&env.wallet),
            BitcoinTxAdaptorParams {
                sacps: vec![env.instant_refund_spend(
                    &params,
                    htlc_utxos.clone(),
                    env.wallet.address().clone(),
                )],
                spends: vec![],
                sends: vec![],
                fee_rate: DEFAULT_FEE_RATE,
            },
        )
        .await;
    assert_eq!(tx.input[0].sequence, Sequence::MAX);
    assert_eq!(
        tx.output[0].script_pubkey,
        env.wallet.address().script_pubkey()
    );
    assert_eq!(tx.output[0].value.to_sat(), params.amount);

    env.finalize_instant_refund_witness(&params, &mut tx, 0, htlc_utxos.first().expect("utxo"));
    assert_eq!(tx.input[0].witness.len(), 4);

    let txid = env.broadcast_and_confirm("instant refund", &tx).await;
    env.wait_for_confirmed_tx(&txid).await;
    env.assert_htlc_consumed(&htlc_addr).await;

    let on_chain = env.tx_from_chain(&txid).await;
    let sacp = TapSighashType::SinglePlusAnyoneCanPay as u8;
    assert_eq!(
        on_chain.input[0]
            .witness
            .nth(0)
            .expect("redeemer sig")
            .last()
            .copied(),
        Some(sacp)
    );
    assert_eq!(
        on_chain.input[0]
            .witness
            .nth(1)
            .expect("initiator sig")
            .last()
            .copied(),
        Some(sacp)
    );
}

#[serial]
#[tokio::test]
async fn tx_adaptor_and_fee_builder_builds_mixed_batch_with_market_fee_rate_on_regtest() {
    let env = BitcoinTestEnv::new().await;
    let batch_wallet = env.funded_random_wallet().await;
    let fee = env.market_fee_rate().await;

    let sacp_a = env.htlc_params_for_wallets(
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"mixed-sacp-a",
        30_000,
        6,
    );
    let sacp_b = env.htlc_params_for_wallets(
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"mixed-sacp-b",
        32_000,
        6,
    );
    let redeem_secret_a = b"mixed-redeem-secret-a";
    let redeem_secret_b = b"mixed-redeem-secret-b";
    let redeem_a = env.htlc_params_for_wallets(
        env.counterparty.as_ref(),
        batch_wallet.as_ref(),
        redeem_secret_a,
        28_000,
        6,
    );
    let redeem_b = env.htlc_params_for_wallets(
        env.counterparty.as_ref(),
        batch_wallet.as_ref(),
        redeem_secret_b,
        29_000,
        6,
    );
    let refund_a = env.htlc_params_for_wallets(
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"mixed-refund-a",
        26_000,
        1,
    );
    let refund_b = env.htlc_params_for_wallets(
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"mixed-refund-b",
        27_000,
        1,
    );
    let send_a = env.htlc_params_for_wallets(
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"mixed-send-a",
        10_000,
        9,
    );
    let send_b = env.htlc_params_for_wallets(
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"mixed-send-b",
        11_000,
        10,
    );

    let addrs = |params: &HTLCParams| get_htlc_address(params, NETWORK).expect("addr");
    let sacp_a_addr = addrs(&sacp_a);
    let sacp_b_addr = addrs(&sacp_b);
    let redeem_a_addr = addrs(&redeem_a);
    let redeem_b_addr = addrs(&redeem_b);
    let refund_a_addr = addrs(&refund_a);
    let refund_b_addr = addrs(&refund_b);
    let send_a_addr = addrs(&send_a);
    let send_b_addr = addrs(&send_b);

    let funding_sends = vec![
        SendRequest {
            address: sacp_a_addr.clone(),
            amount: sacp_a.amount,
        },
        SendRequest {
            address: sacp_b_addr.clone(),
            amount: sacp_b.amount,
        },
        SendRequest {
            address: redeem_a_addr.clone(),
            amount: redeem_a.amount,
        },
        SendRequest {
            address: redeem_b_addr.clone(),
            amount: redeem_b.amount,
        },
        SendRequest {
            address: refund_a_addr.clone(),
            amount: refund_a.amount,
        },
        SendRequest {
            address: refund_b_addr.clone(),
            amount: refund_b.amount,
        },
    ];
    let funding_tx = env
        .build_with_fee_builder(
            Arc::clone(&batch_wallet),
            BitcoinTxAdaptorParams {
                sacps: vec![],
                spends: vec![],
                sends: funding_sends,
                fee_rate: fee,
            },
        )
        .await;
    env.broadcast_and_confirm("mixed funding", &funding_tx)
        .await;

    let sacp_a_utxos = env
        .wait_for_confirmed_cover_utxo_count(&sacp_a_addr, 1)
        .await;
    let sacp_b_utxos = env
        .wait_for_confirmed_cover_utxo_count(&sacp_b_addr, 1)
        .await;
    let redeem_a_utxos = env
        .wait_for_confirmed_cover_utxo_count(&redeem_a_addr, 1)
        .await;
    let redeem_b_utxos = env
        .wait_for_confirmed_cover_utxo_count(&redeem_b_addr, 1)
        .await;
    let refund_a_utxos = env
        .wait_for_confirmed_cover_utxo_count(&refund_a_addr, 1)
        .await;
    let refund_b_utxos = env
        .wait_for_confirmed_cover_utxo_count(&refund_b_addr, 1)
        .await;
    env.mine_blocks(refund_b.timelock).await;

    let mut mixed_tx = env
        .build_with_fee_builder(
            Arc::clone(&batch_wallet),
            BitcoinTxAdaptorParams {
                sacps: vec![
                    env.instant_refund_spend(
                        &sacp_a,
                        sacp_a_utxos.clone(),
                        env.wallet.address().clone(),
                    ),
                    env.instant_refund_spend(
                        &sacp_b,
                        sacp_b_utxos.clone(),
                        env.counterparty.address().clone(),
                    ),
                ],
                spends: vec![
                    env.redeem_spend(&redeem_a, redeem_secret_a, redeem_a_utxos.clone()),
                    env.redeem_spend(&redeem_b, redeem_secret_b, redeem_b_utxos.clone()),
                    env.refund_spend(&refund_a, refund_a_utxos.clone()),
                    env.refund_spend(&refund_b, refund_b_utxos.clone()),
                ],
                sends: vec![
                    SendRequest {
                        address: env.counterparty.address().clone(),
                        amount: redeem_a.amount + redeem_b.amount,
                    },
                    SendRequest {
                        address: env.wallet.address().clone(),
                        amount: refund_a.amount + refund_b.amount,
                    },
                    SendRequest {
                        address: send_a_addr.clone(),
                        amount: send_a.amount,
                    },
                    SendRequest {
                        address: send_b_addr.clone(),
                        amount: send_b.amount,
                    },
                ],
                fee_rate: fee,
            },
        )
        .await;

    let input_index_for = |outpoint: bitcoin::OutPoint| {
        mixed_tx
            .input
            .iter()
            .position(|input| input.previous_output == outpoint)
            .expect("input outpoint present")
    };
    let sacp_a_input_index = input_index_for(sacp_a_utxos.first().expect("utxo").outpoint);
    let sacp_b_input_index = input_index_for(sacp_b_utxos.first().expect("utxo").outpoint);
    let redeem_a_input_index = input_index_for(redeem_a_utxos.first().expect("utxo").outpoint);
    let redeem_b_input_index = input_index_for(redeem_b_utxos.first().expect("utxo").outpoint);
    let refund_a_input_index = input_index_for(refund_a_utxos.first().expect("utxo").outpoint);
    let refund_b_input_index = input_index_for(refund_b_utxos.first().expect("utxo").outpoint);

    env.finalize_instant_refund_witness_with_wallets(
        &sacp_a,
        &mut mixed_tx,
        sacp_a_input_index,
        sacp_a_utxos.first().expect("utxo"),
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
    );
    env.finalize_instant_refund_witness_with_wallets(
        &sacp_b,
        &mut mixed_tx,
        sacp_b_input_index,
        sacp_b_utxos.first().expect("utxo"),
        batch_wallet.as_ref(),
        env.counterparty.as_ref(),
    );

    assert_eq!(mixed_tx.input[sacp_a_input_index].sequence, Sequence::MAX);
    assert_eq!(mixed_tx.input[sacp_b_input_index].sequence, Sequence::MAX);
    assert_eq!(
        mixed_tx.input[redeem_a_input_index].sequence,
        Sequence::ENABLE_RBF_NO_LOCKTIME
    );
    assert_eq!(
        mixed_tx.input[redeem_b_input_index].sequence,
        Sequence::ENABLE_RBF_NO_LOCKTIME
    );
    assert_eq!(
        mixed_tx.input[refund_a_input_index]
            .sequence
            .to_consensus_u32(),
        refund_a.timelock as u32
    );
    assert_eq!(
        mixed_tx.input[refund_b_input_index]
            .sequence
            .to_consensus_u32(),
        refund_b.timelock as u32
    );

    let output_index_for = |script_pubkey: &bitcoin::ScriptBuf, value: u64| {
        mixed_tx
            .output
            .iter()
            .position(|output| {
                output.script_pubkey == *script_pubkey && output.value.to_sat() == value
            })
            .expect("output present")
    };

    let sacp_a_output_index =
        output_index_for(&env.wallet.address().script_pubkey(), sacp_a.amount);
    let sacp_b_output_index =
        output_index_for(&env.counterparty.address().script_pubkey(), sacp_b.amount);
    let redeem_output_index = output_index_for(
        &env.counterparty.address().script_pubkey(),
        redeem_a.amount + redeem_b.amount,
    );
    let refund_output_index = output_index_for(
        &env.wallet.address().script_pubkey(),
        refund_a.amount + refund_b.amount,
    );
    let send_a_output_index = output_index_for(&send_a_addr.script_pubkey(), send_a.amount);
    let send_b_output_index = output_index_for(&send_b_addr.script_pubkey(), send_b.amount);

    assert_eq!(
        mixed_tx.output[sacp_a_output_index].value.to_sat(),
        sacp_a.amount
    );
    assert_eq!(
        mixed_tx.output[sacp_b_output_index].value.to_sat(),
        sacp_b.amount
    );
    assert_eq!(
        mixed_tx.output[redeem_output_index].value.to_sat(),
        redeem_a.amount + redeem_b.amount
    );
    assert_eq!(
        mixed_tx.output[refund_output_index].value.to_sat(),
        refund_a.amount + refund_b.amount
    );
    assert_eq!(
        mixed_tx.output[send_a_output_index].value.to_sat(),
        send_a.amount
    );
    assert_eq!(
        mixed_tx.output[send_b_output_index].value.to_sat(),
        send_b.amount
    );

    let mixed_txid = env.broadcast_and_confirm("mixed batch", &mixed_tx).await;
    env.wait_for_confirmed_tx(&mixed_txid).await;

    for addr in [
        &sacp_a_addr,
        &sacp_b_addr,
        &redeem_a_addr,
        &redeem_b_addr,
        &refund_a_addr,
        &refund_b_addr,
    ] {
        env.assert_htlc_consumed(addr).await;
    }
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&send_a_addr, 1)
            .await[0]
            .value,
        send_a.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&send_b_addr, 1)
            .await[0]
            .value,
        send_b.amount
    );

    let on_chain = env.tx_from_chain(&mixed_txid).await;
    let sacp = TapSighashType::SinglePlusAnyoneCanPay as u8;
    for idx in [sacp_a_input_index, sacp_b_input_index] {
        assert_eq!(on_chain.input[idx].witness.len(), 4);
        assert_eq!(
            on_chain.input[idx]
                .witness
                .nth(0)
                .expect("r")
                .last()
                .copied(),
            Some(sacp)
        );
        assert_eq!(
            on_chain.input[idx]
                .witness
                .nth(1)
                .expect("i")
                .last()
                .copied(),
            Some(sacp)
        );
    }
    assert_eq!(
        on_chain.input[2].witness.nth(1).expect("secret a"),
        redeem_secret_a
    );
    assert_eq!(
        on_chain.input[3].witness.nth(1).expect("secret b"),
        redeem_secret_b
    );
    assert_eq!(on_chain.input[4].witness.len(), 3);
    assert_eq!(on_chain.input[5].witness.len(), 3);
}
