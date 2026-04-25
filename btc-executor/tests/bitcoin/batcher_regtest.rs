use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use btc_executor::infrastructure::chain::bitcoin::primitives::get_htlc_address;
use btc_executor::infrastructure::chain::bitcoin::tx_builder::primitives::{
    BitcoinTxAdaptorParams, CoverUtxo,
};
use btc_executor::infrastructure::chain::bitcoin::wallet::{
    BitcoinHtlcWalletAdapter, BroadcastPersistenceKind, BroadcastPersistencePlan,
    EnqueueWalletRequestResult, HtlcAction, HtlcAdapterError, LineageId, WalletRequest,
    WalletStore, wallet_scope,
};
use btc_executor::infrastructure::keys::BitcoinWallet;
use bitcoin::hashes::Hash;
use bitcoin::TapSighashType;
use rand::seq::SliceRandom;
use rand::{RngExt, SeedableRng};
use serial_test::serial;

use super::common::{
    BitcoinTestEnv, TestDatabase, WalletRowUpdate, BATCHER_INTERVAL_SECS, DEFAULT_FEE_RATE,
    NETWORK,
};

fn assert_request_keys(submitted: &super::common::SubmittedBatchTx, expected: &[&str]) {
    let actual = submitted
        .request_keys
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .map(|value| value.to_string())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

fn fake_txid(seed: u8) -> bitcoin::Txid {
    bitcoin::Txid::from_byte_array([seed; 32])
}

async fn submit_single_fresh_and_confirm_without_followup_tick(
    env: &BitcoinTestEnv,
    wallet: Arc<BitcoinWallet>,
    db: Arc<TestDatabase>,
    request: WalletRequest,
    chain_anchor_confirmations: u64,
) -> super::common::SubmittedBatchTx {
    let mut harness = env
        .spawn_batcher_paused_with_db_and_config(wallet, db, 30, chain_anchor_confirmations)
        .await;
    harness.submit(request).await;
    harness.start();
    let submission = harness.wait_for_submitted_tx().await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&submission.txid).await;
    drop(harness);
    submission
}

async fn persist_fake_rbf_head_via_store(
    db: &Arc<TestDatabase>,
    scope: &str,
    lineage_id: LineageId,
    fake_head_txid: bitcoin::Txid,
    existing_request: &WalletRequest,
    attached_requests: &[WalletRequest],
    raw_tx_hex: &str,
) {
    let store = db.store();
    for request in attached_requests {
        let result = store
            .enqueue(scope, request)
            .await
            .expect("enqueue attached request");
        assert!(
            matches!(
                result,
                EnqueueWalletRequestResult::EnqueuedPending
                    | EnqueueWalletRequestResult::AlreadyPending
            ),
            "attached request must be pending before fake head persistence"
        );
    }

    let mut included_request_keys = vec![existing_request.dedupe_key().to_string()];
    included_request_keys.extend(
        attached_requests
            .iter()
            .map(|request| request.dedupe_key().to_string()),
    );

    store
        .persist_broadcast(
            scope,
            &BroadcastPersistencePlan {
                kind: BroadcastPersistenceKind::Rbf,
                lineage_id,
                txid: fake_head_txid,
                raw_tx_hex: raw_tx_hex.to_string(),
                included_request_keys,
                dropped_request_keys: vec![],
            },
        )
        .await
        .expect("persist fake rbf head");
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_initiate_creates_htlc_output_on_regtest() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    let params = env.htlc_params(b"batcher-initiate-secret", 25_000, 6);
    let htlc_addr = get_htlc_address(&params, NETWORK).expect("htlc address");
    let request_key = "batcher-initiate-1";
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    harness
        .submit(env.send_request(request_key, htlc_addr.clone(), params.amount))
        .await;

    let mut harness = harness;
    let submitted = harness.wait_for_submitted_tx().await;
    let txid = submitted.txid;
    let txid_string = txid.to_string();
    assert!(submitted.replaces.is_none());
    assert_request_keys(&submitted, &[request_key]);
    let inflight = db.wallet_row(&scope, request_key).await;
    assert_eq!(inflight.status, "inflight");
    assert_eq!(inflight.batch_txid.as_deref(), Some(txid_string.as_str()));
    assert_eq!(inflight.txid_history, vec![txid_string.clone()]);
    assert!(inflight.chain_anchor.is_none());
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&txid).await;

    // sleep 10 seconds to make sure the wallet updates db as confirmed
    tokio::time::sleep(Duration::from_secs(15)).await;

    let utxos = env.wait_for_confirmed_cover_utxo_count(&htlc_addr, 1).await;
    assert_eq!(utxos[0].value, params.amount);
    let confirmed = db
        .wait_for_wallet_status(&scope, request_key, "confirmed")
        .await;
    assert_eq!(confirmed.status, "confirmed");
    assert_eq!(confirmed.batch_txid.as_deref(), Some(txid_string.as_str()));
    assert_eq!(confirmed.txid_history, vec![txid_string]);
    assert!(confirmed.chain_anchor.is_none());
    tracing::info!(scope = %scope, %txid, "wallet flow initiate");
    for (index, input) in submitted.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow initiate input");
    }
    for (index, output) in submitted.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow initiate output"
        );
    }
    tracing::info!(?confirmed, "wallet flow initiate db row");
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_batches_all_request_types_on_regtest() {
    tracing::info!("mixed batch test: setting up wallets, HTLCs, and batcher harness");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let counterparty = env.funded_random_wallet().await;
    let cancel_recipient = BitcoinTestEnv::random_wallet();
    let db = env.new_test_db().await;
    let harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let redeem_secret = b"batcher-mixed-redeem-secret";
    let instant_params = env.htlc_params_for_wallets(
        counterparty.as_ref(),
        batcher_wallet.as_ref(),
        b"batcher-mixed-instant",
        30_000,
        6,
    );
    let redeem_params = env.htlc_params_for_wallets(
        counterparty.as_ref(),
        batcher_wallet.as_ref(),
        redeem_secret,
        28_000,
        6,
    );
    let refund_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        counterparty.as_ref(),
        b"batcher-mixed-refund",
        26_000,
        1,
    );
    let send_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        counterparty.as_ref(),
        b"batcher-mixed-send",
        11_000,
        9,
    );

    let instant_addr = get_htlc_address(&instant_params, NETWORK).expect("instant address");
    let redeem_addr = get_htlc_address(&redeem_params, NETWORK).expect("redeem address");
    let refund_addr = get_htlc_address(&refund_params, NETWORK).expect("refund address");
    let send_addr = get_htlc_address(&send_params, NETWORK).expect("send address");

    let (_, instant_utxos) = env
        .fund_htlc(
            Arc::clone(&counterparty),
            &instant_params,
            DEFAULT_FEE_RATE,
        )
        .await;
    env.fund_htlc(Arc::clone(&counterparty), &redeem_params, DEFAULT_FEE_RATE)
        .await;
    env.fund_htlc(
        Arc::clone(&batcher_wallet),
        &refund_params,
        DEFAULT_FEE_RATE,
    )
    .await;
    env.mine_blocks(refund_params.timelock).await;

    let instant_refund_request = env
        .prepare_single_htlc_action(
            Arc::clone(&batcher_wallet),
            HtlcAction::InstantRefund {
                dedupe_key_prefix: "mixed-instant".to_string(),
                htlc_address: instant_addr.clone(),
                params: instant_params.clone(),
                recipient: cancel_recipient.address().clone(),
                instant_refund_tx_hex: env.initiator_signed_instant_refund_tx_hex(
                    &instant_params,
                    instant_utxos.clone(),
                    cancel_recipient.address().clone(),
                    counterparty.as_ref(),
                ),
            },
        )
        .await;
    let instant_request_key = instant_refund_request.dedupe_key().to_string();

    tracing::info!(
        "mixed batch test: submitting instant refund, redeem, refund, and initiate requests"
    );
    harness.submit(instant_refund_request).await;
    harness
        .submit(
            env.prepare_single_htlc_action(
                Arc::clone(&batcher_wallet),
                HtlcAction::Redeem {
                    dedupe_key: "mixed-redeem".to_string(),
                    htlc_address: redeem_addr.clone(),
                    params: redeem_params.clone(),
                    secret: redeem_secret.to_vec(),
                },
            )
            .await,
        )
        .await;
    harness
        .submit(
            env.prepare_single_htlc_action(
                Arc::clone(&batcher_wallet),
                HtlcAction::Refund {
                    dedupe_key: "mixed-refund".to_string(),
                    htlc_address: refund_addr.clone(),
                    params: refund_params.clone(),
                },
            )
            .await,
        )
        .await;
    harness
        .submit(env.send_request("mixed-send", send_addr.clone(), send_params.amount))
        .await;

    tracing::info!("mixed batch test: waiting for submitted batch tx event");
    let mut harness = harness;
    let submitted = harness.wait_for_submitted_tx().await;
    let txid = submitted.txid;
    let txid_string = txid.to_string();
    assert!(submitted.replaces.is_none());
    assert_request_keys(
        &submitted,
        &[
            instant_request_key.as_str(),
            "mixed-redeem",
            "mixed-refund",
            "mixed-send",
        ],
    );
    for key in [
        instant_request_key.as_str(),
        "mixed-redeem",
        "mixed-refund",
        "mixed-send",
    ] {
        let row = db.wait_for_wallet_status(&scope, key, "inflight").await;
        assert_eq!(row.batch_txid.as_deref(), Some(txid_string.as_str()));
    }
    tracing::info!(%txid, request_count = submitted.request_keys.len(), "mixed batch test: confirming batch tx");
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&txid).await;

    tracing::info!(%txid, "mixed batch test: asserting on-chain effects for all request types");
    env.assert_htlc_consumed(&instant_addr).await;
    env.assert_htlc_consumed(&redeem_addr).await;
    env.assert_htlc_consumed(&refund_addr).await;
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&send_addr, 1).await[0].value,
        send_params.amount
    );

    let on_chain = env.tx_from_chain(&txid).await;
    let sacp = TapSighashType::SinglePlusAnyoneCanPay as u8;
    assert_eq!(
        on_chain.output[0].script_pubkey,
        cancel_recipient.address().script_pubkey()
    );
    assert_eq!(on_chain.output[0].value.to_sat(), instant_params.amount);
    assert!(on_chain
        .output
        .iter()
        .any(|output| output.script_pubkey == send_addr.script_pubkey()
            && output.value.to_sat() == send_params.amount));
    assert!(on_chain.input.iter().any(|input| {
        input.witness.len() == 4
            && input.witness.nth(0).and_then(|sig| sig.last().copied()) == Some(sacp)
            && input.witness.nth(1).and_then(|sig| sig.last().copied()) == Some(sacp)
    }));
    assert!(on_chain.input.iter().any(|input| {
        input.witness.len() == 4
            && input
                .witness
                .nth(1)
                .map(|secret| secret == redeem_secret)
                .unwrap_or(false)
    }));
    assert!(on_chain.input.iter().any(|input| {
        input.witness.len() == 3
            && input.sequence.to_consensus_u32() == refund_params.timelock as u32
    }));
    for key in [
        instant_request_key.as_str(),
        "mixed-redeem",
        "mixed-refund",
        "mixed-send",
    ] {
        let row = db.wait_for_wallet_status(&scope, key, "confirmed").await;
        assert_eq!(row.batch_txid.as_deref(), Some(txid_string.as_str()));
        assert!(row.chain_anchor.is_none());
    }
    tracing::info!(scope = %scope, %txid, "wallet flow mixed");
    for (index, input) in submitted.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow mixed input");
    }
    for (index, output) in on_chain.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow mixed output"
        );
    }
    tracing::info!(db_rows = ?db.wallet_rows(&scope).await, "wallet flow mixed db rows");
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_duplicate_request_is_noop_while_inflight_and_confirmed() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let mut harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;

    let params = env.htlc_params(b"batcher-duplicate-live", 25_000, 6);
    let htlc_addr = get_htlc_address(&params, NETWORK).expect("duplicate live htlc address");
    let request_key = "duplicate-live";
    let request = env.send_request(request_key, htlc_addr.clone(), params.amount);

    harness.submit(request.clone()).await;
    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    assert!(first_submission.replaces.is_none());
    assert_request_keys(&first_submission, &[request_key]);

    let inflight = db
        .wait_for_wallet_status(&scope, request_key, "inflight")
        .await;
    assert_eq!(
        inflight.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(inflight.txid_history, vec![first_txid_string.clone()]);

    let replayed_inflight_txid = harness.submit_and_wait(request.clone()).await;
    assert_eq!(replayed_inflight_txid, first_txid);
    harness
        .assert_no_submitted_tx_within(Duration::from_secs(BATCHER_INTERVAL_SECS * 2 + 1))
        .await;

    let inflight_after_duplicate = db.wallet_row(&scope, request_key).await;
    assert_eq!(
        inflight_after_duplicate.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(
        inflight_after_duplicate.txid_history,
        vec![first_txid_string.clone()]
    );
    assert_eq!(inflight_after_duplicate.status, "inflight");

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&first_txid).await;
    let confirmed = db
        .wait_for_wallet_status(&scope, request_key, "confirmed")
        .await;
    assert_eq!(
        confirmed.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(confirmed.txid_history, vec![first_txid_string.clone()]);

    let replayed_confirmed_txid = harness.submit_and_wait(request).await;
    assert_eq!(replayed_confirmed_txid, first_txid);
    harness
        .assert_no_submitted_tx_within(Duration::from_secs(BATCHER_INTERVAL_SECS * 2 + 1))
        .await;

    tracing::info!(scope = %scope, txid = %first_txid, "wallet flow duplicate noop");
    for (index, input) in first_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow duplicate noop input");
    }
    for (index, output) in first_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow duplicate noop output"
        );
    }
    tracing::info!(?confirmed, "wallet flow duplicate noop db row");
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_rbf_replaces_inflight_and_keeps_old_requests() {
    tracing::info!("single RBF test: setting up batcher with two initiate requests");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    env.fund_with_merry(batcher_wallet.address()).await;
    env.fund_with_merry(batcher_wallet.address()).await;
    let initial_cover_utxos = env
        .get_confirmed_cover_utxos(batcher_wallet.address())
        .await;
    assert!(
        initial_cover_utxos.len() >= 3,
        "expected at least 3 confirmed cover utxos before RBF test, found {}",
        initial_cover_utxos.len()
    );
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-rbf-first",
        12_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-rbf-second",
        13_000,
        7,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("first address");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("second address");

    harness
        .submit(env.send_request("rbf-first", first_addr.clone(), first_params.amount))
        .await;
    tracing::info!("single RBF test: waiting for first submitted parent tx");
    let mut harness = harness;
    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    assert!(first_submission.replaces.is_none());
    assert_request_keys(&first_submission, &["rbf-first"]);
    let first_inflight = db
        .wait_for_wallet_status(&scope, "rbf-first", "inflight")
        .await;
    assert_eq!(
        first_inflight.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(first_inflight.txid_history, vec![first_txid_string.clone()]);
    assert!(first_inflight.lineage_id.is_some());
    assert!(first_inflight.chain_anchor.is_none());

    harness
        .submit(env.send_request("rbf-second", second_addr.clone(), second_params.amount))
        .await;
    tracing::info!(replaces = %first_txid, "single RBF test: waiting for replacement tx");
    let replacement_submission = harness.wait_for_submitted_tx().await;
    let replacement_txid = replacement_submission.txid;
    let replacement_txid_string = replacement_txid.to_string();
    assert_eq!(replacement_submission.replaces, Some(first_txid));
    assert_request_keys(&replacement_submission, &["rbf-first", "rbf-second"]);
    assert_ne!(replacement_txid, first_txid);
    let first_replaced = db
        .wait_for_wallet_status(&scope, "rbf-first", "inflight")
        .await;
    let second_inflight = db
        .wait_for_wallet_status(&scope, "rbf-second", "inflight")
        .await;
    assert_eq!(
        first_replaced.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        second_inflight.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        first_replaced.txid_history,
        vec![first_txid_string.clone(), replacement_txid_string.clone()]
    );
    assert_eq!(
        second_inflight.txid_history,
        vec![replacement_txid_string.clone()]
    );
    assert_eq!(first_replaced.lineage_id, second_inflight.lineage_id);
    assert!(first_replaced.chain_anchor.is_none());
    assert!(second_inflight.chain_anchor.is_none());

    tracing::info!(old = %first_txid, new = %replacement_txid, "single RBF test: confirming replacement tx and final outputs");
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&replacement_txid).await;
    let first_confirmed = db
        .wait_for_wallet_status(&scope, "rbf-first", "confirmed")
        .await;
    let second_confirmed = db
        .wait_for_wallet_status(&scope, "rbf-second", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        second_confirmed.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        first_confirmed.txid_history,
        vec![first_txid_string.clone(), replacement_txid_string.clone()]
    );
    assert_eq!(
        second_confirmed.txid_history,
        vec![replacement_txid_string.clone()]
    );
    assert_eq!(first_confirmed.lineage_id, second_confirmed.lineage_id);
    assert!(first_confirmed.chain_anchor.is_none());
    assert!(second_confirmed.chain_anchor.is_none());
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&first_addr, 1)
            .await[0]
            .value,
        first_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&second_addr, 1)
            .await[0]
            .value,
        second_params.amount
    );

    tracing::info!(scope = %scope, ?initial_cover_utxos, first_txid = %first_txid, "wallet flow rbf");
    for (index, input) in first_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow rbf first input");
    }
    for (index, output) in first_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow rbf first output"
        );
    }
    tracing::info!(replacement_txid = %replacement_txid, "wallet flow rbf replacement");
    for (index, input) in replacement_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow rbf replacement input");
    }
    for (index, output) in replacement_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow rbf replacement output"
        );
    }
    tracing::info!(
        db_rows = ?vec![first_confirmed.clone(), second_confirmed.clone()],
        "wallet flow rbf db rows"
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_large_second_send_with_descendant_pressure_prefers_new_fresh_lineage() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    env.fund_with_merry(batcher_wallet.address()).await;
    env.fund_with_merry(batcher_wallet.address()).await;
    env.fund_with_merry(batcher_wallet.address()).await;
    let initial_cover_utxos = env
        .get_confirmed_cover_utxos(batcher_wallet.address())
        .await;
    assert!(
        initial_cover_utxos.len() >= 4,
        "expected at least 4 confirmed cover utxos before large-send split test, found {}",
        initial_cover_utxos.len()
    );

    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let mut harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;

    let first_recipient = BitcoinTestEnv::random_wallet();
    let second_recipient = BitcoinTestEnv::random_wallet();
    let amount = 150_000_000u64;

    harness
        .submit(env.send_request(
            "large-fresh-first",
            first_recipient.address().clone(),
            amount,
        ))
        .await;
    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    assert!(first_submission.replaces.is_none());
    assert_request_keys(&first_submission, &["large-fresh-first"]);
    env.wait_for_tx_in_mempool(&first_txid).await;

    let first_inflight = db
        .wait_for_wallet_status(&scope, "large-fresh-first", "inflight")
        .await;
    assert_eq!(
        first_inflight.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(first_inflight.txid_history, vec![first_txid_string.clone()]);

    let parent_fee_rate = env
        .bitcoind
        .get_rbf_tx_fee_info(&first_txid.to_string())
        .await
        .expect("first large parent fee info")
        .tx_fee_rate;
    let child_fee_rate = (parent_fee_rate * 10.0).max(20.0);
    let change_utxo = env.address_output_utxo(
        &first_submission.raw_tx,
        first_txid,
        batcher_wallet.address(),
    );
    let child_recipient = BitcoinTestEnv::random_wallet();
    let descendant_tx = env
        .build_send_from_wallet_utxo(
            Arc::clone(&batcher_wallet),
            change_utxo.clone(),
            child_recipient.address().clone(),
            change_utxo.value.saturating_sub(50_000),
            child_fee_rate,
        )
        .await;
    let descendant_txid = env
        .broadcast_to_mempool("large fresh descendant", &descendant_tx)
        .await;
    env.wait_for_tx_in_mempool(&descendant_txid).await;

    harness
        .submit(env.send_request(
            "large-fresh-second",
            second_recipient.address().clone(),
            amount,
        ))
        .await;
    let second_submission = harness.wait_for_submitted_tx().await;
    let second_txid = second_submission.txid;
    let second_txid_string = second_txid.to_string();
    assert!(second_submission.replaces.is_none());
    assert_request_keys(&second_submission, &["large-fresh-second"]);
    assert_ne!(second_txid, first_txid);
    assert_ne!(second_submission.lineage_id, first_submission.lineage_id);

    let first_still_inflight = db.wallet_row(&scope, "large-fresh-first").await;
    let second_inflight = db
        .wait_for_wallet_status(&scope, "large-fresh-second", "inflight")
        .await;
    assert_eq!(
        first_still_inflight.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(
        second_inflight.batch_txid.as_deref(),
        Some(second_txid_string.as_str())
    );
    assert_eq!(
        first_still_inflight.txid_history,
        vec![first_txid_string.clone()]
    );
    assert_eq!(
        second_inflight.txid_history,
        vec![second_txid_string.clone()]
    );
    assert_ne!(first_still_inflight.lineage_id, second_inflight.lineage_id);
    env.wait_for_tx_in_mempool(&first_txid).await;
    env.wait_for_tx_in_mempool(&descendant_txid).await;
    env.wait_for_tx_in_mempool(&second_txid).await;

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&first_txid).await;
    env.wait_for_confirmed_tx(&descendant_txid).await;
    env.wait_for_confirmed_tx(&second_txid).await;

    let first_confirmed = db
        .wait_for_wallet_status(&scope, "large-fresh-first", "confirmed")
        .await;
    let second_confirmed = db
        .wait_for_wallet_status(&scope, "large-fresh-second", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(
        second_confirmed.batch_txid.as_deref(),
        Some(second_txid_string.as_str())
    );
    assert_eq!(
        first_confirmed.txid_history,
        vec![first_txid_string.clone()]
    );
    assert_eq!(
        second_confirmed.txid_history,
        vec![second_txid_string.clone()]
    );
    assert_ne!(first_confirmed.lineage_id, second_confirmed.lineage_id);

    tracing::info!(
        scope = %scope,
        ?initial_cover_utxos,
        parent_fee_rate,
        descendant_fee_rate = child_fee_rate,
        first_txid = %first_txid,
        "wallet flow large fresh split"
    );
    for (index, input) in first_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow large fresh first input");
    }
    for (index, output) in first_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow large fresh first output"
        );
    }
    tracing::info!(descendant_txid = %descendant_txid, "wallet flow large fresh descendant");
    for (index, input) in descendant_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow large fresh descendant input");
    }
    for (index, output) in descendant_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow large fresh descendant output"
        );
    }
    tracing::info!(second_txid = %second_txid, "wallet flow large fresh second tx");
    for (index, input) in second_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow large fresh second input");
    }
    for (index, output) in second_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow large fresh second output"
        );
    }
    tracing::info!(db_rows = ?db.wallet_rows(&scope).await, "wallet flow large fresh db rows");
}

// restart tests
#[serial]
#[tokio::test]
async fn bitcoin_batcher_recovers_mempool_inflight_after_restart_and_rbf_merges_new_requests() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut first_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-restart-first",
        17_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-restart-second",
        19_000,
        7,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("first restart address");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("second restart address");

    first_harness
        .submit(env.send_request("restart-first", first_addr.clone(), first_params.amount))
        .await;
    let first_submission = first_harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    env.wait_for_tx_in_mempool(&first_txid).await;
    let first_inflight = db
        .wait_for_wallet_status(&scope, "restart-first", "inflight")
        .await;
    assert_eq!(
        first_inflight.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(first_inflight.txid_history, vec![first_txid_string.clone()]);
    assert!(first_inflight.chain_anchor.is_none());
    drop(first_harness);
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    tokio::time::sleep(Duration::from_secs(BATCHER_INTERVAL_SECS + 1)).await;
    let recovered_row = db.wallet_row(&scope, "restart-first").await;
    assert_eq!(recovered_row.status, "inflight");
    assert_eq!(
        recovered_row.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(recovered_row.txid_history, vec![first_txid_string.clone()]);
    restarted_harness
        .submit(env.send_request("restart-second", second_addr.clone(), second_params.amount))
        .await;

    let replacement_submission = restarted_harness.wait_for_submitted_tx().await;
    let replacement_txid = replacement_submission.txid;
    let replacement_txid_string = replacement_txid.to_string();
    assert_eq!(replacement_submission.replaces, Some(first_txid));
    assert_request_keys(
        &replacement_submission,
        &["restart-first", "restart-second"],
    );
    assert_ne!(replacement_txid, first_txid);
    let first_replaced = db
        .wait_for_wallet_status(&scope, "restart-first", "inflight")
        .await;
    let second_replaced = db
        .wait_for_wallet_status(&scope, "restart-second", "inflight")
        .await;
    assert_eq!(
        first_replaced.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        second_replaced.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        first_replaced.txid_history,
        vec![first_txid_string.clone(), replacement_txid_string.clone()]
    );
    assert_eq!(
        second_replaced.txid_history,
        vec![replacement_txid_string.clone()]
    );
    assert_eq!(first_replaced.lineage_id, second_replaced.lineage_id);

    env.wait_for_tx_not_in_mempool(&first_txid).await;
    env.wait_for_tx_in_mempool(&replacement_txid).await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&replacement_txid).await;
    let first_confirmed = db
        .wait_for_wallet_status(&scope, "restart-first", "confirmed")
        .await;
    let second_confirmed = db
        .wait_for_wallet_status(&scope, "restart-second", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        second_confirmed.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        first_confirmed.txid_history,
        vec![first_txid_string.clone(), replacement_txid_string.clone()]
    );
    assert_eq!(
        second_confirmed.txid_history,
        vec![replacement_txid_string.clone()]
    );
    assert_eq!(first_confirmed.lineage_id, second_confirmed.lineage_id);
    assert!(first_confirmed.chain_anchor.is_none());
    assert!(second_confirmed.chain_anchor.is_none());
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&first_addr, 1)
            .await[0]
            .value,
        first_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&second_addr, 1)
            .await[0]
            .value,
        second_params.amount
    );
    tracing::info!(scope = %scope, first_txid = %first_txid, replacement_txid = %replacement_txid, "wallet flow restart mempool");
    for (index, input) in first_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow restart mempool first input");
    }
    for (index, output) in first_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(index, value_sats = output.value.to_sat(), script_pubkey = %output.script_pubkey, "wallet flow restart mempool first output");
    }
    for (index, input) in replacement_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow restart mempool replacement input");
    }
    for (index, output) in replacement_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(index, value_sats = output.value.to_sat(), script_pubkey = %output.script_pubkey, "wallet flow restart mempool replacement output");
    }
    tracing::info!(first_row = ?first_confirmed, second_row = ?second_confirmed, "wallet flow restart mempool db rows");
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_recovers_confirmed_state_after_restart_and_starts_new_fresh_batch() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut first_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-restart-confirmed-first",
        18_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-restart-confirmed-second",
        20_000,
        7,
    );
    let first_addr =
        get_htlc_address(&first_params, NETWORK).expect("first confirmed restart address");
    let second_addr =
        get_htlc_address(&second_params, NETWORK).expect("second confirmed restart address");

    first_harness
        .submit(env.send_request(
            "restart-confirmed-first",
            first_addr.clone(),
            first_params.amount,
        ))
        .await;
    let first_submission = first_harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    let first_inflight = db
        .wait_for_wallet_status(&scope, "restart-confirmed-first", "inflight")
        .await;
    assert_eq!(
        first_inflight.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(first_inflight.txid_history, vec![first_txid_string.clone()]);

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&first_txid).await;
    let first_confirmed = db
        .wait_for_wallet_status(&scope, "restart-confirmed-first", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(
        first_confirmed.txid_history,
        vec![first_txid_string.clone()]
    );
    assert!(first_confirmed.chain_anchor.is_none());
    drop(first_harness);
    tokio::time::sleep(Duration::from_secs(2)).await;

    let mut restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    tokio::time::sleep(Duration::from_secs(BATCHER_INTERVAL_SECS + 1)).await;
    let recovered_confirmed = db.wallet_row(&scope, "restart-confirmed-first").await;
    assert_eq!(recovered_confirmed.status, "confirmed");
    assert_eq!(
        recovered_confirmed.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(
        recovered_confirmed.txid_history,
        vec![first_txid_string.clone()]
    );

    restarted_harness
        .submit(env.send_request(
            "restart-confirmed-second",
            second_addr.clone(),
            second_params.amount,
        ))
        .await;

    let second_submission = restarted_harness.wait_for_submitted_tx().await;
    let second_txid = second_submission.txid;
    let second_txid_string = second_txid.to_string();
    assert!(second_submission.replaces.is_none());
    assert_request_keys(&second_submission, &["restart-confirmed-second"]);
    assert_ne!(second_txid, first_txid);

    let second_inflight = db
        .wait_for_wallet_status(&scope, "restart-confirmed-second", "inflight")
        .await;
    assert_eq!(
        second_inflight.batch_txid.as_deref(),
        Some(second_txid_string.as_str())
    );
    assert_eq!(
        second_inflight.txid_history,
        vec![second_txid_string.clone()]
    );
    assert_ne!(recovered_confirmed.lineage_id, second_inflight.lineage_id);

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&second_txid).await;
    let second_confirmed = db
        .wait_for_wallet_status(&scope, "restart-confirmed-second", "confirmed")
        .await;
    let first_still_confirmed = db
        .wait_for_wallet_status(&scope, "restart-confirmed-first", "confirmed")
        .await;
    assert_eq!(
        first_still_confirmed.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(
        first_still_confirmed.txid_history,
        vec![first_txid_string.clone()]
    );
    assert_eq!(
        second_confirmed.batch_txid.as_deref(),
        Some(second_txid_string.as_str())
    );
    assert_eq!(
        second_confirmed.txid_history,
        vec![second_txid_string.clone()]
    );
    assert!(second_confirmed.chain_anchor.is_none());

    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&first_addr, 1)
            .await[0]
            .value,
        first_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&second_addr, 1)
            .await[0]
            .value,
        second_params.amount
    );
    tracing::info!(scope = %scope, first_txid = %first_txid, second_txid = %second_txid, "wallet flow restart confirmed");
    for (index, input) in first_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow restart confirmed first input");
    }
    for (index, output) in first_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(index, value_sats = output.value.to_sat(), script_pubkey = %output.script_pubkey, "wallet flow restart confirmed first output");
    }
    for (index, input) in second_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(index, previous_output = %input.previous_output, "wallet flow restart confirmed second input");
    }
    for (index, output) in second_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(index, value_sats = output.value.to_sat(), script_pubkey = %output.script_pubkey, "wallet flow restart confirmed second output");
    }
    tracing::info!(first_row = ?first_still_confirmed, second_row = ?second_confirmed, "wallet flow restart confirmed db rows");
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_reorg_invalidate_confirmed_tx_restart_recovers_mempool_lineage() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut first_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-restart-first",
        16_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-restart-second",
        18_000,
        7,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("reorg first address");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("reorg second address");

    first_harness
        .submit(env.send_request(
            "reorg-restart-first",
            first_addr.clone(),
            first_params.amount,
        ))
        .await;
    let first_submission = first_harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    env.wait_for_tx_in_mempool(&first_txid).await;
    db.wait_for_wallet_status(&scope, "reorg-restart-first", "inflight")
        .await;
    drop(first_harness);
    tokio::time::sleep(Duration::from_secs(2)).await;

    env.mine_blocks(1).await;
    let confirmed_blockhash = env.best_block_hash().await;
    env.wait_for_confirmed_tx(&first_txid).await;
    env.invalidate_block(&confirmed_blockhash).await;
    env.wait_for_tx_not_confirmed(&first_txid).await;
    env.wait_for_tx_in_mempool(&first_txid).await;

    let mut restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    tokio::time::sleep(Duration::from_secs(BATCHER_INTERVAL_SECS + 1)).await;
    let recovered_row = db.wallet_row(&scope, "reorg-restart-first").await;
    assert_eq!(recovered_row.status, "inflight");
    assert_eq!(
        recovered_row.batch_txid.as_deref(),
        Some(first_txid_string.as_str())
    );
    assert_eq!(recovered_row.txid_history, vec![first_txid_string.clone()]);

    restarted_harness
        .submit(env.send_request(
            "reorg-restart-second",
            second_addr.clone(),
            second_params.amount,
        ))
        .await;
    let replacement_submission = restarted_harness.wait_for_submitted_tx().await;
    let replacement_txid = replacement_submission.txid;
    let replacement_txid_string = replacement_txid.to_string();
    assert_eq!(replacement_submission.replaces, Some(first_txid));
    assert_request_keys(
        &replacement_submission,
        &["reorg-restart-first", "reorg-restart-second"],
    );

    let first_replaced = db
        .wait_for_wallet_status(&scope, "reorg-restart-first", "inflight")
        .await;
    let second_replaced = db
        .wait_for_wallet_status(&scope, "reorg-restart-second", "inflight")
        .await;
    assert_eq!(
        first_replaced.txid_history,
        vec![first_txid_string.clone(), replacement_txid_string.clone()]
    );
    assert_eq!(
        second_replaced.txid_history,
        vec![replacement_txid_string.clone()]
    );

    env.wait_for_tx_not_in_mempool(&first_txid).await;
    env.wait_for_tx_in_mempool(&replacement_txid).await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&replacement_txid).await;
    let first_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-restart-first", "confirmed")
        .await;
    let second_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-restart-second", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        second_confirmed.batch_txid.as_deref(),
        Some(replacement_txid_string.as_str())
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&first_addr, 1)
            .await[0]
            .value,
        first_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&second_addr, 1)
            .await[0]
            .value,
        second_params.amount
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_restart_recovers_confirmed_older_sibling_when_stored_head_is_missing() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-sibling-confirmed-first",
        21_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-sibling-confirmed-second",
        22_000,
        7,
    );
    let third_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-sibling-confirmed-third",
        23_000,
        8,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("confirmed sibling first");
    let third_addr = get_htlc_address(&third_params, NETWORK).expect("confirmed sibling third");
    let first_request = env.send_request(
        "reorg-sibling-confirmed-first",
        first_addr.clone(),
        first_params.amount,
    );
    let second_request = env.send_request(
        "reorg-sibling-confirmed-second",
        get_htlc_address(&second_params, NETWORK).expect("confirmed sibling second"),
        second_params.amount,
    );
    let third_request = env.send_request(
        "reorg-sibling-confirmed-third",
        third_addr.clone(),
        third_params.amount,
    );

    let mut harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    harness.submit(first_request.clone()).await;
    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&first_txid).await;
    drop(harness);

    let lineage_id = first_submission.lineage_id;
    let missing_head_txid = fake_txid(0xab);
    db.upsert_wallet_request_row(
        &scope,
        &first_request,
        WalletRowUpdate {
            status: "inflight",
            lineage_id: Some(lineage_id),
            batch_txid: Some(missing_head_txid),
            txid_history: &[first_txid, missing_head_txid],
            chain_anchor: None,
        },
    )
    .await;
    db.upsert_wallet_request_row(
        &scope,
        &second_request,
        WalletRowUpdate {
            status: "inflight",
            lineage_id: Some(lineage_id),
            batch_txid: Some(missing_head_txid),
            txid_history: &[missing_head_txid],
            chain_anchor: None,
        },
    )
    .await;

    let mut restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    tokio::time::sleep(Duration::from_secs(BATCHER_INTERVAL_SECS + 1)).await;
    let first_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-sibling-confirmed-first", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(first_txid.to_string().as_str())
    );
    assert_eq!(
        first_confirmed.txid_history,
        vec![first_txid.to_string(), missing_head_txid.to_string()]
    );
    let recovered_submission = restarted_harness.wait_for_submitted_tx().await;
    assert!(recovered_submission.replaces.is_none());
    assert_request_keys(&recovered_submission, &["reorg-sibling-confirmed-second"]);
    let second_inflight = db
        .wait_for_wallet_status(&scope, "reorg-sibling-confirmed-second", "inflight")
        .await;
    assert_eq!(
        second_inflight.batch_txid.as_deref(),
        Some(recovered_submission.txid.to_string().as_str())
    );
    assert_eq!(
        second_inflight.txid_history,
        vec![
            missing_head_txid.to_string(),
            recovered_submission.txid.to_string()
        ]
    );
    assert!(second_inflight.chain_anchor.is_none());

    restarted_harness.submit(third_request).await;
    let replacement_submission = restarted_harness.wait_for_submitted_tx().await;
    assert_eq!(
        replacement_submission.replaces,
        Some(recovered_submission.txid)
    );
    assert_request_keys(
        &replacement_submission,
        &[
            "reorg-sibling-confirmed-second",
            "reorg-sibling-confirmed-third",
        ],
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_restart_recovers_confirmed_sibling_creates_real_chain_anchor_and_builds_chained_batch(
) {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-chain-anchor-first",
        26_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-chain-anchor-second",
        27_000,
        7,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("chain anchor first");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("chain anchor second");
    let first_request = env.send_request(
        "reorg-chain-anchor-first",
        first_addr.clone(),
        first_params.amount,
    );
    let second_request = env.send_request(
        "reorg-chain-anchor-second",
        second_addr.clone(),
        second_params.amount,
    );

    let first_submission = submit_single_fresh_and_confirm_without_followup_tick(
        &env,
        Arc::clone(&batcher_wallet),
        Arc::clone(&db),
        first_request.clone(),
        6,
    )
    .await;
    let first_txid = first_submission.txid;
    let confirmed_blockhash = env.best_block_hash().await;
    let confirmed_height = env
        .electrs
        .get_tx_status(&first_txid.to_string())
        .await
        .expect("confirmed tx status")
        .block_height
        .expect("confirmed tx block height");
    let first_inflight = db.wallet_row(&scope, "reorg-chain-anchor-first").await;
    assert_eq!(first_inflight.status, "inflight");

    let lineage_id = first_submission.lineage_id;
    let missing_head_txid = fake_txid(0xcd);
    persist_fake_rbf_head_via_store(
        &db,
        &scope,
        lineage_id,
        missing_head_txid,
        &first_request,
        std::slice::from_ref(&second_request),
        &bitcoin::consensus::encode::serialize_hex(&first_submission.raw_tx),
    )
    .await;

    let mut restarted_harness = env
        .spawn_batcher_with_db_and_config(
            Arc::clone(&batcher_wallet),
            Arc::clone(&db),
            BATCHER_INTERVAL_SECS,
            6,
        )
        .await;

    let first_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-chain-anchor-first", "confirmed")
        .await;
    assert_eq!(
        first_confirmed.batch_txid.as_deref(),
        Some(first_txid.to_string().as_str())
    );

    let chained_submission = restarted_harness.wait_for_submitted_tx().await;
    assert!(chained_submission.replaces.is_none());
    assert_request_keys(&chained_submission, &["reorg-chain-anchor-second"]);

    let anchor_input = bitcoin::OutPoint {
        txid: first_txid,
        vout: 1,
    };
    assert_eq!(
        chained_submission
            .raw_tx
            .input
            .last()
            .expect("chained tx input")
            .previous_output,
        anchor_input
    );

    let second_inflight = db
        .wait_for_wallet_status(&scope, "reorg-chain-anchor-second", "inflight")
        .await;
    let anchor = second_inflight
        .chain_anchor
        .clone()
        .expect("chained inflight request should retain anchor");
    assert_eq!(
        anchor["confirmed_txid"].as_str(),
        Some(first_txid.to_string().as_str())
    );
    assert_eq!(anchor["confirmed_height"].as_u64(), Some(confirmed_height));
    assert_eq!(
        second_inflight.txid_history,
        vec![
            missing_head_txid.to_string(),
            chained_submission.txid.to_string()
        ]
    );
    assert_eq!(
        second_inflight.batch_txid.as_deref(),
        Some(chained_submission.txid.to_string().as_str())
    );

    env.wait_for_tx_in_mempool(&chained_submission.txid).await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&chained_submission.txid).await;

    let second_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-chain-anchor-second", "confirmed")
        .await;
    assert_eq!(
        second_confirmed.batch_txid.as_deref(),
        Some(chained_submission.txid.to_string().as_str())
    );
    assert_eq!(
        second_confirmed.txid_history,
        vec![
            missing_head_txid.to_string(),
            chained_submission.txid.to_string()
        ]
    );
    assert!(second_confirmed.chain_anchor.is_none());

    tracing::info!(
        scope = %scope,
        first_txid = %first_txid,
        confirmed_blockhash = %confirmed_blockhash,
        missing_head_txid = %missing_head_txid,
        chained_txid = %chained_submission.txid,
        chain_anchor = ?second_inflight.chain_anchor,
        "wallet flow real chained anchor"
    );
    for (index, input) in chained_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(
            index,
            previous_output = %input.previous_output,
            "wallet flow real chained anchor input"
        );
    }
    for (index, output) in chained_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow real chained anchor output"
        );
    }
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_restart_recovers_confirmed_sibling_creates_single_real_chain_anchor_for_multiple_orphans(
) {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-multi-anchor-first",
        28_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-multi-anchor-second",
        29_000,
        7,
    );
    let third_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-multi-anchor-third",
        30_000,
        8,
    );

    let first_request = env.send_request(
        "reorg-multi-anchor-first",
        get_htlc_address(&first_params, NETWORK).expect("multi anchor first"),
        first_params.amount,
    );
    let second_request = env.send_request(
        "reorg-multi-anchor-second",
        get_htlc_address(&second_params, NETWORK).expect("multi anchor second"),
        second_params.amount,
    );
    let third_request = env.send_request(
        "reorg-multi-anchor-third",
        get_htlc_address(&third_params, NETWORK).expect("multi anchor third"),
        third_params.amount,
    );

    let first_submission = submit_single_fresh_and_confirm_without_followup_tick(
        &env,
        Arc::clone(&batcher_wallet),
        Arc::clone(&db),
        first_request.clone(),
        6,
    )
    .await;
    let first_txid = first_submission.txid;
    assert_eq!(
        db.wallet_row(&scope, "reorg-multi-anchor-first")
            .await
            .status,
        "inflight"
    );

    let missing_head_txid = fake_txid(0xce);
    persist_fake_rbf_head_via_store(
        &db,
        &scope,
        first_submission.lineage_id,
        missing_head_txid,
        &first_request,
        &[second_request.clone(), third_request.clone()],
        &bitcoin::consensus::encode::serialize_hex(&first_submission.raw_tx),
    )
    .await;

    let mut restarted_harness = env
        .spawn_batcher_with_db_and_config(
            Arc::clone(&batcher_wallet),
            Arc::clone(&db),
            BATCHER_INTERVAL_SECS,
            6,
        )
        .await;

    let chained_submission = restarted_harness.wait_for_submitted_tx().await;
    assert!(chained_submission.replaces.is_none());
    assert_request_keys(
        &chained_submission,
        &["reorg-multi-anchor-second", "reorg-multi-anchor-third"],
    );
    assert_eq!(
        chained_submission
            .raw_tx
            .input
            .last()
            .expect("multi orphan chained input")
            .previous_output,
        bitcoin::OutPoint {
            txid: first_txid,
            vout: 1,
        }
    );

    let second_inflight = db
        .wait_for_wallet_status(&scope, "reorg-multi-anchor-second", "inflight")
        .await;
    let third_inflight = db
        .wait_for_wallet_status(&scope, "reorg-multi-anchor-third", "inflight")
        .await;
    assert_eq!(second_inflight.lineage_id, third_inflight.lineage_id);
    assert_eq!(
        second_inflight.batch_txid,
        Some(chained_submission.txid.to_string())
    );
    assert_eq!(second_inflight.batch_txid, third_inflight.batch_txid);
    assert_eq!(
        second_inflight.txid_history,
        vec![
            missing_head_txid.to_string(),
            chained_submission.txid.to_string()
        ]
    );
    assert_eq!(second_inflight.txid_history, third_inflight.txid_history);
    assert_eq!(second_inflight.chain_anchor, third_inflight.chain_anchor);

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&chained_submission.txid).await;

    let second_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-multi-anchor-second", "confirmed")
        .await;
    let third_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-multi-anchor-third", "confirmed")
        .await;
    assert!(second_confirmed.chain_anchor.is_none());
    assert!(third_confirmed.chain_anchor.is_none());
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_restart_recovers_two_distinct_real_chain_anchors() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    env.fund_with_merry(batcher_wallet.address()).await;
    env.fund_with_merry(batcher_wallet.address()).await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let store = db.store();

    let first_a_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-double-anchor-first-a",
        1_510_000,
        6,
    );
    let second_a_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-double-anchor-second-a",
        1_620_000,
        7,
    );
    let first_b_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-double-anchor-first-b",
        1_730_000,
        8,
    );
    let second_b_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-double-anchor-second-b",
        1_840_000,
        9,
    );

    let first_a_request = env.send_request(
        "reorg-double-anchor-first-a",
        get_htlc_address(&first_a_params, NETWORK).expect("double anchor first a"),
        first_a_params.amount,
    );
    let second_a_request = env.send_request(
        "reorg-double-anchor-second-a",
        get_htlc_address(&second_a_params, NETWORK).expect("double anchor second a"),
        second_a_params.amount,
    );
    let first_b_request = env.send_request(
        "reorg-double-anchor-first-b",
        get_htlc_address(&first_b_params, NETWORK).expect("double anchor first b"),
        first_b_params.amount,
    );
    let second_b_request = env.send_request(
        "reorg-double-anchor-second-b",
        get_htlc_address(&second_b_params, NETWORK).expect("double anchor second b"),
        second_b_params.amount,
    );

    let first_a_submission = submit_single_fresh_and_confirm_without_followup_tick(
        &env,
        Arc::clone(&batcher_wallet),
        Arc::clone(&db),
        first_a_request.clone(),
        6,
    )
    .await;
    assert_eq!(
        db.wallet_row(&scope, "reorg-double-anchor-first-a")
            .await
            .status,
        "inflight"
    );

    let first_b_address =
        get_htlc_address(&first_b_params, NETWORK).expect("double anchor first b address");
    let independent_cover = env
        .get_confirmed_cover_utxos(batcher_wallet.address())
        .await
        .into_iter()
        .find(|utxo| utxo.outpoint.txid != first_a_submission.txid)
        .expect("independent confirmed cover utxo for second anchor");
    let first_b_tx = env
        .build_send_from_wallet_utxo(
            Arc::clone(&batcher_wallet),
            independent_cover,
            first_b_address,
            first_b_params.amount,
            DEFAULT_FEE_RATE,
        )
        .await;
    let first_b_txid = env
        .broadcast_and_confirm("double anchor first b", &first_b_tx)
        .await;
    env.wait_for_confirmed_tx(&first_b_txid).await;
    let first_b_lineage_id = LineageId::new();
    let enqueue_b = store
        .enqueue(&scope, &first_b_request)
        .await
        .expect("enqueue first b request");
    assert!(matches!(
        enqueue_b,
        EnqueueWalletRequestResult::EnqueuedPending
    ));
    store
        .persist_broadcast(
            &scope,
            &BroadcastPersistencePlan {
                kind: BroadcastPersistenceKind::Fresh,
                lineage_id: first_b_lineage_id,
                txid: first_b_txid,
                raw_tx_hex: bitcoin::consensus::encode::serialize_hex(&first_b_tx),
                included_request_keys: vec!["reorg-double-anchor-first-b".to_string()],
                dropped_request_keys: vec![],
            },
        )
        .await
        .expect("persist first b broadcast");

    let missing_head_a = fake_txid(0xd1);
    let missing_head_b = fake_txid(0xd2);
    persist_fake_rbf_head_via_store(
        &db,
        &scope,
        first_a_submission.lineage_id,
        missing_head_a,
        &first_a_request,
        std::slice::from_ref(&second_a_request),
        &bitcoin::consensus::encode::serialize_hex(&first_a_submission.raw_tx),
    )
    .await;
    persist_fake_rbf_head_via_store(
        &db,
        &scope,
        first_b_lineage_id,
        missing_head_b,
        &first_b_request,
        std::slice::from_ref(&second_b_request),
        &bitcoin::consensus::encode::serialize_hex(&first_b_tx),
    )
    .await;

    let mut restarted_harness = env
        .spawn_batcher_with_db_and_config(
            Arc::clone(&batcher_wallet),
            Arc::clone(&db),
            BATCHER_INTERVAL_SECS,
            6,
        )
        .await;

    let first_submission = restarted_harness.wait_for_submitted_tx().await;
    let second_submission = restarted_harness.wait_for_submitted_tx().await;
    let mut submissions = std::collections::BTreeMap::new();
    for submission in [first_submission, second_submission] {
        assert!(submission.replaces.is_none());
        assert_eq!(submission.request_keys.len(), 1);
        submissions.insert(submission.request_keys[0].clone(), submission);
    }

    let chained_a = submissions
        .remove("reorg-double-anchor-second-a")
        .expect("chained submission for anchor a");
    let chained_b = submissions
        .remove("reorg-double-anchor-second-b")
        .expect("chained submission for anchor b");

    assert_eq!(
        chained_a
            .raw_tx
            .input
            .last()
            .expect("anchor a input")
            .previous_output,
        bitcoin::OutPoint {
            txid: first_a_submission.txid,
            vout: 1,
        }
    );
    assert_eq!(
        chained_b
            .raw_tx
            .input
            .last()
            .expect("anchor b input")
            .previous_output,
        bitcoin::OutPoint {
            txid: first_b_txid,
            vout: 1,
        }
    );

    let second_a_inflight = db
        .wait_for_wallet_status(&scope, "reorg-double-anchor-second-a", "inflight")
        .await;
    let second_b_inflight = db
        .wait_for_wallet_status(&scope, "reorg-double-anchor-second-b", "inflight")
        .await;
    let anchor_a = second_a_inflight
        .chain_anchor
        .clone()
        .expect("anchor a must exist");
    let anchor_b = second_b_inflight
        .chain_anchor
        .clone()
        .expect("anchor b must exist");
    assert_ne!(anchor_a, anchor_b);
    assert_eq!(
        anchor_a["confirmed_txid"].as_str(),
        Some(first_a_submission.txid.to_string().as_str())
    );
    assert_eq!(
        anchor_b["confirmed_txid"].as_str(),
        Some(first_b_txid.to_string().as_str())
    );

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&chained_a.txid).await;
    env.wait_for_confirmed_tx(&chained_b.txid).await;

    let second_a_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-double-anchor-second-a", "confirmed")
        .await;
    let second_b_confirmed = db
        .wait_for_wallet_status(&scope, "reorg-double-anchor-second-b", "confirmed")
        .await;
    assert!(second_a_confirmed.chain_anchor.is_none());
    assert!(second_b_confirmed.chain_anchor.is_none());
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_restart_adopts_mempool_older_sibling_when_stored_head_is_missing() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-sibling-mempool-first",
        24_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-sibling-mempool-second",
        25_000,
        7,
    );
    let third_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-reorg-sibling-mempool-third",
        26_000,
        8,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("mempool sibling first");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("mempool sibling second");
    let third_addr = get_htlc_address(&third_params, NETWORK).expect("mempool sibling third");
    let first_request = env.send_request(
        "reorg-sibling-mempool-first",
        first_addr.clone(),
        first_params.amount,
    );
    let second_request = env.send_request(
        "reorg-sibling-mempool-second",
        second_addr.clone(),
        second_params.amount,
    );
    let third_request = env.send_request(
        "reorg-sibling-mempool-third",
        third_addr.clone(),
        third_params.amount,
    );

    let mut harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    harness.submit(first_request.clone()).await;
    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    env.wait_for_tx_in_mempool(&first_txid).await;
    drop(harness);

    let lineage_id = first_submission.lineage_id;
    let missing_head_txid = fake_txid(0xcd);
    db.upsert_wallet_request_row(
        &scope,
        &first_request,
        WalletRowUpdate {
            status: "inflight",
            lineage_id: Some(lineage_id),
            batch_txid: Some(missing_head_txid),
            txid_history: &[first_txid, missing_head_txid],
            chain_anchor: None,
        },
    )
    .await;
    db.upsert_wallet_request_row(
        &scope,
        &second_request,
        WalletRowUpdate {
            status: "inflight",
            lineage_id: Some(lineage_id),
            batch_txid: Some(missing_head_txid),
            txid_history: &[missing_head_txid],
            chain_anchor: None,
        },
    )
    .await;

    let mut restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    tokio::time::sleep(Duration::from_secs(BATCHER_INTERVAL_SECS + 1)).await;
    let recovered_first = db.wallet_row(&scope, "reorg-sibling-mempool-first").await;
    let recovered_second = db.wallet_row(&scope, "reorg-sibling-mempool-second").await;
    assert_eq!(recovered_first.status, "inflight");
    assert_eq!(recovered_second.status, "inflight");

    restarted_harness.submit(third_request).await;
    let replacement_submission = restarted_harness.wait_for_submitted_tx().await;
    assert_eq!(replacement_submission.replaces, Some(first_txid));
    assert_request_keys(
        &replacement_submission,
        &[
            "reorg-sibling-mempool-first",
            "reorg-sibling-mempool-second",
            "reorg-sibling-mempool-third",
        ],
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_restart_reuses_recovered_inflight_for_duplicate_request() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut first_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;

    let params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-restart-duplicate",
        21_000,
        8,
    );
    let htlc_addr = get_htlc_address(&params, NETWORK).expect("duplicate restart address");
    let request = env.send_request("restart-duplicate", htlc_addr.clone(), params.amount);

    first_harness.submit(request.clone()).await;
    let first_submission = first_harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    env.wait_for_tx_in_mempool(&first_txid).await;
    drop(first_harness);

    let restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), db)
        .await;
    let replayed_txid = restarted_harness.submit_and_wait(request).await;
    assert_eq!(replayed_txid, first_txid);

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&first_txid).await;
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&htlc_addr, 1).await[0].value,
        params.amount
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_missing_batch_recovers_to_live_batch_and_rbf_merges_delta() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut first_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-missing-head-parent",
        22_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-missing-head-requeued",
        23_000,
        7,
    );
    let third_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-missing-head-new",
        24_000,
        8,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("first address");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("second address");
    let third_addr = get_htlc_address(&third_params, NETWORK).expect("third address");
    let first_request = env.send_request("missing-first", first_addr.clone(), first_params.amount);
    let second_request =
        env.send_request("missing-second", second_addr.clone(), second_params.amount);
    let third_request = env.send_request("missing-third", third_addr.clone(), third_params.amount);

    first_harness.submit(first_request.clone()).await;
    let first_submission = first_harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    env.wait_for_tx_in_mempool(&first_txid).await;

    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let missing_batch_txid =
        bitcoin::Txid::from_str("abababababababababababababababababababababababababababababababab")
            .unwrap();
    let store = db.store();

    store.enqueue(&scope, &second_request).await.unwrap();
    db.upsert_wallet_request_row(
        &scope,
        &first_request,
        WalletRowUpdate {
            status: "inflight",
            lineage_id: Some(first_submission.lineage_id),
            batch_txid: Some(missing_batch_txid),
            txid_history: &[first_txid, missing_batch_txid],
            chain_anchor: None,
        },
    )
    .await;
    db.upsert_wallet_request_row(
        &scope,
        &second_request,
        WalletRowUpdate {
            status: "inflight",
            lineage_id: Some(first_submission.lineage_id),
            batch_txid: Some(missing_batch_txid),
            txid_history: &[missing_batch_txid],
            chain_anchor: None,
        },
    )
    .await;
    drop(first_harness);

    let mut restarted_harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    restarted_harness.submit(third_request.clone()).await;

    let recovered_submission = restarted_harness.wait_for_submitted_tx().await;
    assert_eq!(recovered_submission.replaces, Some(first_txid));
    assert_request_keys(
        &recovered_submission,
        &["missing-first", "missing-second", "missing-third"],
    );

    env.wait_for_tx_not_in_mempool(&first_txid).await;
    env.wait_for_tx_in_mempool(&recovered_submission.txid).await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&recovered_submission.txid).await;
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&first_addr, 1)
            .await[0]
            .value,
        first_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&second_addr, 1)
            .await[0]
            .value,
        second_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&third_addr, 1)
            .await[0]
            .value,
        third_params.amount
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_starts_fresh_batch_after_confirmation() {
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let mut harness = env.spawn_batcher(Arc::clone(&batcher_wallet)).await;

    let first_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-finalize-first",
        14_000,
        6,
    );
    let second_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        env.counterparty.as_ref(),
        b"batcher-finalize-second",
        15_000,
        7,
    );
    let first_addr = get_htlc_address(&first_params, NETWORK).expect("first address");
    let second_addr = get_htlc_address(&second_params, NETWORK).expect("second address");

    harness
        .submit(env.send_request("fresh-first", first_addr.clone(), first_params.amount))
        .await;
    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&first_txid).await;

    harness
        .submit(env.send_request("fresh-second", second_addr.clone(), second_params.amount))
        .await;
    let second_submission = harness.wait_for_submitted_tx().await;
    let second_txid = second_submission.txid;
    assert!(second_submission.replaces.is_none());
    assert_request_keys(&second_submission, &["fresh-second"]);
    assert_ne!(second_txid, first_txid);

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&second_txid).await;
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&first_addr, 1)
            .await[0]
            .value,
        first_params.amount
    );
    assert_eq!(
        env.wait_for_confirmed_cover_utxo_count(&second_addr, 1)
            .await[0]
            .value,
        second_params.amount
    );
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_retries_refund_once_htlc_utxo_exists() {
    tracing::info!("refund retry test: refund preparation should fail before the HTLC exists");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let counterparty = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut harness = env
        .spawn_batcher_with_db(Arc::clone(&batcher_wallet), Arc::clone(&db))
        .await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());

    let refund_params = env.htlc_params_for_wallets(
        batcher_wallet.as_ref(),
        counterparty.as_ref(),
        b"batcher-retry-refund",
        24_000,
        1,
    );
    let refund_addr = get_htlc_address(&refund_params, NETWORK).expect("refund address");
    let adapter = BitcoinHtlcWalletAdapter::new(
        *batcher_wallet.x_only_pubkey(),
        Arc::clone(&env.electrs),
        NETWORK,
    );

    let missing = adapter
        .prepare(HtlcAction::Refund {
            dedupe_key: "retry-refund".to_string(),
            htlc_address: refund_addr.clone(),
            params: refund_params.clone(),
        })
        .await
        .expect_err("refund preparation should fail before HTLC funding");
    assert!(matches!(
        missing,
        HtlcAdapterError::MissingHtlcUtxo {
            expected_value: 24_000,
            ..
        }
    ));
    assert!(db.wallet_rows(&scope).await.is_empty());

    harness
        .assert_no_submitted_tx_within(Duration::from_secs(BATCHER_INTERVAL_SECS + 1))
        .await;

    tracing::info!("refund retry test: funding HTLC and waiting for refund preparation to succeed");
    env.fund_htlc(
        Arc::clone(&batcher_wallet),
        &refund_params,
        DEFAULT_FEE_RATE,
    )
    .await;
    env.mine_blocks(refund_params.timelock).await;
    env.wait_for_confirmed_cover_utxo_count(&refund_addr, 1)
        .await;

    let refund_request = adapter
        .prepare(HtlcAction::Refund {
            dedupe_key: "retry-refund".to_string(),
            htlc_address: refund_addr.clone(),
            params: refund_params.clone(),
        })
        .await
        .expect("refund preparation should succeed once HTLC UTXO exists")
        .into_iter()
        .next()
        .expect("single refund request");

    harness.submit(refund_request).await;

    let submitted = harness.wait_for_submitted_tx().await;
    let txid = submitted.txid;
    let txid_string = txid.to_string();
    assert!(submitted.replaces.is_none());
    assert_request_keys(&submitted, &["retry-refund"]);
    let inflight = db
        .wait_for_wallet_status(&scope, "retry-refund", "inflight")
        .await;
    assert_eq!(inflight.batch_txid.as_deref(), Some(txid_string.as_str()));
    assert_eq!(inflight.txid_history, vec![txid_string.clone()]);
    assert!(inflight.chain_anchor.is_none());
    tracing::info!(%txid, "refund retry test: confirming refund tx after retry");
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&txid).await;
    env.assert_htlc_consumed(&refund_addr).await;
    let confirmed = db
        .wait_for_wallet_status(&scope, "retry-refund", "confirmed")
        .await;
    assert_eq!(confirmed.batch_txid.as_deref(), Some(txid_string.as_str()));
    assert_eq!(confirmed.txid_history, vec![txid_string]);
    assert!(confirmed.chain_anchor.is_none());

    let on_chain = env.tx_from_chain(&txid).await;
    tracing::info!(scope = %scope, %txid, "wallet flow refund");
    for (index, input) in on_chain.input.iter().enumerate() {
        tracing::info!(
            index,
            previous_output = %input.previous_output,
            sequence = input.sequence.to_consensus_u32(),
            witness_items = input.witness.len(),
            "wallet flow refund input"
        );
    }
    for (index, output) in on_chain.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow refund output"
        );
    }
    tracing::info!(?confirmed, "wallet flow refund db_row");
    assert!(on_chain.input.iter().any(|input| {
        input.witness.len() == 3
            && input.sequence.to_consensus_u32() == refund_params.timelock as u32
    }));
}

// descendant eviction tests
#[serial]
#[tokio::test]
async fn bitcoin_batcher_rbf_drops_external_redeem_descendants_and_keeps_all_init_requests() {
    const DESCENDANT_TEST_TICK_INTERVAL_SECS: u64 = 15;

    // Scenario:
    // 1. Batcher creates 5 HTLC funding outputs.
    // 2. External parties attach redeem descendants to those unconfirmed outputs.
    // 3. Batcher replaces the parent with 8 additional initiates.
    // 4. The replacement should evict the old parent and all descendants while
    //    still carrying forward the original 5 requests.
    tracing::info!("descendant eviction test: setting up 5 initial initiates");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let scope = wallet_scope(NETWORK, batcher_wallet.as_ref());
    let mut harness = env
        .spawn_batcher_paused_with_db_and_tick_interval(
            Arc::clone(&batcher_wallet),
            Arc::clone(&db),
            DESCENDANT_TEST_TICK_INTERVAL_SECS,
        )
        .await;

    let initial_specs: Vec<(Vec<u8>, u64, u64)> = (0..5)
        .map(|i| {
            (
                format!("batcher-rbf-desc-init-{i}").into_bytes(),
                20_000 + (i as u64 * 1_000),
                6 + i as u64,
            )
        })
        .collect();
    let initial: Vec<_> = initial_specs
        .iter()
        .enumerate()
        .map(|(index, (secret, amount, timelock))| {
            let params = env.htlc_params_for_wallets(
                batcher_wallet.as_ref(),
                env.counterparty.as_ref(),
                secret,
                *amount,
                *timelock,
            );
            let address = get_htlc_address(&params, NETWORK).expect("initial htlc address");
            (
                format!("desc-initial-{index}"),
                secret.clone(),
                params,
                address,
            )
        })
        .collect();

    for (key, _, params, address) in &initial {
        harness
            .submit(env.send_request(key.clone(), address.clone(), params.amount))
            .await;
    }
    harness.start();

    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    let first_txid_string = first_txid.to_string();
    assert!(first_submission.replaces.is_none());
    assert_eq!(first_submission.request_keys.len(), 5);
    tracing::info!(%first_txid, request_count = 5, "descendant eviction test: first parent accepted");
    for (key, _, _, _) in &initial {
        let row = db.wait_for_wallet_status(&scope, key, "inflight").await;
        assert_eq!(row.batch_txid.as_deref(), Some(first_txid_string.as_str()));
        assert_eq!(row.txid_history, vec![first_txid_string.clone()]);
        assert!(row.chain_anchor.is_none());
    }

    let parent_fee_rate = env
        .bitcoind
        .get_rbf_tx_fee_info(&first_txid.to_string())
        .await
        .expect("first tx fee info")
        .tx_fee_rate;
    let child_fee_rate = (parent_fee_rate * 2.0).max(DEFAULT_FEE_RATE);

    let mut child_txids = Vec::with_capacity(initial.len());
    tracing::info!(
        parent_fee_rate,
        child_fee_rate,
        descendant_count = initial.len(),
        "descendant eviction test: building external redeem descendants"
    );
    for (_, secret, params, address) in &initial {
        let vout = first_submission
            .raw_tx
            .output
            .iter()
            .position(|output| {
                output.script_pubkey == address.script_pubkey()
                    && output.value.to_sat() == params.amount
            })
            .expect("htlc output in first submission") as u32;
        let utxo = CoverUtxo {
            outpoint: bitcoin::OutPoint {
                txid: first_txid,
                vout,
            },
            value: params.amount,
            script_pubkey: address.script_pubkey(),
        };
        let redeem_tx = env
            .build_with_fee_builder(
                Arc::clone(&env.counterparty),
                BitcoinTxAdaptorParams {
                    sacps: vec![],
                    spends: vec![env.redeem_spend(params, secret, vec![utxo])],
                    sends: vec![],
                    fee_rate: child_fee_rate,
                },
            )
            .await;
        let child_txid = env
            .broadcast_to_mempool("descendant redeem", &redeem_tx)
            .await;
        env.wait_for_tx_in_mempool(&child_txid).await;
        child_txids.push(child_txid);
    }

    let replacement_specs: Vec<(Vec<u8>, u64, u64)> = (0..8)
        .map(|i| {
            (
                format!("batcher-rbf-desc-replacement-{i}").into_bytes(),
                30_000 + (i as u64 * 1_000),
                12 + i as u64,
            )
        })
        .collect();
    let replacement: Vec<_> = replacement_specs
        .iter()
        .enumerate()
        .map(|(index, (secret, amount, timelock))| {
            let params = env.htlc_params_for_wallets(
                batcher_wallet.as_ref(),
                env.counterparty.as_ref(),
                secret,
                *amount,
                *timelock,
            );
            let address = get_htlc_address(&params, NETWORK).expect("replacement htlc address");
            (format!("desc-replacement-{index}"), params, address)
        })
        .collect();

    for (key, params, address) in &replacement {
        harness
            .submit(env.send_request(key.clone(), address.clone(), params.amount))
            .await;
    }

    tracing::info!(
        new_request_count = replacement.len(),
        "descendant eviction test: waiting for replacement parent"
    );
    let replacement_submission = harness.wait_for_submitted_tx().await;
    let replacement_txid = replacement_submission.txid;
    let replacement_txid_string = replacement_txid.to_string();
    assert_eq!(replacement_submission.replaces, Some(first_txid));
    assert_eq!(replacement_submission.request_keys.len(), 13);
    assert_ne!(replacement_txid, first_txid);
    let mut expected_lineage_id = None;
    for (key, _, _, _) in &initial {
        let row = db.wait_for_wallet_status(&scope, key, "inflight").await;
        assert_eq!(
            row.batch_txid.as_deref(),
            Some(replacement_txid_string.as_str())
        );
        assert_eq!(
            row.txid_history,
            vec![first_txid_string.clone(), replacement_txid_string.clone()]
        );
        assert!(row.chain_anchor.is_none());
        let lineage = row.lineage_id.clone().expect("lineage id for initial row");
        match &expected_lineage_id {
            Some(expected) => assert_eq!(&lineage, expected),
            None => expected_lineage_id = Some(lineage),
        }
    }
    for (key, _, _) in &replacement {
        let row = db.wait_for_wallet_status(&scope, key, "inflight").await;
        assert_eq!(
            row.batch_txid.as_deref(),
            Some(replacement_txid_string.as_str())
        );
        assert_eq!(row.txid_history, vec![replacement_txid_string.clone()]);
        assert!(row.chain_anchor.is_none());
        assert_eq!(row.lineage_id, expected_lineage_id);
    }

    tracing::info!(old = %first_txid, new = %replacement_txid, descendants = child_txids.len(), "descendant eviction test: asserting mempool eviction of old parent and descendants");
    env.wait_for_tx_not_in_mempool(&first_txid).await;
    for child_txid in &child_txids {
        env.wait_for_tx_not_in_mempool(child_txid).await;
    }
    env.wait_for_tx_in_mempool(&replacement_txid).await;

    tracing::info!(%replacement_txid, "descendant eviction test: confirming replacement and checking all HTLC outputs");
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&replacement_txid).await;

    for (key, _, params, address) in &initial {
        let utxos = env.wait_for_confirmed_cover_utxo_count(address, 1).await;
        assert_eq!(utxos[0].value, params.amount);
        let row = db.wait_for_wallet_status(&scope, key, "confirmed").await;
        assert_eq!(
            row.batch_txid.as_deref(),
            Some(replacement_txid_string.as_str())
        );
        assert_eq!(
            row.txid_history,
            vec![first_txid_string.clone(), replacement_txid_string.clone()]
        );
        assert!(row.chain_anchor.is_none());
        assert_eq!(row.lineage_id, expected_lineage_id);
    }
    for (key, params, address) in &replacement {
        let utxos = env.wait_for_confirmed_cover_utxo_count(address, 1).await;
        assert_eq!(utxos[0].value, params.amount);
        let row = db.wait_for_wallet_status(&scope, key, "confirmed").await;
        assert_eq!(
            row.batch_txid.as_deref(),
            Some(replacement_txid_string.as_str())
        );
        assert_eq!(row.txid_history, vec![replacement_txid_string.clone()]);
        assert!(row.chain_anchor.is_none());
        assert_eq!(row.lineage_id, expected_lineage_id);
    }
    tracing::info!(
        scope = %scope,
        first_txid = %first_txid,
        replacement_txid = %replacement_txid,
        child_txids = ?child_txids,
        lineage_id = ?expected_lineage_id,
        "wallet flow descendant eviction"
    );
    for (index, input) in replacement_submission.raw_tx.input.iter().enumerate() {
        tracing::info!(
            index,
            previous_output = %input.previous_output,
            "wallet flow descendant eviction input"
        );
    }
    for (index, output) in replacement_submission.raw_tx.output.iter().enumerate() {
        tracing::info!(
            index,
            value_sats = output.value.to_sat(),
            script_pubkey = %output.script_pubkey,
            "wallet flow descendant eviction output"
        );
    }
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_rbf_replaces_multiple_times_while_dropping_external_descendants() {
    const MULTI_RBF_TEST_TICK_INTERVAL_SECS: u64 = 10;

    // This test repeats the "descendants get attached, parent gets replaced"
    // pattern across several generations to verify the batcher keeps carrying
    // forward its own requests while external descendants are evicted each round.
    tracing::info!("multi-RBF test: setting up initial parent with 3 HTLC initiates");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut harness = env
        .spawn_batcher_paused_with_db_and_tick_interval(
            Arc::clone(&batcher_wallet),
            db,
            MULTI_RBF_TEST_TICK_INTERVAL_SECS,
        )
        .await;

    let initial_specs: Vec<(Vec<u8>, u64, u64)> = (0..3)
        .map(|i| {
            (
                format!("batcher-rbf-multi-initial-{i}").into_bytes(),
                18_000 + (i as u64 * 1_000),
                6 + i as u64,
            )
        })
        .collect();
    let mut all_requests: Vec<_> = initial_specs
        .iter()
        .enumerate()
        .map(|(index, (secret, amount, timelock))| {
            let params = env.htlc_params_for_wallets(
                batcher_wallet.as_ref(),
                env.counterparty.as_ref(),
                secret,
                *amount,
                *timelock,
            );
            let address = get_htlc_address(&params, NETWORK).expect("initial multi htlc address");
            (
                format!("multi-initial-{index}"),
                secret.clone(),
                params,
                address,
            )
        })
        .collect();

    for (key, _, params, address) in &all_requests {
        harness
            .submit(env.send_request(key.clone(), address.clone(), params.amount))
            .await;
    }
    harness.start();

    let mut current_submission = harness.wait_for_submitted_tx().await;
    assert!(current_submission.replaces.is_none());
    assert_eq!(current_submission.request_keys.len(), all_requests.len());
    tracing::info!(txid = %current_submission.txid, request_count = all_requests.len(), "multi-RBF test: first parent accepted");

    let replacement_rounds = [2usize, 3usize, 4usize];
    for (round_index, new_request_count) in replacement_rounds.into_iter().enumerate() {
        let current_txid = current_submission.txid;
        let parent_fee_rate = env
            .bitcoind
            .get_rbf_tx_fee_info(&current_txid.to_string())
            .await
            .expect("current tx fee info")
            .tx_fee_rate;
        let child_fee_rate = (parent_fee_rate * 2.0).max(DEFAULT_FEE_RATE);
        tracing::info!(
            round = round_index + 1,
            %current_txid,
            carried_requests = all_requests.len(),
            new_request_count,
            parent_fee_rate,
            child_fee_rate,
            "multi-RBF test: building descendants and next replacement"
        );

        let mut child_txids = Vec::new();
        for (_, secret, params, address) in all_requests.iter().take(2) {
            let vout = current_submission
                .raw_tx
                .output
                .iter()
                .position(|output| {
                    output.script_pubkey == address.script_pubkey()
                        && output.value.to_sat() == params.amount
                })
                .expect("htlc output in current submission") as u32;
            let utxo = CoverUtxo {
                outpoint: bitcoin::OutPoint {
                    txid: current_txid,
                    vout,
                },
                value: params.amount,
                script_pubkey: address.script_pubkey(),
            };
            let redeem_tx = env
                .build_with_fee_builder(
                    Arc::clone(&env.counterparty),
                    BitcoinTxAdaptorParams {
                        sacps: vec![],
                        spends: vec![env.redeem_spend(params, secret, vec![utxo])],
                        sends: vec![],
                        fee_rate: child_fee_rate,
                    },
                )
                .await;
            let child_txid = env
                .broadcast_to_mempool("multi-round descendant redeem", &redeem_tx)
                .await;
            env.wait_for_tx_in_mempool(&child_txid).await;
            child_txids.push(child_txid);
        }

        let new_specs: Vec<(Vec<u8>, u64, u64)> = (0..new_request_count)
            .map(|i| {
                (
                    format!("batcher-rbf-multi-round-{round_index}-{i}").into_bytes(),
                    26_000 + ((round_index as u64 * 10 + i as u64) * 1_000),
                    10 + round_index as u64 + i as u64,
                )
            })
            .collect();
        let new_requests: Vec<_> = new_specs
            .iter()
            .enumerate()
            .map(|(index, (secret, amount, timelock))| {
                let params = env.htlc_params_for_wallets(
                    batcher_wallet.as_ref(),
                    env.counterparty.as_ref(),
                    secret,
                    *amount,
                    *timelock,
                );
                let address =
                    get_htlc_address(&params, NETWORK).expect("replacement round htlc address");
                (
                    format!("multi-round-{round_index}-{index}"),
                    secret.clone(),
                    params,
                    address,
                )
            })
            .collect();

        for (key, _, params, address) in &new_requests {
            harness
                .submit(env.send_request(key.clone(), address.clone(), params.amount))
                .await;
        }
        all_requests.extend(new_requests);

        let replacement_submission = harness.wait_for_submitted_tx().await;
        assert_eq!(replacement_submission.replaces, Some(current_txid));
        assert_eq!(
            replacement_submission.request_keys.len(),
            all_requests.len()
        );
        assert_ne!(replacement_submission.txid, current_txid);
        tracing::info!(
            round = round_index + 1,
            old = %current_txid,
            new = %replacement_submission.txid,
            descendants = child_txids.len(),
            carried_requests = all_requests.len(),
            "multi-RBF test: replacement accepted, checking evictions"
        );

        env.wait_for_tx_not_in_mempool(&current_txid).await;
        for child_txid in &child_txids {
            env.wait_for_tx_not_in_mempool(child_txid).await;
        }
        env.wait_for_tx_in_mempool(&replacement_submission.txid)
            .await;

        current_submission = replacement_submission;
    }

    tracing::info!(%current_submission.txid, total_requests = all_requests.len(), "multi-RBF test: confirming final replacement");
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&current_submission.txid).await;

    for (_, _, params, address) in &all_requests {
        let utxos = env.wait_for_confirmed_cover_utxo_count(address, 1).await;
        assert_eq!(utxos[0].value, params.amount);
    }
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_rbf_drops_ten_long_descendant_chains_under_policy_limit() {
    const TEN_CHAIN_TEST_TICK_INTERVAL_SECS: u64 = 10;

    // Build 10 independent descendant chains under one parent tx, staying
    // below policy limits, then replace the parent and verify all descendants
    // are evicted together.
    tracing::info!("ten-chain test: setting up 10 initial initiates");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut harness = env
        .spawn_batcher_paused_with_db_and_tick_interval(
            Arc::clone(&batcher_wallet),
            db,
            TEN_CHAIN_TEST_TICK_INTERVAL_SECS,
        )
        .await;

    let initial_specs: Vec<(Vec<u8>, u64, u64)> = (0..10)
        .map(|i| {
            (
                format!("batcher-rbf-chain-init-{i}").into_bytes(),
                70_000 + (i as u64 * 2_000),
                6 + i as u64,
            )
        })
        .collect();
    let initial: Vec<_> = initial_specs
        .iter()
        .enumerate()
        .map(|(index, (secret, amount, timelock))| {
            let params = env.htlc_params_for_wallets(
                batcher_wallet.as_ref(),
                env.counterparty.as_ref(),
                secret,
                *amount,
                *timelock,
            );
            let address = get_htlc_address(&params, NETWORK).expect("chain initial htlc address");
            (
                format!("chain-initial-{index}"),
                secret.clone(),
                params,
                address,
            )
        })
        .collect();

    for (key, _, params, address) in &initial {
        harness
            .submit(env.send_request(key.clone(), address.clone(), params.amount))
            .await;
    }
    harness.start();

    let first_submission = harness.wait_for_submitted_tx().await;
    let first_txid = first_submission.txid;
    assert!(first_submission.replaces.is_none());
    assert_eq!(first_submission.request_keys.len(), 10);
    tracing::info!(%first_txid, request_count = 10, "ten-chain test: first parent accepted");

    let parent_fee_rate = env
        .bitcoind
        .get_rbf_tx_fee_info(&first_txid.to_string())
        .await
        .expect("chain parent fee info")
        .tx_fee_rate;
    let child_fee_rate = (parent_fee_rate * 2.0).max(DEFAULT_FEE_RATE);

    let mut descendant_txids = Vec::new();
    tracing::info!(
        parent_fee_rate,
        child_fee_rate,
        "ten-chain test: building 10 descendant chains"
    );
    for (_, secret, params, address) in &initial {
        let vout = first_submission
            .raw_tx
            .output
            .iter()
            .position(|output| {
                output.script_pubkey == address.script_pubkey()
                    && output.value.to_sat() == params.amount
            })
            .expect("initial htlc output in first submission") as u32;
        let utxo = CoverUtxo {
            outpoint: bitcoin::OutPoint {
                txid: first_txid,
                vout,
            },
            value: params.amount,
            script_pubkey: address.script_pubkey(),
        };

        let redeem_tx = env
            .build_with_fee_builder(
                Arc::clone(&env.counterparty),
                BitcoinTxAdaptorParams {
                    sacps: vec![],
                    spends: vec![env.redeem_spend(params, secret, vec![utxo])],
                    sends: vec![],
                    fee_rate: child_fee_rate,
                },
            )
            .await;
        let redeem_txid = env.broadcast_to_mempool("chain redeem", &redeem_tx).await;
        env.wait_for_tx_in_mempool(&redeem_txid).await;
        descendant_txids.push(redeem_txid);

        let mut current_wallet = Arc::clone(&env.counterparty);
        let mut current_utxo =
            env.address_output_utxo(&redeem_tx, redeem_txid, current_wallet.address());

        for hop in 0..1 {
            let next_wallet = BitcoinTestEnv::random_wallet();
            let send_value = current_utxo.value.saturating_sub(2_000);
            let transfer_tx = env
                .build_send_from_wallet_utxo(
                    Arc::clone(&current_wallet),
                    current_utxo.clone(),
                    next_wallet.address().clone(),
                    send_value,
                    child_fee_rate,
                )
                .await;
            let transfer_txid = env
                .broadcast_to_mempool("chain transfer descendant", &transfer_tx)
                .await;
            env.wait_for_tx_in_mempool(&transfer_txid).await;
            descendant_txids.push(transfer_txid);

            current_utxo =
                env.address_output_utxo(&transfer_tx, transfer_txid, next_wallet.address());
            current_wallet = next_wallet;

            assert!(
                current_utxo.value > 10_000,
                "chain value must remain spendable after hop {hop}"
            );
        }
    }

    let descendants = env
        .bitcoind
        .get_mempool_descendants(&first_txid.to_string())
        .await
        .expect("parent descendants");
    assert_eq!(descendants.len(), 20);
    assert!(
        descendants.len() < 100,
        "descendant package must stay under policy cap"
    );
    tracing::info!(%first_txid, descendants = descendants.len(), "ten-chain test: descendant package established");

    let replacement_specs: Vec<(Vec<u8>, u64, u64)> = (0..10)
        .map(|i| {
            (
                format!("batcher-rbf-chain-replacement-{i}").into_bytes(),
                40_000 + (i as u64 * 1_000),
                20 + i as u64,
            )
        })
        .collect();
    let replacement: Vec<_> = replacement_specs
        .iter()
        .enumerate()
        .map(|(index, (secret, amount, timelock))| {
            let params = env.htlc_params_for_wallets(
                batcher_wallet.as_ref(),
                env.counterparty.as_ref(),
                secret,
                *amount,
                *timelock,
            );
            let address =
                get_htlc_address(&params, NETWORK).expect("chain replacement htlc address");
            (format!("chain-replacement-{index}"), params, address)
        })
        .collect();

    for (key, params, address) in &replacement {
        harness
            .submit(env.send_request(key.clone(), address.clone(), params.amount))
            .await;
    }

    tracing::info!(
        new_request_count = replacement.len(),
        "ten-chain test: waiting for replacement parent"
    );
    let replacement_submission = harness.wait_for_submitted_tx().await;
    let replacement_txid = replacement_submission.txid;
    assert_eq!(replacement_submission.replaces, Some(first_txid));
    assert_eq!(replacement_submission.request_keys.len(), 20);
    assert_ne!(replacement_txid, first_txid);

    tracing::info!(old = %first_txid, new = %replacement_txid, descendants = descendant_txids.len(), "ten-chain test: checking parent and descendant eviction");
    env.wait_for_tx_not_in_mempool(&first_txid).await;
    for txid in &descendant_txids {
        env.wait_for_tx_not_in_mempool(txid).await;
    }
    env.wait_for_tx_in_mempool(&replacement_txid).await;

    tracing::info!(%replacement_txid, "ten-chain test: confirming replacement and asserting final HTLC outputs");
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&replacement_txid).await;

    for (_, _, params, address) in &initial {
        let utxos = env.wait_for_confirmed_cover_utxo_count(address, 1).await;
        assert_eq!(utxos[0].value, params.amount);
    }
    for (_, params, address) in &replacement {
        let utxos = env.wait_for_confirmed_cover_utxo_count(address, 1).await;
        assert_eq!(utxos[0].value, params.amount);
    }
}

#[serial]
#[tokio::test]
async fn bitcoin_batcher_rbf_replaces_thirty_times_with_seeded_random_descendants() {
    const THIRTY_ROUND_TEST_TICK_INTERVAL_SECS: u64 = 3;

    // Long-running stress case: 30 sequential replacements. Each round
    // randomly chooses whether to attach one external descendant to the newest
    // outputs, then replaces the parent again with additional initiates.
    tracing::info!("30-round stress test: initializing seeded random replacement scenario");
    let env = BitcoinTestEnv::new().await;
    let batcher_wallet = env.funded_random_wallet().await;
    let db = env.new_test_db().await;
    let mut harness = env
        .spawn_batcher_paused_with_db_and_tick_interval(
            Arc::clone(&batcher_wallet),
            db,
            THIRTY_ROUND_TEST_TICK_INTERVAL_SECS,
        )
        .await;
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xB47C_4E52_3015_0001);

    let initial_specs: Vec<(Vec<u8>, u64, u64)> = (0..2)
        .map(|i| {
            (
                format!("batcher-rbf-30-initial-{i}").into_bytes(),
                18_000 + (i as u64 * 1_000),
                6 + i as u64,
            )
        })
        .collect();
    let mut all_requests: Vec<_> = initial_specs
        .iter()
        .enumerate()
        .map(|(index, (secret, amount, timelock))| {
            let params = env.htlc_params_for_wallets(
                batcher_wallet.as_ref(),
                env.counterparty.as_ref(),
                secret,
                *amount,
                *timelock,
            );
            let address = get_htlc_address(&params, NETWORK).expect("initial stress htlc address");
            (
                format!("stress-initial-{index}"),
                secret.clone(),
                params,
                address,
            )
        })
        .collect();

    for (key, _, params, address) in &all_requests {
        harness
            .submit(env.send_request(key.clone(), address.clone(), params.amount))
            .await;
    }
    harness.start();

    let mut current_submission = harness.wait_for_submitted_tx().await;
    assert!(current_submission.replaces.is_none());
    assert_eq!(current_submission.request_keys.len(), all_requests.len());
    let mut current_descendant_candidates: Vec<usize> = (0..all_requests.len()).collect();
    tracing::info!(txid = %current_submission.txid, initial_requests = all_requests.len(), "30-round stress test: first parent accepted");

    let mut rounds_with_descendants = 0usize;
    let mut rounds_without_descendants = 0usize;

    for round in 0..30usize {
        let current_txid = current_submission.txid;
        env.wait_for_tx_in_mempool(&current_txid).await;
        let parent_fee_rate = env
            .bitcoind
            .get_rbf_tx_fee_info(&current_txid.to_string())
            .await
            .expect("current stress tx fee info")
            .tx_fee_rate;
        let child_fee_rate = (parent_fee_rate * 2.0).clamp(DEFAULT_FEE_RATE, 5.0);

        let eligible_count = all_requests
            .iter()
            .enumerate()
            .filter(|(index, (_, _, params, address))| {
                current_descendant_candidates.contains(index)
                    && current_submission.raw_tx.output.iter().any(|output| {
                        output.script_pubkey == address.script_pubkey()
                            && output.value.to_sat() == params.amount
                    })
            })
            .count();
        let create_descendants = rng.random_bool(0.65) && eligible_count > 0;
        let descendant_target_count = if create_descendants {
            rounds_with_descendants += 1;
            1
        } else {
            rounds_without_descendants += 1;
            0
        };
        tracing::info!(
            round = round + 1,
            %current_txid,
            carried_requests = all_requests.len(),
            eligible_descendants = eligible_count,
            create_descendants,
            descendant_target_count,
            parent_fee_rate,
            child_fee_rate,
            "30-round stress test: preparing next replacement round"
        );

        let mut descendant_txids = Vec::new();
        if descendant_target_count > 0 {
            let mut candidate_indices: Vec<usize> = all_requests
                .iter()
                .enumerate()
                .filter_map(|(index, (_, _, params, address))| {
                    (current_descendant_candidates.contains(&index)
                        && current_submission.raw_tx.output.iter().any(|output| {
                            output.script_pubkey == address.script_pubkey()
                                && output.value.to_sat() == params.amount
                        }))
                    .then_some(index)
                })
                .collect();
            candidate_indices.shuffle(&mut rng);

            for request_index in candidate_indices.into_iter().take(descendant_target_count) {
                let (_, secret, params, address) = &all_requests[request_index];
                let vout = current_submission
                    .raw_tx
                    .output
                    .iter()
                    .position(|output| {
                        output.script_pubkey == address.script_pubkey()
                            && output.value.to_sat() == params.amount
                    })
                    .expect("stress htlc output in current submission")
                    as u32;
                let utxo = CoverUtxo {
                    outpoint: bitcoin::OutPoint {
                        txid: current_txid,
                        vout,
                    },
                    value: params.amount,
                    script_pubkey: address.script_pubkey(),
                };
                let redeem_tx = env
                    .build_with_fee_builder(
                        Arc::clone(&env.counterparty),
                        BitcoinTxAdaptorParams {
                            sacps: vec![],
                            spends: vec![env.redeem_spend(params, secret, vec![utxo])],
                            sends: vec![],
                            fee_rate: child_fee_rate,
                        },
                    )
                    .await;
                let descendant_txid = env
                    .broadcast_to_mempool("stress descendant redeem", &redeem_tx)
                    .await;
                env.wait_for_tx_in_mempool(&descendant_txid).await;
                descendant_txids.push(descendant_txid);
            }
        }

        let new_request_count = rng.random_range(1..=2usize);
        let new_specs: Vec<(Vec<u8>, u64, u64)> = (0..new_request_count)
            .map(|i| {
                (
                    format!("batcher-rbf-30-round-{round}-{i}").into_bytes(),
                    22_000 + ((round as u64 * 10 + i as u64) * 500),
                    12 + round as u64 + i as u64,
                )
            })
            .collect();
        let new_requests: Vec<_> = new_specs
            .iter()
            .enumerate()
            .map(|(index, (secret, amount, timelock))| {
                let params = env.htlc_params_for_wallets(
                    batcher_wallet.as_ref(),
                    env.counterparty.as_ref(),
                    secret,
                    *amount,
                    *timelock,
                );
                let address =
                    get_htlc_address(&params, NETWORK).expect("stress replacement htlc address");
                (
                    format!("stress-round-{round}-{index}"),
                    secret.clone(),
                    params,
                    address,
                )
            })
            .collect();
        let first_new_request_index = all_requests.len();

        for (key, _, params, address) in &new_requests {
            harness
                .submit(env.send_request(key.clone(), address.clone(), params.amount))
                .await;
        }
        all_requests.extend(new_requests);

        let replacement_submission = harness.wait_for_submitted_tx().await;
        assert_eq!(replacement_submission.replaces, Some(current_txid));
        assert_eq!(
            replacement_submission.request_keys.len(),
            all_requests.len()
        );
        assert_ne!(replacement_submission.txid, current_txid);
        tracing::info!(
            round = round + 1,
            old = %current_txid,
            new = %replacement_submission.txid,
            descendants = descendant_txids.len(),
            total_requests = all_requests.len(),
            "30-round stress test: replacement accepted, checking evictions"
        );

        env.wait_for_tx_not_in_mempool(&current_txid).await;
        for descendant_txid in &descendant_txids {
            env.wait_for_tx_not_in_mempool(descendant_txid).await;
        }
        env.wait_for_tx_in_mempool(&replacement_submission.txid)
            .await;

        current_submission = replacement_submission;
        current_descendant_candidates = (first_new_request_index..all_requests.len()).collect();
    }

    assert!(
        rounds_with_descendants > 0,
        "seeded run must include descendant rounds"
    );
    assert!(
        rounds_without_descendants > 0,
        "seeded run must include descendant-free rounds"
    );
    tracing::info!(
        rounds_with_descendants,
        rounds_without_descendants,
        final_requests = all_requests.len(),
        final_txid = %current_submission.txid,
        "30-round stress test: confirming final replacement"
    );

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&current_submission.txid).await;

    for (_, _, params, address) in &all_requests {
        let utxos = env.wait_for_confirmed_cover_utxo_count(address, 1).await;
        assert_eq!(utxos[0].value, params.amount);
    }
}
