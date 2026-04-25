use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use btc_executor::infrastructure::chain::bitcoin::clients::{BitcoindRpcClient, ElectrsClient};
use btc_executor::infrastructure::chain::bitcoin::fee_providers::{
    ElectrsFeeRateEstimator, FeeRateEstimator,
};
use btc_executor::infrastructure::chain::bitcoin::primitives::*;
use btc_executor::infrastructure::chain::bitcoin::tx_builder::{
    cover_utxo::BitcoinCoverUtxoProvider,
    deps::CoverUtxoProvider,
    fee_builder,
    primitives::{BitcoinTxAdaptorParams, CoverUtxo},
    tx_adaptor::BitcoinTxAdaptor,
};
use btc_executor::infrastructure::chain::bitcoin::wallet::{
    BitcoinHtlcWalletAdapter, BitcoinWalletRunner, ChainAnchor, HtlcAction, LineageId,
    SendRequest, SpendRequest, SubmittedWalletBatch, WalletConfig, WalletRequest,
    WalletRequestKind, WalletRequestSubmitter, WalletStore,
};
use btc_executor::infrastructure::keys::BitcoinWallet;
use btc_executor::infrastructure::persistence::PgBitcoinWalletStore;
use bitcoin::secp256k1::Secp256k1;
use bitcoin::{Network, OutPoint, Sequence, TapSighashType, Transaction, Txid};
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};
use tars::bitcoin::{
    Utxo as TarsUtxo, UtxoStatus as TarsUtxoStatus, generate_instant_refund_hash,
};
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::ImageExt;
use tokio::sync::{mpsc, OnceCell};
use tokio::task::JoinHandle;
use uuid::Uuid;

pub(crate) const ELECTRS_URL: &str = "http://localhost:30000";
pub(crate) const BITCOIND_URL: &str = "http://localhost:18443";
pub(crate) const BITCOIND_USER: &str = "admin1";
pub(crate) const BITCOIND_PASS: &str = "123";
pub(crate) const NETWORK: Network = Network::Regtest;
pub(crate) const TX_CONFIRMATION_TIMEOUT_MS: u64 = 500;
pub(crate) const TX_CONFIRMATION_MAX_RETRIES: u64 = 20;
pub(crate) const DEFAULT_FEE_RATE: f64 = 2.0;
pub(crate) const BATCHER_INTERVAL_SECS: u64 = 1;

static TEST_POSTGRES_URL: OnceCell<String> = OnceCell::const_new();
static TEST_POSTGRES_CONTAINER_ID: StdMutex<Option<String>> = StdMutex::new(None);

#[ctor::dtor]
fn cleanup_test_postgres() {
    let id = TEST_POSTGRES_CONTAINER_ID
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    if let Some(container_id) = id {
        let _ = Command::new("docker")
            .args(["rm", "-f", &container_id])
            .output();
    }
}

#[derive(Clone)]
pub(crate) struct TestDatabase {
    pool: PgPool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WalletDbRow {
    pub(crate) dedupe_key: String,
    pub(crate) status: String,
    pub(crate) lineage_id: Option<String>,
    pub(crate) batch_txid: Option<String>,
    pub(crate) txid_history: Vec<String>,
    pub(crate) chain_anchor: Option<serde_json::Value>,
}

pub(crate) struct WalletRowUpdate<'a> {
    pub(crate) status: &'a str,
    pub(crate) lineage_id: Option<LineageId>,
    pub(crate) batch_txid: Option<Txid>,
    pub(crate) txid_history: &'a [Txid],
    pub(crate) chain_anchor: Option<ChainAnchor>,
}

impl TestDatabase {
    pub(crate) async fn new() -> Self {
        let admin_url = postgres_admin_url().await;
        let db_name = format!("bitcoin_regtest_{}", Uuid::new_v4().simple());
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&admin_url)
            .await
            .expect("connect postgres admin pool");

        let create = format!("CREATE DATABASE \"{db_name}\"");
        sqlx::query(&create)
            .execute(&admin_pool)
            .await
            .expect("create test database");

        let db_url = database_url_for_name(&admin_url, &db_name);
        let pool = PgPoolOptions::new()
            .max_connections(20)
            .connect(&db_url)
            .await
            .expect("connect test database");

        tracing::info!(admin_url = %admin_url, db_url = %db_url, "wallet test db");

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run bitcoin regtest migrations");

        Self { pool }
    }

    pub(crate) fn pool(&self) -> PgPool {
        self.pool.clone()
    }

    pub(crate) fn store(&self) -> Arc<PgBitcoinWalletStore> {
        Arc::new(PgBitcoinWalletStore::new(self.pool()))
    }

    pub(crate) async fn wallet_rows(&self, scope: &str) -> Vec<WalletDbRow> {
        sqlx::query(
            "SELECT dedupe_key, status, lineage_id, batch_txid, txid_history, chain_anchor
             FROM bitcoin_wallet_requests
             WHERE scope = $1
             ORDER BY dedupe_key ASC",
        )
        .bind(scope)
        .fetch_all(&self.pool)
        .await
        .expect("load wallet db rows")
        .into_iter()
        .map(|row| WalletDbRow {
            dedupe_key: row.get("dedupe_key"),
            status: row.get("status"),
            lineage_id: row.get("lineage_id"),
            batch_txid: row.get("batch_txid"),
            txid_history: row
                .get::<serde_json::Value, _>("txid_history")
                .as_array()
                .expect("txid history array")
                .iter()
                .map(|value| value.as_str().expect("txid string").to_string())
                .collect(),
            chain_anchor: row.get("chain_anchor"),
        })
        .collect()
    }

    pub(crate) async fn wallet_row(&self, scope: &str, dedupe_key: &str) -> WalletDbRow {
        self.wallet_rows(scope)
            .await
            .into_iter()
            .find(|row| row.dedupe_key == dedupe_key)
            .expect("wallet row")
    }

    pub(crate) async fn wait_for_wallet_status(
        &self,
        scope: &str,
        dedupe_key: &str,
        expected_status: &str,
    ) -> WalletDbRow {
        for _ in 0..60 {
            let row = self.wallet_row(scope, dedupe_key).await;
            if row.status == expected_status {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
        }

        panic!("wallet row {dedupe_key} did not reach status {expected_status}");
    }

    pub(crate) async fn upsert_wallet_request_row(
        &self,
        scope: &str,
        request: &WalletRequest,
        update: WalletRowUpdate<'_>,
    ) {
        sqlx::query(
            "INSERT INTO bitcoin_wallet_requests (
                scope, dedupe_key, kind, status, lineage_id, batch_txid, txid_history, chain_anchor, payload
             ) VALUES (
                $1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9
             )
             ON CONFLICT (scope, dedupe_key) DO UPDATE
             SET kind = EXCLUDED.kind,
                 status = EXCLUDED.status,
                 lineage_id = EXCLUDED.lineage_id,
                 batch_txid = EXCLUDED.batch_txid,
                 txid_history = EXCLUDED.txid_history,
                 chain_anchor = EXCLUDED.chain_anchor,
                 payload = EXCLUDED.payload,
                 updated_at = now()",
        )
        .bind(scope)
        .bind(request.dedupe_key())
        .bind(match request.kind() {
            WalletRequestKind::Send(_) => "send",
            WalletRequestKind::Spend(_) => "spend",
        })
        .bind(update.status)
        .bind(update.lineage_id.map(|value| value.to_string()))
        .bind(update.batch_txid.map(|value| value.to_string()))
        .bind(
            serde_json::to_string(
                &update
                    .txid_history
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>(),
            )
            .expect("serialize txid history"),
        )
        .bind(
            update
                .chain_anchor
                .map(serde_json::to_value)
                .transpose()
                .expect("serialize chain anchor"),
        )
        .bind(serde_json::to_value(request.kind()).expect("serialize wallet request kind"))
        .execute(&self.pool)
        .await
        .expect("upsert wallet request row");
    }
}

async fn postgres_admin_url() -> String {
    TEST_POSTGRES_URL
        .get_or_init(|| async {
            dotenvy::dotenv().ok();
            if let Ok(url) = std::env::var("LOCAL_POSTGRES_URL") {
                return url;
            }
            if let Ok(url) = std::env::var("DATABASE_URL") {
                return url;
            }

            let container = Postgres::default()
                .with_startup_timeout(Duration::from_secs(60))
                .start()
                .await
                .expect("start postgres testcontainer");

            let host = container.get_host().await.expect("postgres host");
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("postgres port");
            let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

            *TEST_POSTGRES_CONTAINER_ID
                .lock()
                .expect("lock postgres container id") = Some(container.id().to_string());
            std::mem::forget(container);

            url
        })
        .await
        .clone()
}

fn database_url_for_name(admin_url: &str, db_name: &str) -> String {
    let mut url = reqwest::Url::parse(admin_url).expect("valid postgres admin url");
    url.set_path(&format!("/{db_name}"));
    url.to_string()
}

#[derive(Clone, Debug)]
pub(crate) struct SubmittedBatchTx {
    pub(crate) lineage_id: LineageId,
    pub(crate) txid: Txid,
    pub(crate) request_keys: Vec<String>,
    pub(crate) replaces: Option<Txid>,
    pub(crate) raw_tx: Transaction,
}

pub(crate) struct BatcherHarness {
    submitter: Arc<dyn WalletRequestSubmitter>,
    submitted_rx: mpsc::Receiver<SubmittedWalletBatch>,
    bitcoind: Arc<BitcoindRpcClient>,
    handle: Option<JoinHandle<()>>,
    runner: Option<BitcoinWalletRunner>,
    _db: Arc<TestDatabase>,
}

impl BatcherHarness {
    pub(crate) fn submitter(&self) -> Arc<dyn WalletRequestSubmitter> {
        Arc::clone(&self.submitter)
    }

    pub(crate) fn start(&mut self) {
        if self.handle.is_some() {
            return;
        }

        let runner = self.runner.take().expect("batcher runner to start");
        self.handle = Some(tokio::spawn(runner.run()));
    }

    pub(crate) async fn submit(&self, request: WalletRequest) {
        self.submitter
            .submit(request)
            .await
            .expect("submit wallet request");
    }

    pub(crate) async fn submit_and_wait(&self, request: WalletRequest) -> Txid {
        self.submitter
            .submit_and_wait(request)
            .await
            .expect("submit and wait wallet request")
    }

    pub(crate) async fn wait_for_submitted_tx(&mut self) -> SubmittedBatchTx {
        let submitted = self
            .submitted_rx
            .recv()
            .await
            .expect("submitted wallet tx event");
        SubmittedBatchTx {
            lineage_id: submitted.lineage_id,
            txid: submitted.txid,
            request_keys: submitted.request_keys,
            replaces: submitted.replaces,
            raw_tx: tx_from_node(self.bitcoind.as_ref(), &submitted.txid).await,
        }
    }

    pub(crate) async fn assert_no_submitted_tx_within(&mut self, duration: Duration) {
        match tokio::time::timeout(duration, self.submitted_rx.recv()).await {
            Ok(Some(event)) => panic!("unexpected submitted tx event {}", event.txid),
            Ok(None) => panic!("submitted tx channel closed unexpectedly"),
            Err(_) => {},
        }
    }
}

impl Drop for BatcherHarness {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

pub(crate) struct BitcoinTestEnv {
    pub(crate) electrs: Arc<ElectrsClient>,
    pub(crate) bitcoind: Arc<BitcoindRpcClient>,
    pub(crate) miner: Arc<BitcoinWallet>,
    pub(crate) wallet: Arc<BitcoinWallet>,
    pub(crate) counterparty: Arc<BitcoinWallet>,
}

impl BitcoinTestEnv {
    pub(crate) async fn new() -> Self {
        // Initialize tracing so batcher info traces appear with --nocapture
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| {
                        tracing_subscriber::EnvFilter::new(
                            "info,btc_executor::infrastructure::chain::bitcoin::wallet=debug,btc_executor::infrastructure::persistence::bitcoin_wallet=debug",
                        )
                    }),
            )
            .with_test_writer()
            .try_init();
        let miner = Self::random_wallet();
        let wallet = Self::random_wallet();
        let counterparty = Self::random_wallet();
        let electrs = Arc::new(ElectrsClient::new(ELECTRS_URL.to_string()));
        let bitcoind = Arc::new(BitcoindRpcClient::new(
            BITCOIND_URL.to_string(),
            BITCOIND_USER.to_string(),
            BITCOIND_PASS.to_string(),
        ));
        let env = Self {
            electrs,
            bitcoind,
            miner,
            wallet,
            counterparty,
        };

        env.fund_with_merry(env.wallet.address()).await;
        env.fund_with_merry(env.counterparty.address()).await;

        env
    }

    pub(crate) async fn mine_blocks(&self, n: u64) {
        self.bitcoind
            .generate_to_address(n, &self.miner.address().to_string())
            .await
            .expect("mine blocks");
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    pub(crate) fn random_wallet() -> Arc<BitcoinWallet> {
        let secret_key =
            bitcoin::secp256k1::SecretKey::new(&mut bitcoin::secp256k1::rand::thread_rng());
        let wallet =
            BitcoinWallet::from_private_key(&hex::encode(secret_key.secret_bytes()), NETWORK)
                .expect("random wallet");
        Arc::new(wallet)
    }

    pub(crate) async fn funded_random_wallet(&self) -> Arc<BitcoinWallet> {
        let wallet = Self::random_wallet();
        self.fund_with_merry(wallet.address()).await;
        wallet
    }

    pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }

    pub(crate) async fn get_confirmed_utxos(
        &self,
        address: &str,
    ) -> Vec<btc_executor::infrastructure::chain::bitcoin::clients::Utxo> {
        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            match self.electrs.get_address_utxos(address).await {
                Ok(utxos) => {
                    return utxos.into_iter().filter(|u| u.status.confirmed).collect();
                },
                Err(_) => {
                    tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
                },
            }
        }

        panic!("get utxos failed for {address}");
    }

    pub(crate) async fn fund_with_merry(&self, address: &bitcoin::Address) {
        let initial_utxos_len = self.get_confirmed_utxos(&address.to_string()).await.len();

        let output = Command::new("merry")
            .args(["faucet", "--to", &address.to_string()])
            .output()
            .expect("execute merry faucet");

        assert!(
            output.status.success(),
            "merry faucet failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            let current_utxos = self.get_confirmed_utxos(&address.to_string()).await;
            if current_utxos.len() > initial_utxos_len {
                return;
            }

            tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
        }

        panic!("faucet funding did not appear for {address}");
    }

    pub(crate) async fn get_confirmed_cover_utxos(
        &self,
        address: &bitcoin::Address,
    ) -> Vec<CoverUtxo> {
        self.get_confirmed_utxos(&address.to_string())
            .await
            .into_iter()
            .map(|utxo| CoverUtxo {
                outpoint: OutPoint {
                    txid: utxo.txid.parse().expect("valid txid"),
                    vout: utxo.vout,
                },
                value: utxo.value,
                script_pubkey: address.script_pubkey(),
            })
            .collect()
    }

    pub(crate) async fn wait_for_confirmed_cover_utxo_count(
        &self,
        address: &bitcoin::Address,
        expected: usize,
    ) -> Vec<CoverUtxo> {
        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            let utxos = self.get_confirmed_cover_utxos(address).await;
            if utxos.len() == expected {
                return utxos;
            }

            tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
        }

        panic!("address {address} did not reach {expected} confirmed UTXOs");
    }

    pub(crate) async fn wait_for_confirmed_tx(&self, txid: &Txid) {
        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            match self.electrs.get_tx_status(&txid.to_string()).await {
                Ok(status) if status.confirmed => return,
                Ok(_) | Err(_) => {
                    tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
                },
            }
        }

        panic!("tx {txid} did not confirm in electrs");
    }

    pub(crate) async fn market_fee_rate(&self) -> f64 {
        1.0
    }

    pub(crate) async fn build_with_fee_builder(
        &self,
        wallet: Arc<BitcoinWallet>,
        params: BitcoinTxAdaptorParams,
    ) -> Transaction {
        let adaptor = BitcoinTxAdaptor::new(Arc::clone(&wallet), NETWORK);
        let mut cover = BitcoinCoverUtxoProvider::new(
            wallet.address().clone(),
            Arc::clone(&self.electrs),
            vec![],
            None,
            std::iter::empty(),
        );

        fee_builder::build_with_fee(&adaptor, &params, &mut cover)
            .await
            .expect("build tx with fee builder")
            .tx
    }

    pub(crate) async fn broadcast_and_confirm(&self, label: &str, tx: &Transaction) -> Txid {
        let raw_hex = bitcoin::consensus::encode::serialize_hex(tx);
        let txid_str = match self.bitcoind.send_raw_transaction(&raw_hex).await {
            Ok(txid) => txid,
            Err(err) => {
                let diagnostics = reqwest::Client::new()
                    .post(BITCOIND_URL)
                    .basic_auth(BITCOIND_USER, Some(BITCOIND_PASS))
                    .json(&serde_json::json!({
                        "jsonrpc": "1.0",
                        "id": "bitcoin_regtest",
                        "method": "testmempoolaccept",
                        "params": [[raw_hex.clone()]],
                    }))
                    .send()
                    .await
                    .expect("testmempoolaccept request")
                    .text()
                    .await
                    .expect("testmempoolaccept response");

                panic!("{label} broadcast tx failed: {err}; testmempoolaccept={diagnostics}");
            },
        };
        self.mine_blocks(1).await;
        txid_str.parse().expect("valid txid")
    }

    pub(crate) async fn broadcast_to_mempool(&self, label: &str, tx: &Transaction) -> Txid {
        let raw_hex = bitcoin::consensus::encode::serialize_hex(tx);
        let txid_str = match self.bitcoind.send_raw_transaction(&raw_hex).await {
            Ok(txid) => txid,
            Err(err) => {
                let diagnostics = reqwest::Client::new()
                    .post(BITCOIND_URL)
                    .basic_auth(BITCOIND_USER, Some(BITCOIND_PASS))
                    .json(&serde_json::json!({
                        "jsonrpc": "1.0",
                        "id": "bitcoin_regtest",
                        "method": "testmempoolaccept",
                        "params": [[raw_hex.clone()]],
                    }))
                    .send()
                    .await
                    .expect("testmempoolaccept request")
                    .text()
                    .await
                    .expect("testmempoolaccept response");

                panic!("{label} broadcast tx failed: {err}; testmempoolaccept={diagnostics}");
            },
        };
        txid_str.parse().expect("valid txid")
    }

    pub(crate) async fn bitcoind_rpc(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> serde_json::Value {
        let response = reqwest::Client::new()
            .post(BITCOIND_URL)
            .basic_auth(BITCOIND_USER, Some(BITCOIND_PASS))
            .json(&serde_json::json!({
                "jsonrpc": "1.0",
                "id": "bitcoin_regtest",
                "method": method,
                "params": params,
            }))
            .send()
            .await
            .expect("bitcoind rpc request")
            .json::<serde_json::Value>()
            .await
            .expect("bitcoind rpc json");

        if !response["error"].is_null() {
            panic!("{method} rpc failed: {}", response["error"]);
        }

        response["result"].clone()
    }

    pub(crate) async fn best_block_hash(&self) -> String {
        self.bitcoind_rpc("getbestblockhash", serde_json::json!([]))
            .await
            .as_str()
            .expect("best block hash string")
            .to_string()
    }

    pub(crate) async fn invalidate_block(&self, blockhash: &str) {
        let _ = self
            .bitcoind_rpc("invalidateblock", serde_json::json!([blockhash]))
            .await;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    pub(crate) async fn raw_mempool_txids(&self) -> Vec<Txid> {
        self.bitcoind_rpc("getrawmempool", serde_json::json!([]))
            .await
            .as_array()
            .expect("raw mempool array")
            .iter()
            .map(|txid| {
                txid.as_str()
                    .expect("mempool txid string")
                    .parse()
                    .expect("valid txid")
            })
            .collect()
    }

    pub(crate) async fn wait_for_tx_in_mempool(&self, txid: &Txid) {
        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            if self.raw_mempool_txids().await.contains(txid) {
                return;
            }

            tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
        }

        panic!("tx {txid} did not appear in mempool");
    }

    pub(crate) async fn wait_for_tx_not_in_mempool(&self, txid: &Txid) {
        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            if !self.raw_mempool_txids().await.contains(txid) {
                return;
            }

            tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
        }

        panic!("tx {txid} stayed in mempool unexpectedly");
    }

    pub(crate) async fn wait_for_tx_not_confirmed(&self, txid: &Txid) {
        for _ in 0..TX_CONFIRMATION_MAX_RETRIES {
            match self.electrs.get_tx_status(&txid.to_string()).await {
                Ok(status) if !status.confirmed => return,
                Ok(_) | Err(_) => {
                    tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
                },
            }
        }

        panic!("tx {txid} remained confirmed unexpectedly");
    }

    pub(crate) fn refund_spend(
        &self,
        htlc_params: &HTLCParams,
        utxos: Vec<CoverUtxo>,
    ) -> SpendRequest {
        let htlc_address = get_htlc_address(htlc_params, NETWORK).expect("htlc address");
        let script = get_htlc_leaf_script(htlc_params, HTLCLeaf::Refund);
        let witness = get_refund_witness(htlc_params).expect("refund witness");
        let utxo = utxos.into_iter().next().expect("refund utxo");

        SpendRequest {
            outpoint: utxo.outpoint,
            value: utxo.value,
            script_pubkey: htlc_address.script_pubkey(),
            witness_template: witness,
            script: script.clone(),
            leaf_hash: script.as_script().tapscript_leaf_hash(),
            sequence: Sequence::from_consensus(htlc_params.timelock as u32),
            sighash_type: TapSighashType::All,
            recipient: None,
        }
    }

    pub(crate) fn redeem_spend(
        &self,
        htlc_params: &HTLCParams,
        secret: &[u8],
        utxos: Vec<CoverUtxo>,
    ) -> SpendRequest {
        let htlc_address = get_htlc_address(htlc_params, NETWORK).expect("htlc address");
        let script = get_htlc_leaf_script(htlc_params, HTLCLeaf::Redeem);
        let witness = get_redeem_witness(htlc_params, secret).expect("redeem witness");
        let utxo = utxos.into_iter().next().expect("redeem utxo");

        SpendRequest {
            outpoint: utxo.outpoint,
            value: utxo.value,
            script_pubkey: htlc_address.script_pubkey(),
            witness_template: witness,
            script: script.clone(),
            leaf_hash: script.as_script().tapscript_leaf_hash(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            sighash_type: TapSighashType::All,
            recipient: None,
        }
    }

    pub(crate) fn instant_refund_spend(
        &self,
        htlc_params: &HTLCParams,
        utxos: Vec<CoverUtxo>,
        recipient: bitcoin::Address,
    ) -> SpendRequest {
        let htlc_address = get_htlc_address(htlc_params, NETWORK).expect("htlc address");
        let script = get_htlc_leaf_script(htlc_params, HTLCLeaf::InstantRefund);
        let placeholder_sig = [0_u8; 64];
        let witness = get_instant_refund_witness(htlc_params, &placeholder_sig, &placeholder_sig)
            .expect("instant refund witness");
        let utxo = utxos.into_iter().next().expect("instant refund utxo");

        SpendRequest {
            outpoint: utxo.outpoint,
            value: utxo.value,
            script_pubkey: htlc_address.script_pubkey(),
            witness_template: witness,
            script: script.clone(),
            leaf_hash: script.as_script().tapscript_leaf_hash(),
            sequence: Sequence::MAX,
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
            recipient: Some(SendRequest {
                address: recipient,
                amount: htlc_params.amount,
            }),
        }
    }

    pub(crate) fn finalize_instant_refund_witness(
        &self,
        htlc_params: &HTLCParams,
        tx: &mut Transaction,
        input_index: usize,
        utxo: &CoverUtxo,
    ) {
        self.finalize_instant_refund_witness_with_wallets(
            htlc_params,
            tx,
            input_index,
            utxo,
            self.wallet.as_ref(),
            self.counterparty.as_ref(),
        );
    }

    pub(crate) fn finalize_instant_refund_witness_with_wallets(
        &self,
        htlc_params: &HTLCParams,
        tx: &mut Transaction,
        input_index: usize,
        utxo: &CoverUtxo,
        initiator_wallet: &BitcoinWallet,
        redeemer_wallet: &BitcoinWallet,
    ) {
        let script = get_htlc_leaf_script(htlc_params, HTLCLeaf::InstantRefund);
        let leaf_hash = script.as_script().tapscript_leaf_hash();
        let prevout = bitcoin::TxOut {
            value: bitcoin::Amount::from_sat(utxo.value),
            script_pubkey: utxo.script_pubkey.clone(),
        };

        let mut sighash_generator = TapScriptSpendSigHashGenerator::new(tx.clone(), leaf_hash);
        let sighash = sighash_generator
            .with_prevout(
                input_index,
                &prevout,
                TapSighashType::SinglePlusAnyoneCanPay,
            )
            .expect("instant refund sighash");

        let initiator_signature = initiator_wallet
            .sign_taproot_script_spend(&sighash, TapSighashType::SinglePlusAnyoneCanPay)
            .expect("initiator signature");
        let redeemer_signature = redeemer_wallet
            .sign_taproot_script_spend(&sighash, TapSighashType::SinglePlusAnyoneCanPay)
            .expect("redeemer signature");

        let initiator_signature_bytes = initiator_signature.serialize();
        let redeemer_signature_bytes = redeemer_signature.serialize();
        tx.input[input_index].witness = get_instant_refund_witness(
            htlc_params,
            &initiator_signature_bytes,
            &redeemer_signature_bytes,
        )
        .expect("signed instant refund witness");
    }

    pub(crate) fn htlc_params(&self, secret: &[u8], amount: u64, timelock: u64) -> HTLCParams {
        self.htlc_params_for_wallets(
            self.wallet.as_ref(),
            self.counterparty.as_ref(),
            secret,
            amount,
            timelock,
        )
    }

    pub(crate) fn htlc_params_for_wallets(
        &self,
        initiator: &BitcoinWallet,
        redeemer: &BitcoinWallet,
        secret: &[u8],
        amount: u64,
        timelock: u64,
    ) -> HTLCParams {
        HTLCParams {
            initiator_pubkey: *initiator.x_only_pubkey(),
            redeemer_pubkey: *redeemer.x_only_pubkey(),
            amount,
            secret_hash: Self::sha256(secret),
            timelock,
        }
    }

    pub(crate) fn send_request(
        &self,
        dedupe_key: impl Into<String>,
        address: bitcoin::Address,
        amount: u64,
    ) -> WalletRequest {
        WalletRequest::send(dedupe_key, address, amount).expect("valid send request")
    }

    pub(crate) async fn prepare_htlc_action(
        &self,
        executor_wallet: Arc<BitcoinWallet>,
        action: HtlcAction,
    ) -> Vec<WalletRequest> {
        BitcoinHtlcWalletAdapter::new(
            *executor_wallet.x_only_pubkey(),
            Arc::clone(&self.electrs),
            NETWORK,
        )
            .prepare(action)
            .await
            .expect("prepare wallet requests from HTLC action")
    }

    pub(crate) async fn prepare_single_htlc_action(
        &self,
        executor_wallet: Arc<BitcoinWallet>,
        action: HtlcAction,
    ) -> WalletRequest {
        let mut requests = self.prepare_htlc_action(executor_wallet, action).await;
        assert_eq!(requests.len(), 1, "expected exactly one wallet request");
        requests.remove(0)
    }

    pub(crate) async fn new_test_db(&self) -> Arc<TestDatabase> {
        Arc::new(TestDatabase::new().await)
    }

    pub(crate) async fn spawn_batcher(&self, wallet: Arc<BitcoinWallet>) -> BatcherHarness {
        let db = self.new_test_db().await;
        self.spawn_batcher_with_db(wallet, db).await
    }

    pub(crate) async fn spawn_batcher_with_db(
        &self,
        wallet: Arc<BitcoinWallet>,
        db: Arc<TestDatabase>,
    ) -> BatcherHarness {
        self.spawn_batcher_with_db_and_config(wallet, db, BATCHER_INTERVAL_SECS, 1)
            .await
    }

    pub(crate) async fn spawn_batcher_with_db_and_config(
        &self,
        wallet: Arc<BitcoinWallet>,
        db: Arc<TestDatabase>,
        tick_interval_secs: u64,
        chain_anchor_confirmations: u64,
    ) -> BatcherHarness {
        self.spawn_batcher_internal(
            wallet,
            db,
            tick_interval_secs,
            chain_anchor_confirmations,
            true,
        )
        .await
    }

    pub(crate) async fn spawn_batcher_paused_with_db_and_tick_interval(
        &self,
        wallet: Arc<BitcoinWallet>,
        db: Arc<TestDatabase>,
        tick_interval_secs: u64,
    ) -> BatcherHarness {
        self.spawn_batcher_paused_with_db_and_config(wallet, db, tick_interval_secs, 1)
            .await
    }

    pub(crate) async fn spawn_batcher_paused_with_db_and_config(
        &self,
        wallet: Arc<BitcoinWallet>,
        db: Arc<TestDatabase>,
        tick_interval_secs: u64,
        chain_anchor_confirmations: u64,
    ) -> BatcherHarness {
        self.spawn_batcher_internal(
            wallet,
            db,
            tick_interval_secs,
            chain_anchor_confirmations,
            false,
        )
        .await
    }

    async fn spawn_batcher_internal(
        &self,
        wallet: Arc<BitcoinWallet>,
        db: Arc<TestDatabase>,
        tick_interval_secs: u64,
        chain_anchor_confirmations: u64,
        autostart: bool,
    ) -> BatcherHarness {
        let (submitted_tx, submitted_rx) = mpsc::channel(16);
        let fee_estimator: Arc<dyn FeeRateEstimator> =
            Arc::new(ElectrsFeeRateEstimator::new(Arc::clone(&self.electrs)));
        let store: Arc<dyn WalletStore> = db.store();
        let (runner, submitter) = BitcoinWalletRunner::new(
            wallet,
            store,
            Arc::clone(&self.electrs),
            Arc::clone(&self.bitcoind),
            fee_estimator,
            NETWORK,
            WalletConfig {
                tick_interval_secs,
                missing_batch_threshold: 20,
                chain_anchor_confirmations,
                ..WalletConfig::default()
            },
            Some(submitted_tx),
        )
        .await
        .expect("spawn bitcoin wallet runner");

        let (handle, runner) = if autostart {
            (Some(tokio::spawn(runner.run())), None)
        } else {
            (None, Some(runner))
        };

        BatcherHarness {
            submitter,
            submitted_rx,
            bitcoind: Arc::clone(&self.bitcoind),
            handle,
            runner,
            _db: db,
        }
    }

    pub(crate) fn initiator_signed_instant_refund_tx_hex(
        &self,
        htlc_params: &HTLCParams,
        utxos: Vec<CoverUtxo>,
        recipient: bitcoin::Address,
        initiator_wallet: &BitcoinWallet,
    ) -> String {
        let utxo = utxos.into_iter().next().expect("instant refund utxo");
        let placeholder_sig = [0_u8; 65];
        let mut tx = Transaction {
            version: bitcoin::transaction::Version::TWO,
            lock_time: bitcoin::absolute::LockTime::ZERO,
            input: vec![bitcoin::TxIn {
                previous_output: utxo.outpoint,
                script_sig: bitcoin::ScriptBuf::new(),
                sequence: Sequence::MAX,
                witness: get_instant_refund_witness(
                    htlc_params,
                    &placeholder_sig,
                    &placeholder_sig,
                )
                .expect("instant refund witness"),
            }],
            output: vec![bitcoin::TxOut {
                value: bitcoin::Amount::from_sat(htlc_params.amount),
                script_pubkey: recipient.script_pubkey(),
            }],
        };
        let sighash = generate_instant_refund_hash(
            &tars::bitcoin::HTLCParams {
                initiator_pubkey: htlc_params.initiator_pubkey,
                redeemer_pubkey: htlc_params.redeemer_pubkey,
                amount: htlc_params.amount,
                secret_hash: htlc_params.secret_hash,
                timelock: htlc_params.timelock,
            },
            &[TarsUtxo {
                txid: utxo.outpoint.txid,
                vout: utxo.outpoint.vout,
                value: utxo.value,
                status: TarsUtxoStatus {
                    confirmed: true,
                    block_height: Some(0),
                },
            }],
            &recipient,
            NETWORK,
            Some(0),
        )
        .expect("instant refund sighash")
        .into_iter()
        .next()
        .expect("single instant refund sighash");

        let initiator_signature = initiator_wallet
            .sign_taproot_script_spend(&sighash, TapSighashType::SinglePlusAnyoneCanPay)
            .expect("initiator signature")
            .serialize();
        tx.input[0].witness = get_instant_refund_witness(
            htlc_params,
            &initiator_signature,
            &placeholder_sig,
        )
        .expect("initiator signed instant refund witness");

        bitcoin::consensus::encode::serialize_hex(&tx)
    }

    pub(crate) async fn fund_htlc(
        &self,
        wallet: Arc<BitcoinWallet>,
        params: &HTLCParams,
        fee_rate: f64,
    ) -> (Txid, Vec<CoverUtxo>) {
        let addr = get_htlc_address(params, NETWORK).expect("htlc address");
        let tx = self
            .build_with_fee_builder(
                wallet,
                BitcoinTxAdaptorParams {
                    sacps: vec![],
                    spends: vec![],
                    sends: vec![SendRequest {
                        address: addr.clone(),
                        amount: params.amount,
                    }],
                    fee_rate,
                },
            )
            .await;
        Self::assert_htlc_output(&tx, &addr, params.amount);
        let txid = self.broadcast_and_confirm("funding", &tx).await;
        let utxos = self.wait_for_confirmed_cover_utxo_count(&addr, 1).await;
        assert!(
            utxos.iter().any(|u| u.outpoint.txid == txid),
            "funding must leave HTLC UTXO"
        );
        (txid, utxos)
    }

    pub(crate) async fn build_send_from_wallet_utxo(
        &self,
        wallet: Arc<BitcoinWallet>,
        utxo: CoverUtxo,
        recipient: bitcoin::Address,
        value: u64,
        fee_rate: f64,
    ) -> Transaction {
        let adaptor = BitcoinTxAdaptor::new(Arc::clone(&wallet), NETWORK);
        let mut cover = BitcoinCoverUtxoProvider::new(
            wallet.address().clone(),
            Arc::clone(&self.electrs),
            vec![],
            None,
            std::iter::empty(),
        );
        cover.add(vec![utxo]);

        fee_builder::build_with_fee(
            &adaptor,
            &BitcoinTxAdaptorParams {
                sacps: vec![],
                spends: vec![],
                sends: vec![SendRequest {
                    address: recipient,
                    amount: value,
                }],
                fee_rate,
            },
            &mut cover,
        )
        .await
        .expect("build send from known wallet utxo")
        .tx
    }

    pub(crate) fn address_output_utxo(
        &self,
        tx: &Transaction,
        txid: Txid,
        address: &bitcoin::Address,
    ) -> CoverUtxo {
        let (vout, output) = tx
            .output
            .iter()
            .enumerate()
            .find(|(_, output)| output.script_pubkey == address.script_pubkey())
            .expect("output for address");

        CoverUtxo {
            outpoint: OutPoint {
                txid,
                vout: vout as u32,
            },
            value: output.value.to_sat(),
            script_pubkey: output.script_pubkey.clone(),
        }
    }

    fn assert_htlc_output(tx: &Transaction, addr: &bitcoin::Address, amount: u64) {
        assert!(
            tx.output
                .iter()
                .any(|o| o.script_pubkey == addr.script_pubkey() && o.value.to_sat() == amount),
            "funding tx must create the HTLC output"
        );
    }

    pub(crate) async fn assert_htlc_consumed(&self, address: &bitcoin::Address) {
        let remaining = self.wait_for_confirmed_cover_utxo_count(address, 0).await;
        assert!(remaining.is_empty(), "HTLC should be fully consumed");
    }

    pub(crate) async fn tx_from_chain(&self, txid: &Txid) -> Transaction {
        let hex = self
            .electrs
            .get_tx_hex(&txid.to_string())
            .await
            .expect("tx hex");
        bitcoin::consensus::deserialize(&hex::decode(hex).expect("decode hex"))
            .expect("deserialize tx")
    }
}

async fn tx_from_node(bitcoind: &BitcoindRpcClient, txid: &Txid) -> Transaction {
    let response = reqwest::Client::new()
        .post(BITCOIND_URL)
        .basic_auth(BITCOIND_USER, Some(BITCOIND_PASS))
        .json(&serde_json::json!({
            "jsonrpc": "1.0",
            "id": "bitcoin_regtest",
            "method": "getrawtransaction",
            "params": [txid.to_string(), false],
        }))
        .send()
        .await
        .expect("getrawtransaction request")
        .json::<serde_json::Value>()
        .await
        .expect("getrawtransaction response");

    if !response["error"].is_null() {
        panic!("getrawtransaction failed for {txid}: {}", response["error"]);
    }

    let hex = response["result"]
        .as_str()
        .expect("raw tx hex string")
        .to_string();
    let _ = bitcoind;
    bitcoin::consensus::deserialize(&hex::decode(hex).expect("decode raw tx hex"))
        .expect("deserialize node tx")
}

pub(crate) fn unit_test_htlc_params(secret: &[u8], amount: u64, timelock: u64) -> HTLCParams {
    let secp = Secp256k1::new();
    let (x1, _) =
        bitcoin::secp256k1::Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng())
            .x_only_public_key();
    let (x2, _) =
        bitcoin::secp256k1::Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng())
            .x_only_public_key();
    HTLCParams {
        initiator_pubkey: x1,
        redeemer_pubkey: x2,
        amount,
        secret_hash: Sha256::digest(secret).into(),
        timelock,
    }
}
