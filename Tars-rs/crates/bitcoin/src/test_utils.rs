use crate::{
    batcher::{batcher::BitcoinTxBatcher, primitives::SpendRequest},
    fund_btc, get_htlc_address,
    htlc::htlc::{get_control_block, get_htlc_leaf_script, BitcoinHTLC},
    ArcFeeRateEstimator, ArcIndexer, BitcoinIndexerClient, FeeLevel, FixedFeeRateEstimator,
    HTLCLeaf, HTLCParams, Utxo, UtxoStatus,
};
use bigdecimal::BigDecimal;
use bitcoin::{
    hashes::Hash,
    key::{Keypair, Secp256k1},
    Address, Amount, Network, OutPoint, ScriptBuf, Sequence, TxIn, TxOut, Txid, Witness,
    XOnlyPublicKey,
};
use chrono::Utc;
use eyre::{eyre, Result};
use orderbook::{
    primitives::{AdditionalData, MatchedOrderVerbose, MaybeString, SingleSwap, SwapChain},
    test_utils::{create_test_matched_order, TestMatchedOrderConfig, TestSwapConfig},
    traits::Orderbook,
};
use primitives::HTLCVersion;
use std::{str::FromStr, sync::Arc};
use utils::gen_secret;
/// Test amount for Bitcoin transactions (50,000 satoshis)
pub const TEST_AMOUNT: u64 = 50000;

/// Test timelock for HTLC (144 blocks)
pub const TEST_TIMELOCK: u64 = 144;

/// Network used for tests (Regtest network)
pub const TEST_NETWORK: Network = Network::Regtest;

/// Test fee for Bitcoin transactions (1000 satoshis)
pub const TEST_FEE: u64 = 1000;

/// URL of the Bitcoin indexer for test use
pub const TEST_INDEXER_URL: &str = "http://localhost:30000";

/// Test fee rate for Bitcoin transactions (1 sat/vbyte)
pub const TEST_FEE_RATE: f64 = 1.0;

/// Generates a random Bitcoin keypair for testing
///
/// # Returns
/// A new `Keypair` generated using a random Secp256k1 instance.
pub fn generate_bitcoin_random_keypair() -> Keypair {
    let secp = Secp256k1::new();
    Keypair::new(&secp, &mut rand::thread_rng())
}

/// Returns test parameters for HTLC, including initiator and redeemer keys, secret hash, amount, and timelock.
///
/// # Arguments
/// * `initiator_pubkey` - The initiator's public key for the HTLC.
/// * `redeemer_pubkey` - The redeemer's public key for the HTLC.
/// * `secret_hash` - The hash of the secret used in the HTLC.
///
/// # Returns
/// `HTLCParams` with the test values.
pub fn get_test_htlc_params(
    initiator_pubkey: &XOnlyPublicKey,
    redeemer_pubkey: &XOnlyPublicKey,
    secret_hash: [u8; 32],
) -> HTLCParams {
    HTLCParams {
        initiator_pubkey: initiator_pubkey.clone(),
        redeemer_pubkey: redeemer_pubkey.clone(),
        amount: TEST_AMOUNT,
        secret_hash,
        timelock: TEST_TIMELOCK,
    }
}

/// Returns a new Bitcoin indexer client for test purposes.
///
/// # Returns
/// A wrapped `ArcIndexer` pointing to the test Bitcoin indexer.
pub fn get_test_bitcoin_indexer() -> Result<ArcIndexer> {
    Ok(Arc::new(BitcoinIndexerClient::new(
        TEST_INDEXER_URL.to_string(),
        None,
    )?))
}

/// Returns a fee rate estimator with a fixed fee rate for testing.
///
/// # Returns
/// A wrapped `ArcFeeRateEstimator` using a fixed fee rate.
pub fn get_test_fee_rate_estimator() -> Result<ArcFeeRateEstimator> {
    Ok(Arc::new(FixedFeeRateEstimator::new(TEST_FEE_RATE)))
}

/// Returns a dummy sighash value (32-byte array filled with 42s).
///
/// # Returns
/// A dummy `u8` array representing the sighash.
pub fn get_dummy_sighash() -> [u8; 32] {
    [42u8; 32]
}

/// Returns a dummy UTXO for testing purposes.
///
/// # Returns
/// A new `Utxo` with predefined txid, vout, and value.
pub fn get_dummy_utxo() -> Utxo {
    Utxo {
        txid: Txid::all_zeros(),
        vout: 0,
        value: 10u64.pow(8),
        status: UtxoStatus {
            confirmed: true,
            block_height: Some(100),
        },
    }
}

/// Returns a dummy `TxOut` for testing purposes.
///
/// # Returns
/// A new `TxOut` with predefined value and script_pubkey.
pub fn get_dummy_txout() -> TxOut {
    TxOut {
        value: Amount::from_sat(10u64.pow(8)),
        script_pubkey: ScriptBuf::new(),
    }
}

/// Returns a dummy `TxIn` for testing purposes.
///
/// # Returns
/// A new `TxIn` with predefined previous output and witness.
pub fn get_dummy_txin() -> TxIn {
    TxIn {
        previous_output: OutPoint {
            txid: Txid::all_zeros(),
            vout: 0,
        },
        script_sig: ScriptBuf::new(),
        sequence: Sequence(0xFFFFFFFF),
        witness: Witness::new(),
    }
}

/// Generates a test `SpendRequest` for HTLC transaction construction.
///
/// # Returns
/// A `SpendRequest` configured with test HTLC parameters, witness, and other mock data.
pub async fn get_test_spend_request() -> Result<SpendRequest> {
    let indexer = get_test_bitcoin_indexer()?;

    let initiator_keypair = generate_bitcoin_random_keypair();
    let initiator_pubkey = initiator_keypair.x_only_public_key().0;

    let redeemer_keypair = generate_bitcoin_random_keypair();
    let redeemer_pubkey = redeemer_keypair.x_only_public_key().0;

    let (secret, secret_hash) = gen_secret();

    let htlc_params = get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());
    let htlc_address = get_htlc_address(&htlc_params, TEST_NETWORK)?;

    fund_btc(&htlc_address, &indexer).await?;

    let utxos = indexer.get_utxos(&htlc_address).await?;

    let secp = Secp256k1::new();
    let recipient = Address::p2tr(
        &secp,
        htlc_params.redeemer_pubkey.clone(),
        None,
        TEST_NETWORK,
    );

    let leaf_script = get_htlc_leaf_script(&htlc_params, HTLCLeaf::Redeem);
    let control_block = get_control_block(&htlc_params, HTLCLeaf::Redeem)?.serialize();

    let mut witness = Witness::new();
    witness.push(b"add_signature_segwit_v1");
    witness.push(secret.to_vec());
    witness.push(leaf_script);
    witness.push(&control_block);

    let script = get_htlc_leaf_script(&htlc_params, HTLCLeaf::Redeem);
    let spend_request = SpendRequest {
        id: htlc_address.clone().to_string(),
        htlc_address,
        keypair: redeemer_keypair,
        recipient,
        script,
        utxos,
        witness,
    };

    Ok(spend_request)
}

/// Creates a test `BitcoinTxBatcher` with a fee rate estimator and indexer.
///
/// # Returns
/// A `BitcoinTxBatcher` configured with a fixed fee rate estimator.
pub async fn get_test_bitcoin_tx_batcher() -> Result<BitcoinTxBatcher> {
    let indexer = get_test_bitcoin_indexer()?;
    let fee_rate_estimator = get_test_fee_rate_estimator()?;
    let fee_level = FeeLevel::Fastest;

    Ok(BitcoinTxBatcher::new(
        indexer,
        fee_level,
        fee_rate_estimator,
    ))
}

/// Creates a test `BitcoinHTLC` with a fee rate estimator and indexer.
///
/// # Returns
/// A `BitcoinHTLC` configured with a fixed fee rate estimator.
pub async fn get_test_bitcoin_htlc(keypair: Keypair) -> Result<BitcoinHTLC> {
    let indexer = get_test_bitcoin_indexer()?;
    let fee_rate_estimator = get_test_fee_rate_estimator()?;
    let fee_level = FeeLevel::Fastest;

    Ok(BitcoinHTLC::new(
        keypair,
        indexer,
        fee_rate_estimator,
        fee_level,
        TEST_NETWORK,
    ))
}

/// Updates the secret hash in the create order and associated source/destination swaps
pub async fn update_test_matched_order_secret_hash(
    pool: &sqlx::PgPool,
    order_id: &str,
    secret_hash: &str,
) -> Result<()> {
    // Update create_orders table
    sqlx::query(
        r#"
        UPDATE create_orders
        SET secret_hash = $1, updated_at = NOW()
        WHERE create_id = $2
        "#,
    )
    .bind(secret_hash)
    .bind(order_id)
    .execute(pool)
    .await?;

    // Update source and destination swaps
    sqlx::query(
        r#"
        UPDATE swaps
        SET secret_hash = $1, updated_at = NOW()
        WHERE swap_id IN (
            SELECT mo.source_swap_id
            FROM matched_orders mo
            WHERE mo.create_order_id = $2
            UNION
            SELECT mo.destination_swap_id
            FROM matched_orders mo
            WHERE mo.create_order_id = $2
        )
        "#,
    )
    .bind(secret_hash)
    .bind(order_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Updates a matched order's source swap ID
///
/// Links a matched order to its corresponding swap by updating the source_swap_id.
async fn update_matched_order(
    pool: &sqlx::PgPool,
    order_id: &str,
    swap_id: &str,
    perform_on: SwapChain,
) -> Result<()> {
    let column_name = match perform_on {
        SwapChain::Source => "source_swap_id",
        SwapChain::Destination => "destination_swap_id",
    };

    let query = format!(
        r#"
            UPDATE matched_orders
            SET {} = $1, updated_at = NOW()
            WHERE create_order_id = $2
            "#,
        column_name
    );

    sqlx::query(&query)
        .bind(swap_id)
        .bind(order_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Creates a test Bitcoin swap entry in the database
///
/// This function creates a new swap record with the provided configuration and swap ID.
pub async fn create_bitcoin_test_swap(
    pool: &sqlx::PgPool,
    config: TestSwapConfig,
    swap_id: String,
) -> Result<SingleSwap> {
    let now = chrono::Utc::now();

    let swap = SingleSwap {
        created_at: now,
        updated_at: now,
        deleted_at: None,
        swap_id,
        chain: config.chain,
        asset: config.asset,
        htlc_address: None,
        token_address: None,
        initiator: config.initiator,
        redeemer: config.redeemer,
        timelock: config.timelock,
        filled_amount: BigDecimal::from_str("0")?,
        amount: config.amount,
        secret_hash: config.secret_hash,
        secret: MaybeString::new("".to_string()),
        initiate_tx_hash: MaybeString::new("".to_string()),
        redeem_tx_hash: MaybeString::new("".to_string()),
        refund_tx_hash: MaybeString::new("".to_string()),
        initiate_block_number: Some(BigDecimal::from_str("0")?),
        redeem_block_number: Some(BigDecimal::from_str("0")?),
        refund_block_number: Some(BigDecimal::from_str("0")?),
        required_confirmations: 1,
        current_confirmations: 0,
        initiate_timestamp: None,
        redeem_timestamp: None,
        refund_timestamp: None,
    };

    sqlx::query(
            r#"
            INSERT INTO swaps
            (created_at, updated_at, deleted_at, swap_id, chain, asset, initiator, redeemer,
            timelock, filled_amount, amount, secret_hash, secret, initiate_tx_hash, redeem_tx_hash,
            refund_tx_hash, initiate_block_number, redeem_block_number, refund_block_number,
            required_confirmations, current_confirmations)
            VALUES
            ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21)
            "#
        )
        .bind(&swap.created_at)
        .bind(&swap.updated_at)
        .bind(&swap.deleted_at)
        .bind(&swap.swap_id)
        .bind(&swap.chain)
        .bind(&swap.asset)
        .bind(&swap.initiator)
        .bind(&swap.redeemer)
        .bind(&swap.timelock)
        .bind(&swap.filled_amount)
        .bind(&swap.amount)
        .bind(&swap.secret_hash)
        .bind(&swap.secret.to_string())
        .bind(&swap.initiate_tx_hash.to_string())
        .bind(&swap.redeem_tx_hash.to_string())
        .bind(&swap.refund_tx_hash.to_string())
        .bind(&swap.initiate_block_number)
        .bind(&swap.redeem_block_number)
        .bind(&swap.refund_block_number)
        .bind(&swap.required_confirmations)
        .bind(&swap.current_confirmations)
        .execute(pool)
        .await?;

    Ok(swap)
}

pub async fn create_test_bitcoin_matched_order(
    pool: &sqlx::PgPool,
    orderbook_provider: &Arc<dyn Orderbook + Send + Sync>,
    test_swap_config: &TestSwapConfig,
    htlc_address: String,
    recipient: &Address,
    perform_on: SwapChain,
) -> Result<MatchedOrderVerbose> {
    let test_additional_data = AdditionalData {
        source_delegator: None,
        strategy_id: "arbrry".to_string(),
        bitcoin_optional_recipient: Some(recipient.to_string()),
        input_token_price: 1.0,
        output_token_price: 1.0,
        sig: "1edafa02fb0a2777aba158c43007308efea5207830b9ba844d523bec49b4fa233e08525a1da271ecf0533604512471811fda6c4406c7ec3a453a758b65d95a121b".to_string(),
        deadline: Utc::now().timestamp() + 3600,
        instant_refund_tx_bytes: None,
        redeem_tx_bytes: None,
        tx_hash: Some("0x62357149dad8db2e33ed3809247d99d47146589d8a6226d98e495cf0d2854888".to_string()),
        is_blacklisted: false,
        integrator: None,
        version: HTLCVersion::V1,
        bitcoin: None
    };

    let test_matched_order_config = TestMatchedOrderConfig {
        additional_data: test_additional_data,
        ..Default::default()
    };

    let order = create_test_matched_order(pool, test_matched_order_config).await?;

    let bitcoin_swap =
        create_bitcoin_test_swap(pool, test_swap_config.clone(), htlc_address).await?;

    // Updating the source swap of the test order to newly created bitcoin test swap.
    update_matched_order(
        &pool,
        &order.create_order.create_id,
        &bitcoin_swap.swap_id,
        perform_on,
    )
    .await?;

    let order = orderbook_provider
        .get_matched_order(&order.create_order.create_id)
        .await?
        .ok_or_else(|| eyre!("Failed to get matched order"))?;

    Ok(order)
}
