#![cfg(test)]

use std::sync::Arc;
use std::sync::OnceLock;

use bitcoin::hashes::Hash;
use bitcoin::{Address, Network, Txid};
use tars::orderbook::test_utils::default_matched_order;
use tars::primitives::HTLCAction;
use tokio::sync::Mutex;

use btc_executor::infrastructure::chain::bitcoin::BitcoinActionExecutor;
use btc_executor::infrastructure::chain::bitcoin::primitives::{HTLCParams, get_htlc_address};
use btc_executor::infrastructure::keys::BitcoinWallet;

use super::common::{BitcoinTestEnv, DEFAULT_FEE_RATE, NETWORK, TestDatabase};

const WAIT_RETRIES: usize = 40;
const WAIT_DELAY_MS: u64 = 500;

fn regtest_lock() -> &'static Mutex<()> {
    static REGTEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    REGTEST_LOCK.get_or_init(|| Mutex::new(()))
}

fn assert_request_keys(
    submitted: &super::common::SubmittedBatchTx,
    expected: &[String],
) {
    let actual = submitted
        .request_keys
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    let expected = expected
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(actual, expected);
}

fn assert_has_output(
    tx: &bitcoin::Transaction,
    address: &Address,
    amount: u64,
) {
    assert!(
        tx.output.iter().any(|output| {
            output.script_pubkey == address.script_pubkey()
                && output.value.to_sat() == amount
        }),
        "missing output {amount} sats to {address}"
    );
}

fn htlc_params(
    initiator_wallet: &BitcoinWallet,
    redeemer_wallet: &BitcoinWallet,
    secret: &[u8],
    amount: u64,
    timelock: u16,
) -> HTLCParams {
    let mut secret_hash = [0u8; 32];
    secret_hash.copy_from_slice(&bitcoin::hashes::sha256::Hash::hash(secret).to_byte_array());

    HTLCParams {
        initiator_pubkey: *initiator_wallet.x_only_pubkey(),
        redeemer_pubkey: *redeemer_wallet.x_only_pubkey(),
        amount,
        secret_hash,
        timelock: u64::from(timelock),
    }
}

async fn wait_for_address_utxos(env: &BitcoinTestEnv, address: &Address, min_count: usize) {
    for _ in 0..WAIT_RETRIES {
        let utxos = env
            .electrs
            .get_address_utxos(&address.to_string())
            .await
            .expect("address utxos");
        if utxos.len() >= min_count {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(WAIT_DELAY_MS)).await;
    }

    panic!("address {address} did not reach {min_count} utxos");
}

async fn wait_for_address_empty(env: &BitcoinTestEnv, address: &Address) {
    for _ in 0..WAIT_RETRIES {
        let utxos = env
            .electrs
            .get_address_utxos(&address.to_string())
            .await
            .expect("address utxos");
        if utxos.is_empty() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(WAIT_DELAY_MS)).await;
    }

    panic!("address {address} still has spendable utxos");
}

fn make_order_and_swap(
    params: &HTLCParams,
    htlc_address: &Address,
    initiate_txid: Txid,
    recipient: Option<&Address>,
    instant_refund_tx_hex: Option<String>,
) -> (tars::orderbook::primitives::MatchedOrderVerbose, tars::orderbook::primitives::SingleSwap) {
    let mut order = default_matched_order();
    order.create_order.source_chain = "bitcoin_regtest".to_string();
    order.create_order.destination_chain = "bitcoin_regtest".to_string();
    order.create_order.source_amount = params.amount.into();
    order.create_order.destination_amount = params.amount.into();
    order.create_order.timelock = params.timelock as i32;
    order.create_order.secret_hash = hex::encode(params.secret_hash);
    order.create_order.additional_data.bitcoin_optional_recipient =
        recipient.map(ToString::to_string);
    order.create_order.additional_data.instant_refund_tx_bytes = instant_refund_tx_hex;

    let mut swap = order.destination_swap.clone();
    swap.chain = "bitcoin_regtest".to_string();
    swap.asset = "primary".to_string();
    swap.swap_id = format!("btc-executor-{}", initiate_txid);
    swap.htlc_address = Some(htlc_address.to_string());
    swap.initiator = params.initiator_pubkey.to_string();
    swap.redeemer = params.redeemer_pubkey.to_string();
    swap.timelock = params.timelock as i32;
    swap.amount = params.amount.into();
    swap.secret_hash = hex::encode(params.secret_hash);
    swap.initiate_tx_hash = tars::orderbook::primitives::MaybeString::new(format!(
        "{}:1",
        initiate_txid
    ));

    (order, swap)
}

#[ignore = "requires local bitcoind + electrs regtest services on default ports"]
#[tokio::test]
async fn redeem_action_executor_executes_on_regtest() {
    let _guard = regtest_lock().lock().await;
    let env = BitcoinTestEnv::new().await;
    let executor_wallet = env.funded_random_wallet().await;
    let initiator_wallet = env.funded_random_wallet().await;
    let db = Arc::new(TestDatabase::new().await);
    let harness = env
        .spawn_batcher_with_db(Arc::clone(&executor_wallet), Arc::clone(&db))
        .await;
    let action_executor = BitcoinActionExecutor::new(
        Arc::clone(&executor_wallet),
        harness.submitter(),
        Arc::clone(&env.electrs),
        Network::Regtest,
    );

    let secret = b"btc-executor-redeem-secret";
    let params = htlc_params(
        initiator_wallet.as_ref(),
        executor_wallet.as_ref(),
        secret,
        50_000,
        6,
    );
    let htlc_address = get_htlc_address(&params, NETWORK).expect("htlc address");
    let (initiate_txid, _) = env
        .fund_htlc(
            Arc::clone(&initiator_wallet),
            &params,
            DEFAULT_FEE_RATE,
        )
        .await;
    wait_for_address_utxos(&env, &htlc_address, 1).await;

    let (order, swap) = make_order_and_swap(&params, &htlc_address, initiate_txid, None, None);
    action_executor
        .execute_action(
            &order,
            &HTLCAction::Redeem {
                secret: secret.to_vec().into(),
            },
            &swap,
        )
        .await
        .expect("execute redeem");

    let mut harness = harness;
    let submitted = harness.wait_for_submitted_tx().await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&submitted.txid).await;
    wait_for_address_empty(&env, &htlc_address).await;
}

#[ignore = "requires local bitcoind + electrs regtest services on default ports"]
#[tokio::test]
async fn refund_action_executor_executes_on_regtest() {
    let _guard = regtest_lock().lock().await;
    let env = BitcoinTestEnv::new().await;
    let executor_wallet = env.funded_random_wallet().await;
    let redeemer_wallet = env.funded_random_wallet().await;
    let db = Arc::new(TestDatabase::new().await);
    let harness = env
        .spawn_batcher_with_db(Arc::clone(&executor_wallet), Arc::clone(&db))
        .await;
    let action_executor = BitcoinActionExecutor::new(
        Arc::clone(&executor_wallet),
        harness.submitter(),
        Arc::clone(&env.electrs),
        Network::Regtest,
    );

    let params = htlc_params(
        executor_wallet.as_ref(),
        redeemer_wallet.as_ref(),
        b"btc-executor-refund-secret",
        55_000,
        2,
    );
    let htlc_address = get_htlc_address(&params, NETWORK).expect("htlc address");
    let (initiate_txid, _) = env
        .fund_htlc(
            Arc::clone(&executor_wallet),
            &params,
            DEFAULT_FEE_RATE,
        )
        .await;
    wait_for_address_utxos(&env, &htlc_address, 1).await;
    env.mine_blocks(params.timelock).await;

    let (order, swap) = make_order_and_swap(&params, &htlc_address, initiate_txid, None, None);
    action_executor
        .execute_action(&order, &HTLCAction::Refund, &swap)
        .await
        .expect("execute refund");

    let mut harness = harness;
    let submitted = harness.wait_for_submitted_tx().await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&submitted.txid).await;
    wait_for_address_empty(&env, &htlc_address).await;
}

#[ignore = "requires local bitcoind + electrs regtest services on default ports"]
#[tokio::test]
async fn instant_refund_action_executor_executes_on_regtest() {
    let _guard = regtest_lock().lock().await;
    let env = BitcoinTestEnv::new().await;
    let executor_wallet = env.funded_random_wallet().await;
    let initiator_wallet = env.funded_random_wallet().await;
    let recipient_wallet = BitcoinTestEnv::random_wallet();
    let db = Arc::new(TestDatabase::new().await);
    let harness = env
        .spawn_batcher_with_db(Arc::clone(&executor_wallet), Arc::clone(&db))
        .await;
    let action_executor = BitcoinActionExecutor::new(
        Arc::clone(&executor_wallet),
        harness.submitter(),
        Arc::clone(&env.electrs),
        Network::Regtest,
    );

    let params = htlc_params(
        initiator_wallet.as_ref(),
        executor_wallet.as_ref(),
        b"btc-executor-instant-refund-secret",
        48_000,
        6,
    );
    let htlc_address = get_htlc_address(&params, NETWORK).expect("htlc address");
    let (initiate_txid, utxos) = env
        .fund_htlc(
            Arc::clone(&initiator_wallet),
            &params,
            DEFAULT_FEE_RATE,
        )
        .await;
    wait_for_address_utxos(&env, &htlc_address, 1).await;

    let instant_refund_tx_hex = env.initiator_signed_instant_refund_tx_hex(
        &params,
        utxos,
        recipient_wallet.address().clone(),
        initiator_wallet.as_ref(),
    );
    let (order, swap) = make_order_and_swap(
        &params,
        &htlc_address,
        initiate_txid,
        Some(recipient_wallet.address()),
        Some(instant_refund_tx_hex),
    );
    action_executor
        .execute_action(&order, &HTLCAction::InstantRefund, &swap)
        .await
        .expect("execute instant refund");

    let mut harness = harness;
    let submitted = harness.wait_for_submitted_tx().await;
    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&submitted.txid).await;
    wait_for_address_empty(&env, &htlc_address).await;
}

#[ignore = "requires local bitcoind + electrs regtest services on default ports"]
#[tokio::test]
async fn instant_refund_action_executor_batches_multiple_requests_on_regtest() {
    let _guard = regtest_lock().lock().await;
    let env = BitcoinTestEnv::new().await;
    let executor_wallet = env.funded_random_wallet().await;
    let initiator_wallet = env.funded_random_wallet().await;
    let recipient_wallet_a = BitcoinTestEnv::random_wallet();
    let recipient_wallet_b = BitcoinTestEnv::random_wallet();
    let db = Arc::new(TestDatabase::new().await);
    let mut harness = env
        .spawn_batcher_paused_with_db_and_tick_interval(Arc::clone(&executor_wallet), Arc::clone(&db), 30)
        .await;
    let action_executor = BitcoinActionExecutor::new(
        Arc::clone(&executor_wallet),
        harness.submitter(),
        Arc::clone(&env.electrs),
        Network::Regtest,
    );

    let params_a = htlc_params(
        initiator_wallet.as_ref(),
        executor_wallet.as_ref(),
        b"btc-executor-instant-refund-batch-secret-a",
        48_000,
        6,
    );
    let htlc_address_a = get_htlc_address(&params_a, NETWORK).expect("htlc address a");
    let (initiate_txid_a, utxos_a) = env
        .fund_htlc(
            Arc::clone(&initiator_wallet),
            &params_a,
            DEFAULT_FEE_RATE,
        )
        .await;

    let params_b = htlc_params(
        initiator_wallet.as_ref(),
        executor_wallet.as_ref(),
        b"btc-executor-instant-refund-batch-secret-b",
        52_000,
        6,
    );
    let htlc_address_b = get_htlc_address(&params_b, NETWORK).expect("htlc address b");
    let (initiate_txid_b, utxos_b) = env
        .fund_htlc(
            Arc::clone(&initiator_wallet),
            &params_b,
            DEFAULT_FEE_RATE,
        )
        .await;

    wait_for_address_utxos(&env, &htlc_address_a, 1).await;
    wait_for_address_utxos(&env, &htlc_address_b, 1).await;

    let instant_refund_tx_hex_a = env.initiator_signed_instant_refund_tx_hex(
        &params_a,
        utxos_a.clone(),
        recipient_wallet_a.address().clone(),
        initiator_wallet.as_ref(),
    );
    let instant_refund_tx_hex_b = env.initiator_signed_instant_refund_tx_hex(
        &params_b,
        utxos_b.clone(),
        recipient_wallet_b.address().clone(),
        initiator_wallet.as_ref(),
    );

    let (order_a, swap_a) = make_order_and_swap(
        &params_a,
        &htlc_address_a,
        initiate_txid_a,
        Some(recipient_wallet_a.address()),
        Some(instant_refund_tx_hex_a),
    );
    let (order_b, swap_b) = make_order_and_swap(
        &params_b,
        &htlc_address_b,
        initiate_txid_b,
        Some(recipient_wallet_b.address()),
        Some(instant_refund_tx_hex_b),
    );

    let request_count_a = action_executor
        .execute_action(&order_a, &HTLCAction::InstantRefund, &swap_a)
        .await
        .expect("execute instant refund a");
    let request_count_b = action_executor
        .execute_action(&order_b, &HTLCAction::InstantRefund, &swap_b)
        .await
        .expect("execute instant refund b");
    assert_eq!(request_count_a, 1);
    assert_eq!(request_count_b, 1);

    let expected_request_keys = vec![
        format!(
            "instant_refund:{}:{}:{}",
            swap_a.swap_id,
            utxos_a[0].outpoint.txid,
            utxos_a[0].outpoint.vout
        ),
        format!(
            "instant_refund:{}:{}:{}",
            swap_b.swap_id,
            utxos_b[0].outpoint.txid,
            utxos_b[0].outpoint.vout
        ),
    ];

    harness.start();
    let submitted = harness.wait_for_submitted_tx().await;
    assert_request_keys(&submitted, &expected_request_keys);
    assert_eq!(submitted.request_keys.len(), 2);
    assert_has_output(&submitted.raw_tx, recipient_wallet_a.address(), params_a.amount);
    assert_has_output(&submitted.raw_tx, recipient_wallet_b.address(), params_b.amount);

    env.mine_blocks(1).await;
    env.wait_for_confirmed_tx(&submitted.txid).await;
    wait_for_address_empty(&env, &htlc_address_a).await;
    wait_for_address_empty(&env, &htlc_address_b).await;
}
