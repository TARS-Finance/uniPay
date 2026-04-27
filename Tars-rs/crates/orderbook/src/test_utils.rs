//! Test utilities to insert mock data into the database
use std::str::FromStr;

use alloy::{
    hex,
    primitives::FixedBytes,
    signers::k256::sha2::{self, Digest},
    sol_types::SolValue,
};
use bigdecimal::BigDecimal;
use chrono::Utc;
use eyre::Result;
use primitives::HTLCVersion;
use sqlx::{postgres::PgPoolOptions, Pool, Postgres, Row};
use utils::Hashable;

use crate::{
    primitives::{AdditionalData, CreateOrder, MatchedOrderVerbose, MaybeString, SingleSwap},
    OrderbookProvider,
};

#[derive(Clone, Debug)]
/// Config to insert a custom mock swap
pub struct TestSwapConfig {
    pub chain: String,
    pub asset: String,
    pub initiator: String,
    pub redeemer: String,
    pub timelock: i32,
    pub amount: BigDecimal,
    pub secret_hash: String,
    pub chain_id: String,
}

#[derive(Clone)]
/// Config to insert a custom mock matched order
pub struct TestMatchedOrderConfig {
    pub block_number: BigDecimal,
    pub source_chain: String,
    pub destination_chain: String,
    pub source_asset: String,
    pub destination_asset: String,
    pub initiator_source_address: String,
    pub initiator_destination_address: String,
    pub source_amount: BigDecimal,
    pub destination_amount: BigDecimal,
    pub user_id: String,
    pub fee: BigDecimal,
    pub nonce: BigDecimal,
    pub min_destination_confirmations: i32,
    pub timelock: i32,
    pub source_swap_config: Option<TestSwapConfig>,
    pub destination_swap_config: Option<TestSwapConfig>,
    pub additional_data: AdditionalData,
    pub source_chain_redeemer_address: String,
    pub destination_chain_initiator_address: String,
}

impl Default for TestMatchedOrderConfig {
    fn default() -> Self {
        let initiator_source_address =
            format!("bcrt1q{}", hex::encode(&rand::random::<[u8; 20]>()));

        let initiator_destination_address = format!(
            "0x{}",
            alloy::primitives::hex::encode(alloy_primitives::Address::random())
        );
        let source_chain_redeemer_address =
            "bcd6f4cfa96358c74dbc03fec5ba25da66bbc92a31b714ce339dd93db1a9ffac".to_string();
        let destination_chain_initiator_address =
            "0x70997970c51812dc3a010c7d01b50e0d17dc79c8".to_string();

        Self {
            block_number: BigDecimal::from_str("226").unwrap(),
            source_chain: "bitcoin_regtest".to_string(),
            destination_chain: "ethereum_localnet".to_string(),
            source_asset: "primary".to_string(),
            destination_asset: "0x0165878A594ca255338adfa4d48449f69242Eb8F".to_string(),
            initiator_source_address: initiator_source_address.clone(),
            initiator_destination_address: initiator_destination_address.clone(),
            source_amount: BigDecimal::from(10000),
            destination_amount: BigDecimal::from(5000),
            user_id: initiator_source_address.clone(),
            fee: BigDecimal::from_str("0.0000000000000000188").unwrap(),
            nonce: BigDecimal::from_str("1").unwrap(),
            min_destination_confirmations: 6,
            timelock: 400,
            source_swap_config: None,
            destination_swap_config: None,
            additional_data: AdditionalData {
                source_delegator: None,
                strategy_id: "arbrry".to_string(),
                bitcoin_optional_recipient: None,
                input_token_price: 1.0,
                output_token_price: 1.0,
                sig: "1edafa02fb0a2777aba158c43007308efea5207830b9ba844d523bec49b4fa233e08525a1da271ecf0533604512471811fda6c4406c7ec3a453a758b65d95a121b".to_string(),
                deadline: Utc::now().timestamp() + 3600,
                instant_refund_tx_bytes: Some("45a89422a90f89ae067c3a3bb0dc64a79ef3f9b0c7caa0d6c3563e06d5ecda32".to_string()),
                redeem_tx_bytes: None,
                tx_hash: Some("0x62357149dad8db2e33ed3809247d99d47146589d8a6226d98e495cf0d2854888".to_string()),
                is_blacklisted: false,
                integrator: None,
                version: HTLCVersion::V1,
                bitcoin:None,
            },
            source_chain_redeemer_address,
            destination_chain_initiator_address,
        }
    }
}

/// Config to insert a custom mock transaction data
pub struct TestTxData {
    pub tx_hash: String,
    pub block_number: i32,
    pub filled_amount: BigDecimal,
    pub current_confirmations: i32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Creates and returns a connection to the orderbook database.
///
/// This function establishes a PostgreSQL database connection pool
/// that can be used for interacting with the Unipay orderbook system.
///
/// # Returns
/// An `OrderbookProvider` instance configured with a PostgreSQL connection pool
pub async fn provider() -> OrderbookProvider {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect("postgres://postgres:postgres@localhost:5433/unipay")
        .await
        .unwrap();
    OrderbookProvider::new(pool)
}

/// Inserts mock inititate transaction data into the given swap
pub async fn simulate_test_swap_initiate(
    pool: &Pool<Postgres>,
    swap_id: &str,
    test_tx_data: Option<TestTxData>,
) -> Result<()> {
    let (tx_hash, block_number, filled_amount, current_confirmations, timestamp) =
        match test_tx_data {
            Some(data) => (
                data.tx_hash,
                data.block_number,
                data.filled_amount,
                data.current_confirmations,
                data.timestamp,
            ),
            None => (
                "0x4a2a7a10f282155a6949f1b08db056aed20db7b448b673a4b8ad77824275e0a7".to_lowercase(),
                132,
                BigDecimal::from(10000),
                3,
                chrono::Utc::now(),
            ),
        };

    sqlx::query("UPDATE swaps SET initiate_tx_hash = $1, initiate_block_number = $2, filled_amount = $3, current_confirmations = $4, initiate_timestamp = $5 WHERE swap_id = $6")
        .bind(tx_hash)
        .bind(block_number)
        .bind(filled_amount)
        .bind(current_confirmations)
        .bind(timestamp)
        .bind(swap_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Inserts mock redeem transaction data into the given swap
pub async fn simulate_test_swap_redeem(
    pool: &Pool<Postgres>,
    swap_id: &str,
    secret: &str,
    test_tx_data: Option<TestTxData>,
) -> Result<()> {
    let (tx_hash, block_number, timestamp) = match test_tx_data {
        Some(data) => (data.tx_hash, data.block_number, data.timestamp),
        None => (
            "0x4a2a7a10f282155a6949f1b08db056aed20db7b448b673a4b8ad77824275e0a7".to_lowercase(),
            132,
            chrono::Utc::now(),
        ),
    };

    sqlx::query("UPDATE swaps SET redeem_tx_hash = $1, redeem_block_number = $2, secret = $3, redeem_timestamp = $4 WHERE swap_id = $5")
        .bind(tx_hash)
        .bind(block_number)
        .bind(secret)
        .bind(timestamp)
        .bind(swap_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Inserts mock refund transaction data into the given swap
pub async fn simulate_test_swap_refund(
    pool: &Pool<Postgres>,
    swap_id: &str,
    test_tx_data: Option<TestTxData>,
) -> Result<()> {
    let (tx_hash, block_number, timestamp) = match test_tx_data {
        Some(data) => (data.tx_hash, data.block_number, data.timestamp),
        None => (
            "0x4a2a7a10f282155a6949f1b08db056aed20db7b448b673a4b8ad77824275e0a7".to_lowercase(),
            132,
            chrono::Utc::now(),
        ),
    };

    sqlx::query("UPDATE swaps SET refund_tx_hash = $1, refund_block_number = $2, refund_timestamp = $3 WHERE swap_id = $4")
        .bind(tx_hash)
        .bind(block_number)
        .bind(timestamp)
        .bind(swap_id)
        .execute(pool)
        .await?;

    Ok(())
}

fn generate_test_swap_id(
    chain_id: &str,
    secret_hash: &str,
    initiator: &str,
) -> Result<FixedBytes<32>, eyre::Error> {
    let components = (chain_id, secret_hash, initiator);
    let hash = sha2::Sha256::digest(components.abi_encode());
    Ok(FixedBytes::new(hash.into()))
}

/// Inserts a mock swap into the database with the given configuration
pub async fn create_test_swap(
    pool: &sqlx::PgPool,
    config: TestSwapConfig,
) -> Result<SingleSwap, eyre::Error> {
    let now = chrono::Utc::now();

    let swap_id = generate_test_swap_id(&config.chain_id, &config.secret_hash, &config.initiator)
        .unwrap()
        .to_string();

    let swap = SingleSwap {
        created_at: now,
        updated_at: now,
        deleted_at: None,
        swap_id,
        chain: config.chain,
        asset: config.asset,
        htlc_address: None,  // Optional, can be set later if needed
        token_address: None, // Optional, can be set later if needed
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
        required_confirmations: 3,
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

/// Inserts a mock unmatched order into the database
pub async fn create_test_unmatched_order(
    pool: &sqlx::PgPool,
    initiator_address: Option<String>,
) -> Result<CreateOrder, eyre::Error> {
    let secret: String = (0..32)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();

    let secret_hash = secret.sha256().unwrap();

    let address1 = initiator_address.unwrap_or_else(|| {
        format!(
            "0x{}",
            alloy::primitives::hex::encode(alloy_primitives::Address::random())
        )
    });

    let now = chrono::Utc::now();

    // generating a random create_id, not the actual process
    let mut hasher = sha2::Sha256::new();
    hasher.update(secret_hash);
    let create_id = format!("{:x}", hasher.finalize());

    let additional_data = AdditionalData {
        source_delegator: None,
        strategy_id: "alel12".to_string(),
        bitcoin_optional_recipient: None,
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
        bitcoin:None
    };

    let create_order = CreateOrder {
        created_at: now,
        updated_at: now,
        deleted_at: None,
        create_id: create_id.to_string(),
        block_number: BigDecimal::from_str("226")?,
        source_chain: "arbitrum_localnet".to_string(),
        destination_chain: "ethereum_localnet".to_string(),
        source_asset: "0x0165878A594ca255338adfa4d48449f69242Eb8F".to_string(),
        destination_asset: "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0".to_string(),
        initiator_source_address: address1.to_string(),
        initiator_destination_address: address1.to_string(),
        source_amount: BigDecimal::from_str("10000")?,
        destination_amount: BigDecimal::from_str("1000")?,
        fee: BigDecimal::from_str("0.0000000000000000188")?,
        nonce: BigDecimal::from_str("1")?,
        affiliate_fees: Some(vec![]),
        min_destination_confirmations: 6,
        timelock: 400,
        secret_hash: secret_hash.to_string(),
        user_id: Some(address1.to_string()),
        additional_data,
    };

    let additional_data_json = serde_json::to_value(&create_order.additional_data)?;

    sqlx::query(
        r#"
    INSERT INTO create_orders
    (created_at, updated_at, deleted_at, create_id, block_number, source_chain,
    destination_chain, source_asset, destination_asset, initiator_source_address,
    initiator_destination_address, source_amount, destination_amount, fee, nonce,
    min_destination_confirmations, timelock, secret_hash, user_id, additional_data)
    VALUES
    ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
    "#,
    )
    .bind(&create_order.created_at)
    .bind(&create_order.updated_at)
    .bind(&create_order.deleted_at)
    .bind(&create_order.create_id)
    .bind(&create_order.block_number)
    .bind(&create_order.source_chain)
    .bind(&create_order.destination_chain)
    .bind(&create_order.source_asset)
    .bind(&create_order.destination_asset)
    .bind(&create_order.initiator_source_address)
    .bind(&create_order.initiator_destination_address)
    .bind(&create_order.source_amount)
    .bind(&create_order.destination_amount)
    .bind(&create_order.fee)
    .bind(&create_order.nonce)
    .bind(&create_order.min_destination_confirmations)
    .bind(&create_order.timelock)
    .bind(&create_order.secret_hash)
    .bind(&create_order.user_id)
    .bind(&additional_data_json)
    .execute(pool)
    .await?;

    Ok(create_order)
}

/// Inserts a mock matched order into the database with the given configuration
pub async fn create_test_matched_order(
    pool: &sqlx::PgPool,
    config: TestMatchedOrderConfig,
) -> Result<MatchedOrderVerbose, eyre::Error> {
    let secret: String = (0..32)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect();

    let secret_hash = match config.source_swap_config.clone() {
        Some(config) => config.secret_hash,
        None => secret.sha256()?.to_string(),
    };

    let now = chrono::Utc::now();

    let mut hasher = sha2::Sha256::new();
    hasher.update(secret_hash.clone());
    let create_id = format!("{:x}", hasher.finalize());

    let create_order = CreateOrder {
        created_at: now,
        updated_at: now,
        deleted_at: None,
        create_id,
        block_number: config.block_number,
        source_chain: config.source_chain.clone(),
        destination_chain: config.destination_chain.clone(),
        source_asset: config.source_asset.clone(),
        destination_asset: config.destination_asset.clone(),
        initiator_source_address: config.initiator_source_address.clone(),
        initiator_destination_address: config.initiator_destination_address.clone(),
        source_amount: config.source_amount.clone(),
        destination_amount: config.destination_amount.clone(),
        fee: config.fee,
        nonce: config.nonce,
        affiliate_fees: Some(vec![]),
        min_destination_confirmations: config.min_destination_confirmations,
        timelock: config.timelock,
        secret_hash: secret_hash.clone(),
        user_id: Some(config.user_id.clone()),
        additional_data: config.additional_data,
    };

    let additional_data_json = serde_json::to_value(&create_order.additional_data)?;

    sqlx::query(
        r#"
        INSERT INTO create_orders
        (created_at, updated_at, deleted_at, create_id, block_number, source_chain,
        destination_chain, source_asset, destination_asset, initiator_source_address,
        initiator_destination_address, source_amount, destination_amount, fee, nonce,
        min_destination_confirmations, timelock, secret_hash, user_id, additional_data)
        VALUES
        ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20)
        "#,
    )
    .bind(&create_order.created_at)
    .bind(&create_order.updated_at)
    .bind(&create_order.deleted_at)
    .bind(&create_order.create_id)
    .bind(&create_order.block_number)
    .bind(&create_order.source_chain)
    .bind(&create_order.destination_chain)
    .bind(&create_order.source_asset)
    .bind(&create_order.destination_asset)
    .bind(&create_order.initiator_source_address)
    .bind(&create_order.initiator_destination_address)
    .bind(&create_order.source_amount)
    .bind(&create_order.destination_amount)
    .bind(&create_order.fee)
    .bind(&create_order.nonce)
    .bind(&create_order.min_destination_confirmations)
    .bind(&create_order.timelock)
    .bind(&create_order.secret_hash)
    .bind(&create_order.user_id)
    .bind(&additional_data_json)
    .execute(pool)
    .await?;

    let source_swap_config = config.source_swap_config.unwrap_or_else(|| TestSwapConfig {
        chain: config.source_chain.clone(),
        asset: config.source_asset.clone(),
        initiator: config.initiator_source_address.clone(),
        redeemer: config.source_chain_redeemer_address.clone(),
        timelock: config.timelock,
        amount: config.source_amount.clone(),
        secret_hash: secret_hash.clone(),
        chain_id: "0x7a69".to_string(),
    });

    let destination_swap_config =
        config
            .destination_swap_config
            .unwrap_or_else(|| TestSwapConfig {
                chain: config.destination_chain.clone(),
                asset: config.destination_asset.clone(),
                initiator: config.destination_chain_initiator_address.clone(),
                redeemer: config.initiator_source_address.clone(),
                timelock: 100,
                amount: config.destination_amount.clone(),
                secret_hash: secret_hash.clone(),
                chain_id: "0x7a69".to_string(),
            });

    let source_swap = create_test_swap(pool, source_swap_config).await?;
    let destination_swap = create_test_swap(pool, destination_swap_config).await?;

    sqlx::query(
        r#"
        INSERT INTO matched_orders
        (created_at, updated_at, deleted_at, create_order_id, source_swap_id, destination_swap_id)
        VALUES
        ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(&now)
    .bind(&now)
    .bind::<Option<chrono::DateTime<chrono::Utc>>>(None)
    .bind(&create_order.create_id)
    .bind(&source_swap.swap_id)
    .bind(&destination_swap.swap_id)
    .execute(pool)
    .await?;

    println!(
        "Created order: {} with source_swap_id: {} and destination_swap_id: {}",
        create_order.create_id, source_swap.swap_id, destination_swap.swap_id
    );

    let matched_order_verbose = MatchedOrderVerbose {
        created_at: now,
        updated_at: now,
        deleted_at: None,
        source_swap,
        destination_swap,
        create_order,
    };

    Ok(matched_order_verbose)
}

/// Deletes an unmatched order from the database
pub async fn delete_unmatched_order(
    pool: &sqlx::PgPool,
    create_id: &str,
) -> Result<(), eyre::Error> {
    sqlx::query("DELETE FROM create_orders WHERE create_id = $1")
        .bind(create_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Deletes a matched order from the database
pub async fn delete_matched_order(pool: &sqlx::PgPool, create_id: &str) -> Result<(), eyre::Error> {
    let matched_order = sqlx::query(
        "SELECT source_swap_id, destination_swap_id FROM matched_orders WHERE create_order_id = $1",
    )
    .bind(create_id)
    .fetch_optional(pool)
    .await?;

    if let Some(row) = matched_order {
        let source_swap_id: String = row.get("source_swap_id");
        let destination_swap_id: String = row.get("destination_swap_id");

        sqlx::query("DELETE FROM matched_orders WHERE create_order_id = $1")
            .bind(create_id)
            .execute(pool)
            .await?;

        sqlx::query("DELETE FROM swaps WHERE swap_id = $1 OR swap_id = $2")
            .bind(&source_swap_id)
            .bind(&destination_swap_id)
            .execute(pool)
            .await?;

        sqlx::query("DELETE FROM create_orders WHERE create_id = $1")
            .bind(create_id)
            .execute(pool)
            .await?;
    }

    Ok(())
}

/// Deletes all matched orders from the database
pub async fn delete_all_matched_orders(pool: &sqlx::PgPool) -> Result<(), eyre::Error> {
    sqlx::query("DELETE FROM matched_orders CASCADE")
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM create_orders")
        .execute(pool)
        .await?;

    sqlx::query("DELETE FROM swaps").execute(pool).await?;
    Ok(())
}

pub fn default_matched_order() -> MatchedOrderVerbose {
    let now = chrono::Utc::now();
    MatchedOrderVerbose {
        created_at: now,
        updated_at: now,
        deleted_at: None,
        source_swap: SingleSwap {
            created_at: now,
            updated_at: now,
            deleted_at: None,
            filled_amount: BigDecimal::from_str("0").unwrap(),
            secret: MaybeString::new("".to_string()),
            initiate_tx_hash: MaybeString::new("".to_string()),
            redeem_tx_hash: MaybeString::new("".to_string()),
            refund_tx_hash: MaybeString::new("".to_string()),
            initiate_block_number: None,
            redeem_block_number: None,
            refund_block_number: None,
            required_confirmations: 3,
            current_confirmations: 0,
            swap_id: "1".to_string(),
            chain: "arbitrum_localnet".to_string(),
            asset: "asset_1".to_string(),
            htlc_address: None,  // Optional, can be set later if needed
            token_address: None, // Optional, can be set later if needed
            initiator: "0x123".to_string(),
            redeemer: "0x456".to_string(),
            timelock: 100,
            amount: BigDecimal::from_str("10000").unwrap(),
            secret_hash: "0x789".to_string(),
            initiate_timestamp: None,
            redeem_timestamp: None,
            refund_timestamp: None,
        },
        destination_swap: SingleSwap {
            created_at: now,
            updated_at: now,
            deleted_at: None,
            filled_amount: BigDecimal::from_str("0").unwrap(),
            secret: MaybeString::new("".to_string()),
            initiate_tx_hash: MaybeString::new("".to_string()),
            redeem_tx_hash: MaybeString::new("".to_string()),
            refund_tx_hash: MaybeString::new("".to_string()),
            initiate_block_number: None,
            redeem_block_number: None,
            refund_block_number: None,
            required_confirmations: 3,
            current_confirmations: 0,
            swap_id: "2".to_string(),
            chain: "bitcoin_regtest".to_string(),
            asset: "asset_2".to_string(),
            htlc_address: None,  // Optional, can be set later if needed
            token_address: None, // Optional, can be set later if needed
            initiator: "0x123".to_string(),
            redeemer: "0x456".to_string(),
            timelock: 100,
            amount: BigDecimal::from_str("10000").unwrap(),
            secret_hash: "0x789".to_string(),
            initiate_timestamp: None,
            redeem_timestamp: None,
            refund_timestamp: None,
        },
        create_order: CreateOrder {
            created_at: now,
            updated_at: now,
            deleted_at: None,
            create_id: "1".to_string(),
            block_number: BigDecimal::from_str("1").unwrap(),
            source_chain: "arbitrum_localnet".to_string(),
            destination_chain: "bitcoin_regtest".to_string(),
            source_asset: "asset_1".to_string(),
            destination_asset: "asset_2".to_string(),
            initiator_source_address: "0x123".to_string(),
            initiator_destination_address: "0x456".to_string(),
            source_amount: BigDecimal::from_str("10000").unwrap(),
            destination_amount: BigDecimal::from_str("10000").unwrap(),
            fee: BigDecimal::from_str("100").unwrap(),
            nonce: BigDecimal::from_str("1").unwrap(),
            affiliate_fees: None,
            min_destination_confirmations: 3,
            timelock: 100,
            secret_hash: "0x789".to_string(),
            user_id: Some("1".to_string()),
            additional_data: AdditionalData {
                source_delegator: None,
                strategy_id: "aa1daae4".to_string(),
                bitcoin_optional_recipient: None,
                input_token_price: 1.0,
                output_token_price: 1.0,
                sig: "0x789".to_string(),
                deadline: Utc::now().timestamp() + 3600,
                instant_refund_tx_bytes: None,
                redeem_tx_bytes: None,
                tx_hash: None,
                is_blacklisted: false,
                integrator: None,
                version: HTLCVersion::V1,
                bitcoin: None,
            },
        },
    }
}
