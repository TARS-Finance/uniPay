use crate::core::{
    AddressScreener, BlockProcessor, BlockchainIndexer, Cache, RPCClient, Swap, SwapStore,
    TxEventParser, TxIndexer, listen_for_pending_swaps, poll_pending_swap_addresses,
    update_confirmations,
};
use bitcoin::{Block, Network, Transaction};
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Deserialize)]
pub struct ChainSettings {
    pub chain: String,
    pub poll_interval_ms: u64,
}

#[allow(clippy::too_many_arguments)]
pub fn watch(
    tx_receiver: mpsc::Receiver<Transaction>,
    blocks_receiver: mpsc::Receiver<Block>,
    network: Network,
    chain_settings: ChainSettings,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
    swap_cache: Arc<dyn Cache<String, Swap> + Send + Sync>,
    indexer: Arc<dyn BlockchainIndexer + Send + Sync>,
    screener: Arc<dyn AddressScreener + Send + Sync>,
    rpc_client: Arc<dyn RPCClient + Send + Sync>,
    indexer_url: String,
) {
    let chain = chain_settings.chain.clone();

    let tx_event_parser = Arc::new(TxEventParser::new(
        screener,
        indexer.clone(),
        swap_cache.clone(),
        chain.clone(),
        network,
    ));

    spawn_monitored("pending_swaps_poller", {
        let (store, cache) = (swap_store.clone(), swap_cache.clone());
        async move {
            // the cache is populated with pending swaps and it used in other tasks to access
            // pending swaps
            listen_for_pending_swaps(
                &chain_settings.chain,
                chain_settings.poll_interval_ms,
                store,
                cache,
            )
            .await;
        }
    });

    spawn_monitored("tx_processor", {
        let mut swap_indexer = TxIndexer::new(
            tx_receiver,
            swap_store.clone(),
            rpc_client,
            tx_event_parser.clone(),
        );
        async move { swap_indexer.index().await }
    });

    spawn_monitored("block_processor", {
        let mut processor =
            BlockProcessor::new(blocks_receiver, swap_store.clone(), tx_event_parser);
        async move { processor.process().await }
    });

    spawn_monitored("confirmation_updater", {
        let chain_for_confirm = chain.clone();
        let store_for_confirm = swap_store.clone();
        async move {
            update_confirmations(&chain_for_confirm, store_for_confirm, indexer).await;
        }
    });

    spawn_monitored("address_poller", {
        async move {
            poll_pending_swap_addresses(chain, indexer_url, swap_store).await;
        }
    });
}

fn spawn_monitored(
    name: &'static str,
    task: impl std::future::Future<Output = ()> + Send + 'static,
) {
    tokio::spawn(async move {
        task.await;
        tracing::error!(task = name, "Task exited unexpectedly");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{
        BitcoinRPCClient, FixedStatusScreener, GardenBitcoinIndexer, GardenSwapStore, SwapCache,
        ZmqListener, ZmqSettings,
    };
    use bitcoin::{Address, Transaction};
    use eyre::Result;
    use tars::{
        bitcoin::{
            get_htlc_address,
            test_utils::{
                TEST_NETWORK, generate_bitcoin_random_keypair, get_test_bitcoin_indexer,
                get_test_htlc_params,
            },
        },
        orderbook::{
            primitives::{MaybeString, SingleSwap},
            test_utils::{TestSwapConfig, delete_all_matched_orders},
        },
        utils::gen_secret,
    };
    use sqlx::{
        Pool, Postgres,
        types::{BigDecimal, chrono::Utc},
    };
    use std::{process::Command, str::FromStr, time::Duration};
    use tokio::time::{self, sleep};
    use tracing::info;

    const DB_URL: &str = "postgres://postgres:postgres@localhost:5432/postgres";
    const BTC_REGNET_USERNAME: &str = "admin1";
    const BTC_REGNET_PASSWORD: &str = "123";
    const BTC_REGNET_URL: &str = "http://localhost:18443";
    fn get_bitcoin_network(chain: &str) -> Network {
        use tars::{
            bitcoin::{BITCOIN_REGTEST, BITCOIN_TESTNET},
            primitives::BITCOIN,
        };
        match chain {
            BITCOIN_REGTEST => Network::Regtest,
            BITCOIN_TESTNET => Network::Testnet,
            BITCOIN => Network::Bitcoin,
            _ => panic!("Unknown chain: {}", chain),
        }
    }

    pub async fn pool() -> Pool<Postgres> {
        sqlx::postgres::PgPoolOptions::new()
            .connect(DB_URL)
            .await
            .expect("Failed to create pool")
    }

    async fn create_test_swap(
        pool: &sqlx::PgPool,
        config: TestSwapConfig,
        swap_id: String,
    ) -> Result<SingleSwap, eyre::Error> {
        let swap_id = swap_id.trim_start_matches("0x").to_string();
        let now = Utc::now();
        info!("Creating swap with id: {}", swap_id);
        let swap = SingleSwap {
            created_at: now,
            updated_at: now,
            deleted_at: None,
            swap_id: swap_id.clone(),
            chain: config.chain.clone(),
            asset: config.asset.clone(),
            initiator: config.initiator.clone(),
            redeemer: config.redeemer.clone(),
            timelock: config.timelock,
            filled_amount: BigDecimal::from_str("0")?,
            amount: config.amount.clone(),
            secret_hash: config.secret_hash.clone(),
            secret: MaybeString::new("".to_string()),
            initiate_tx_hash: MaybeString::new("".to_string()),
            redeem_tx_hash: MaybeString::new("".to_string()),
            refund_tx_hash: MaybeString::new("".to_string()),
            initiate_block_number: Some(BigDecimal::from(0)),
            redeem_block_number: Some(BigDecimal::from(0)),
            refund_block_number: Some(BigDecimal::from(0)),
            required_confirmations: 3,
            current_confirmations: 0,
            initiate_timestamp: None,
            redeem_timestamp: None,
            refund_timestamp: None,
            htlc_address: None,
            token_address: None,
        };

        let result = sqlx::query(
        r#"
        INSERT INTO swaps
        (created_at, updated_at, deleted_at, swap_id, chain, asset, initiator, redeemer,
        timelock, filled_amount, amount, secret_hash, secret, initiate_tx_hash, redeem_tx_hash,
        refund_tx_hash, initiate_block_number, redeem_block_number, refund_block_number,
        required_confirmations, current_confirmations, htlc_address, token_address)
        VALUES
        ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22, $23)
        "#,
    )
    .bind(swap.created_at)
    .bind(swap.updated_at)
    .bind(swap.deleted_at)
    .bind(&swap.swap_id)
    .bind(&swap.chain)
    .bind(&swap.asset)
    .bind(&swap.initiator)
    .bind(&swap.redeemer)
    .bind(swap.timelock)
    .bind(&swap.filled_amount)
    .bind(&swap.amount)
    .bind(&swap.secret_hash)
    .bind(swap.secret.to_string())
    .bind(swap.initiate_tx_hash.to_string())
    .bind(swap.redeem_tx_hash.to_string())
    .bind(swap.refund_tx_hash.to_string())
    .bind(&swap.initiate_block_number)
    .bind(&swap.redeem_block_number)
    .bind(&swap.refund_block_number)
    .bind(swap.required_confirmations)
    .bind(swap.current_confirmations)
    .bind(&swap.htlc_address)
    .bind(&swap.token_address)
    .execute(pool)
    .await?;
        info!(
            "Swap inserted into database, rows affected: {}",
            result.rows_affected()
        );
        Ok(swap)
    }

    /// Creates a test HTLC with funded address
    pub async fn create_test_htlc() -> Result<(tars::bitcoin::HTLCParams, Address, String)> {
        let initiator_keypair = generate_bitcoin_random_keypair();
        let initiator_pubkey = initiator_keypair.x_only_public_key().0;

        let redeemer_keypair = generate_bitcoin_random_keypair();
        let redeemer_pubkey = redeemer_keypair.x_only_public_key().0;

        let (secret, secret_hash) = gen_secret();
        let secret_str = hex::encode(secret);
        let htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());
        let htlc_address = get_htlc_address(&htlc_params, TEST_NETWORK)?;

        Ok((htlc_params, htlc_address, secret_str))
    }

    /// Initiates an HTLC by creating and submitting an initiate transaction
    pub async fn initiate_htlc(htlc_address: &Address) -> Result<Transaction> {
        let indexer = get_test_bitcoin_indexer()?;
        let _ = Command::new("merry")
            .args(["faucet", "--to", &htlc_address.to_string()])
            .output()
            .map_err(|e| eyre::eyre!("Failed to execute merry faucet command: {}", e))?;

        info!("Funding HTLC address");
        sleep(Duration::from_secs(5)).await;

        info!("Funded HTLC address");
        let utxos = indexer.get_utxos(htlc_address).await?;

        if utxos.is_empty() {
            eyre::bail!("No UTXOs found for HTLC address");
        }

        let tx = indexer
            .get_tx_hex(&utxos.first().unwrap().txid.to_string())
            .await?;

        Ok(tx)
    }

    async fn setup_watcher(screener: Arc<dyn AddressScreener + Send + Sync>) {
        const DEADLINE_BUFFER_SECS: i64 = 24 * 60 * 60;
        let zmq_settings = ZmqSettings {
            raw_tx_url: "tcp://localhost:28333".to_string(),
            raw_block_url: "tcp://localhost:28332".to_string(),
        };
        let chain_settings: ChainSettings = ChainSettings {
            chain: "bitcoin_regtest".to_string(),
            poll_interval_ms: 1000,
        };
        let indexer_url = "http://localhost:30000".to_string();
        let address_poller_indexer_url = indexer_url.clone();

        info!("Setting up watcher");
        let swap_store: Arc<dyn SwapStore + Send + Sync> = Arc::new(
            GardenSwapStore::from_db_url(DB_URL, DEADLINE_BUFFER_SECS)
                .await
                .unwrap(),
        );

        info!("Creating swap cache");
        let swap_cache: Arc<dyn Cache<String, Swap> + Send + Sync> = Arc::new(SwapCache::new());

        let network = get_bitcoin_network(&chain_settings.chain);

        let (txs_sender, tx_receiver) = mpsc::channel(1024);
        let (blocks_sender, blocks_receiver) = mpsc::channel(256);

        let zmq_listener = ZmqListener::new(zmq_settings, txs_sender, blocks_sender);
        tokio::spawn(async move {
            zmq_listener.listen().await;
        });

        let indexer: Arc<dyn BlockchainIndexer + Send + Sync> =
            Arc::new(GardenBitcoinIndexer::new(indexer_url).unwrap());

        let rpc_client: Arc<dyn RPCClient + Send + Sync> = Arc::new(BitcoinRPCClient::new(
            BTC_REGNET_URL.to_string(),
            Some(BTC_REGNET_USERNAME.to_string()),
            Some(BTC_REGNET_PASSWORD.to_string()),
        ));

        info!("Spawning watcher");
        tokio::spawn(async move {
            watch(
                tx_receiver,
                blocks_receiver,
                network,
                chain_settings,
                swap_store,
                swap_cache,
                indexer,
                screener,
                rpc_client,
                address_poller_indexer_url,
            );
        });

        info!("Watcher spawned");

        info!("Waiting for ZMQ to be ready");
        time::sleep(Duration::from_secs(2)).await;
    }

    const UNIT_BTC_AMOUNT: u64 = 100_000_000;

    async fn fetch_swap(pool: &sqlx::PgPool, swap_id: &str) -> SingleSwap {
        sqlx::query_as("SELECT * FROM swaps WHERE swap_id = $1")
            .bind(swap_id)
            .fetch_one(pool)
            .await
            .expect("Failed to fetch swap")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial_test::serial]
    async fn test_watch() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();
        let pg_pool = pool().await;
        delete_all_matched_orders(&pg_pool).await.unwrap();

        info!("Creating test HTLC");
        let (htlc_params, htlc_address, _) = create_test_htlc().await.unwrap();
        info!("HTLC address: {}", htlc_address);

        info!("Creating test swap in database");
        let test_swap_config = TestSwapConfig {
            chain: "bitcoin_regtest".to_string(),
            asset: "primary".to_string(),
            initiator: htlc_params.initiator_pubkey.to_string(),
            redeemer: htlc_params.redeemer_pubkey.to_string(),
            timelock: 100,
            amount: BigDecimal::from(UNIT_BTC_AMOUNT),
            secret_hash: hex::encode(htlc_params.secret_hash),
            chain_id: "btc".to_string(),
        };
        let _ = create_test_swap(&pg_pool, test_swap_config, htlc_address.to_string())
            .await
            .unwrap();
        info!("Swap created in database with ID: {}", htlc_address);

        time::sleep(Duration::from_millis(100)).await;

        info!("Setting up watcher (swap already in DB)");
        setup_watcher(Arc::new(FixedStatusScreener::new(false))).await;

        info!("Waiting for cache to be populated and ZMQ to be ready...");
        time::sleep(Duration::from_secs(3)).await;

        info!("Initiating HTLC (transaction will be submitted to network)");
        let init_tx = initiate_htlc(&htlc_address).await.unwrap();
        info!(
            "Transaction submitted with txid: {}",
            init_tx.compute_txid()
        );

        sleep(Duration::from_secs(10)).await;

        info!("Fetching swap from database");
        let swap = fetch_swap(&pg_pool, &htlc_address.to_string()).await;
        info!(
            "Swap fetched: initiate_tx_hash={}, filled_amount={}",
            swap.initiate_tx_hash.as_str(),
            swap.filled_amount
        );

        info!("Asserting swap");
        assert!(
            swap.initiate_tx_hash
                .as_str()
                .contains(&init_tx.compute_txid().to_string()),
            "initiate_tx_hash does not contain the expected txid"
        );
        dbg!(&swap.amount, &swap.filled_amount);
        assert_eq!(swap.amount, swap.filled_amount);
        dbg!(swap.initiate_timestamp);
    }
}
