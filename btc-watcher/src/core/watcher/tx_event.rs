use crate::adapters::MokaCacheAdaptor;
use crate::core::{
    AddressScreener, BlockchainIndexer, Cache, OrderSecret, Swap, SwapEvent, SwapEventType, TxInfo,
};
use bitcoin::{Network, Transaction, TxIn, hashes::Hash, key::TapTweak, taproot};
use tars::bitcoin::TransactionMetadata;
use screener::client::ScreenerRequest;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tracing::info;

const TX_NOT_FOUND_ERROR: &str = "Transaction not found";
const GET_TX_RETRY_DELAY: Duration = Duration::from_millis(256);
const GET_TX_MAX_RETRIES: u32 = 32;
const GET_TX_CACHE_TTL: Duration = Duration::from_secs(5 * 60); // 5 minutes


pub struct TxEventParser {
    pub screener_client: Arc<dyn AddressScreener>,
    pub indexer: Arc<dyn BlockchainIndexer>,
    pub swap_cache: Arc<dyn Cache<String, Swap> + Send + Sync>,
    pub chain: String,
    pub network: Network,
    get_tx_cache: Arc<dyn Cache<String, TransactionMetadata> + Send + Sync>,
}

impl TxEventParser {
    pub fn new(
        screener_client: Arc<dyn AddressScreener>,
        indexer: Arc<dyn BlockchainIndexer>,
        swap_cache: Arc<dyn Cache<String, Swap> + Send + Sync>,
        chain: String,
        network: Network,
    ) -> Self {
        let get_tx_cache: Arc<dyn Cache<String, TransactionMetadata> + Send + Sync> =
            Arc::new(MokaCacheAdaptor::with_ttl(GET_TX_CACHE_TTL));
        Self {
            screener_client,
            indexer,
            swap_cache,
            get_tx_cache,
            chain,
            network,
        }
    }

    pub async fn parse_swap_events(
        &self,
        tx: Transaction,
        block_height: u64,
        block_ts: Option<chrono::DateTime<chrono::Utc>>,
        detected_ts: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Vec<SwapEvent> {
        if tx.is_coinbase() {
            return vec![];
        }

        let mut events = Vec::new();
        events.extend(
            self.process_deposits_in_tx(&tx, block_height, block_ts, detected_ts)
                .await,
        );
        events.extend(
            self.process_withdraws_in_tx(&tx, block_height, block_ts, detected_ts)
                .await,
        );
        events
    }

    /// Collects HTLC inits in a transaction that match swaps in the cache.
    async fn collect_htlc_deposits<'a>(
        &self,
        tx: &'a Transaction,
    ) -> Vec<(Swap, &'a bitcoin::TxOut)> {
        let mut matched = Vec::new();
        for output in tx.output.iter() {
            let address = match bitcoin::Address::from_script(&output.script_pubkey, self.network) {
                Ok(addr) => addr.to_string(),
                Err(_) => continue,
            };
            if let Some(swap) = self.swap_cache.get(&address).await
                && swap.amount == output.value.to_sat() as i64
            {
                matched.push((swap, output));
            }
        }
        matched
    }

    async fn get_tx_with_retry(&self, tx_id: &str) -> eyre::Result<TransactionMetadata> {
        if let Some(cached) = self.get_tx_cache.get(&tx_id.to_string()).await {
            return Ok(cached);
        }

        let mut delay = GET_TX_RETRY_DELAY;
        for attempt in 1..=GET_TX_MAX_RETRIES {
            match self.indexer.get_tx(tx_id).await {
                Ok(metadata) => {
                    self.get_tx_cache
                        .set(&[(tx_id.to_string(), metadata.clone())])
                        .await;
                    return Ok(metadata);
                }
                Err(e) if e.to_string().contains(TX_NOT_FOUND_ERROR) => {
                    tracing::warn!(tx_id = %tx_id, attempt, max = GET_TX_MAX_RETRIES, delay_ms = delay.as_millis(), "Transaction not found, retrying...");
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_secs(2));
                }
                Err(e) => return Err(e),
            }
        }

        Err(eyre::eyre!(
            "Transaction {tx_id} not found after {GET_TX_MAX_RETRIES} retries"
        ))
    }

    /// Returns `true` if any of the tx's input addresses are blacklisted.
    async fn has_blacklisted_inputs(&self, tx_id: &str) -> eyre::Result<bool> {
        // retry getting tx metadata if not found
        let tx_metadata = self.get_tx_with_retry(tx_id).await?;

        let addrs = tx_metadata
            .vin
            .iter()
            .map(|vin| ScreenerRequest {
                address: vin.prevout.script_pubkey_address.clone(),
                chain: self.chain.clone(),
            })
            .collect::<Vec<_>>();

        let responses = self.screener_client.is_blacklisted(addrs).await?;
        Ok(responses.iter().any(|r| r.is_blacklisted))
    }

    /// Processes transaction outputs to find HTLC initiations that match swaps in the cache.
    /// Returns a vector of SwapEvents for each matching initiation found.
    async fn process_deposits_in_tx(
        &self,
        tx: &Transaction,
        block_height: u64,
        block_ts: Option<chrono::DateTime<chrono::Utc>>,
        detected_ts: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Vec<SwapEvent> {
        let tx_id = tx.compute_txid().to_string();

        let deposits = self.collect_htlc_deposits(tx).await;
        if deposits.is_empty() {
            return vec![];
        }

        match self.has_blacklisted_inputs(&tx_id).await {
            Ok(true) => {
                tracing::info!(tx_id = %tx_id, "TX has inputs from a blacklisted address");
                return deposits
                    .iter()
                    .map(|(swap, _)| SwapEvent {
                        event_type: SwapEventType::Initiate,
                        swap_id: swap.swap_id.clone(),
                        amount: swap.amount,
                        tx_info: TxInfo {
                            tx_hash: tx_id.clone(),
                            block_number: block_height as i64,
                            block_timestamp: block_ts,
                            detected_timestamp: detected_ts,
                        },
                        is_blacklisted: true,
                    })
                    .collect();
            }
            Err(e) => {
                tracing::error!(tx_id = %tx_id, "Failed to screen tx inputs: {e}");
                return vec![];
            }
            _ => {}
        }

        deposits
            .into_iter()
            .map(|(swap, output)| {
                let filled_amount = if block_height > 0 {
                    output.value.to_sat() as i64
                } else {
                    0
                };

                info!(event_type = ?SwapEventType::Initiate, swap_id = %swap.swap_id, amount = %filled_amount, tx_id = %tx_id, block_height = %block_height, block_ts = ?block_ts, detected_ts = ?detected_ts, "Initiate event");

                SwapEvent {
                    event_type: SwapEventType::Initiate,
                    swap_id: swap.swap_id,
                    amount: filled_amount,
                    tx_info: TxInfo {
                        tx_hash: format!("{}:{}", tx_id, block_height),
                        block_number: block_height as i64,
                        block_timestamp: block_ts,
                        detected_timestamp: detected_ts,
                    },
                    is_blacklisted: false,
                }
            })
            .collect()
    }

    /// Processes transaction inputs to find HTLC redeems/refunds that match swaps in the cache.
    /// Returns a vector of SwapEvents for each matching redeem or refund found.
    async fn process_withdraws_in_tx(
        &self,
        tx: &Transaction,
        block_height: u64,
        block_ts: Option<chrono::DateTime<chrono::Utc>>,
        detected_ts: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Vec<SwapEvent> {
        let tx_id = tx.compute_txid().to_string();
        let mut events = Vec::new();

        for input in tx.input.iter() {
            // Skip coinbase inputs
            if input.previous_output.txid == bitcoin::Txid::all_zeros() {
                continue;
            }

            // Get taproot address from input
            let address = match get_taproot_address(input, self.network) {
                Some(addr) => addr,
                None => continue,
            };

            // Check if this address corresponds to a pending swap
            let swap = match self.swap_cache.get(&address.to_string()).await {
                Some(s) => s,
                None => continue,
            };


            let event_type = match get_htlc_spend_type(input) {
                Some(event_type) => event_type,
                None => continue,
            };

            info!(event_type = ?event_type, swap_id = %swap.swap_id, amount = %swap.amount, tx_id = %tx_id, block_height = %block_height, block_ts = ?block_ts, detected_ts = ?detected_ts, "Fill event");

            events.push(SwapEvent {
                event_type,
                swap_id: swap.swap_id,
                amount: swap.amount,
                tx_info: TxInfo {
                    tx_hash: format!("{}:{}", tx_id, block_height),
                    block_number: block_height as i64,
                    block_timestamp: block_ts,
                    detected_timestamp: detected_ts,
                },
                is_blacklisted: false,
            });
        }

        events
    }
}

pub fn get_taproot_address(
    input: &bitcoin::TxIn,
    network: bitcoin::Network,
) -> Option<bitcoin::Address> {
    let cblock = match input.witness.taproot_control_block() {
        Some(cblock) => cblock,
        None => {
            return None;
        }
    };
    let tapleaf = match input.witness.taproot_leaf_script() {
        Some(script) => script,
        None => {
            return None;
        }
    };

    let control_block = match bitcoin::taproot::ControlBlock::decode(cblock) {
        Ok(cblock) => cblock,
        Err(_) => {
            return None;
        }
    };
    let merkle_branch_len = control_block.merkle_branch.len();

    if merkle_branch_len < 1 {
        return None;
    }

    let revealing_leaf_hash =
        taproot::TapNodeHash::from_script(tapleaf.script, control_block.leaf_version);
    let mut root_hash;

    if control_block.merkle_branch.len() == 1 {
        root_hash = taproot::TapNodeHash::from_node_hashes(
            revealing_leaf_hash,
            control_block.merkle_branch[0],
        );
    } else if control_block.merkle_branch.len() == 2 {
        root_hash = taproot::TapNodeHash::from_node_hashes(
            revealing_leaf_hash,
            control_block.merkle_branch[0],
        );
        root_hash =
            taproot::TapNodeHash::from_node_hashes(control_block.merkle_branch[1], root_hash);
    } else {
        return None;
    }
    let secp = bitcoin::secp256k1::Secp256k1::new();

    let (output_key, _) = control_block.internal_key.tap_tweak(&secp, Some(root_hash));

    let address = bitcoin::Address::p2tr_tweaked(output_key, network);
    Some(address)
}

/// Determines the HTLC spend type from the witness data of a transaction input.
fn get_htlc_spend_type(input: &TxIn) -> Option<SwapEventType> {
    match input.witness.len() {
        4 => {
            // In this case, we distinguish between a Redeem and Refund via the length of two witness elements
            let is_redeem = input.witness[0].len() != input.witness[1].len();
            if is_redeem {
                let secret_bytes = &input.witness[1];
                let secret_hex = hex::encode(secret_bytes);
                OrderSecret::new(secret_hex).map(SwapEventType::Redeem).ok()
            } else {
                // Instant Refund
                Some(SwapEventType::Refund)
            }
        }
        // Refund
        3 => Some(SwapEventType::Refund),
        _ => None,
    }
}

pub fn remove_duplicates(events: &[SwapEvent]) -> Vec<SwapEvent> {
    let mut seen = HashSet::new();
    events
        .iter()
        .filter(|&e| seen.insert(e.clone())) // insert returns false if already present
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{FixedStatusScreener, GardenBitcoinIndexer, SwapCache};
    use bitcoin::{Address, key::Secp256k1};
    use tars::{
        bitcoin::{
            FeeRate, InstantRefundSignatures, ValidSpendRequests,
            batcher::{batch_tx::build_batch_tx, sign::sign_batch_tx},
            build_instant_refund_sacp, fund_btc, generate_instant_refund_hash, get_htlc_address,
            test_utils::{
                TEST_FEE_RATE, TEST_NETWORK, generate_bitcoin_random_keypair,
                get_test_bitcoin_htlc, get_test_bitcoin_indexer, get_test_htlc_params,
                get_test_spend_request,
            },
        },
        utils::gen_secret,
    };
    use std::time::Duration;
    use tokio::time::sleep;

    const UNIT_BTC: i64 = 100_000_000;
    const TEST_SECRET_HASH: &str =
        "c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6";
    const TEST_INDEXER_URL: &str = "http://localhost:30000";
    const TEST_CHAIN: &str = "bitcoin";

    // ============ Test Helpers ============
    fn new_swap_cache() -> Arc<SwapCache> {
        Arc::new(SwapCache::new())
    }

    fn get_test_core_bitcoin_indexer() -> Arc<dyn BlockchainIndexer> {
        Arc::new(GardenBitcoinIndexer::new(TEST_INDEXER_URL.to_string()).unwrap())
    }

    async fn setup_cache_with_swaps(addresses: &[String], amount: i64) -> Arc<SwapCache> {
        let cache = new_swap_cache();
        let kv_pairs: Vec<_> = addresses
            .iter()
            .map(|addr| {
                let swap = Swap {
                    swap_id: addr.clone(),
                    amount,
                };
                (addr.clone(), swap)
            })
            .collect();
        cache.set(&kv_pairs).await;
        cache
    }

    fn assert_event_tx_info(event: &SwapEvent, tx: &Transaction, block_number: i64) {
        assert!(
            event
                .tx_info
                .tx_hash
                .contains(&tx.compute_txid().to_string())
        );
        assert_eq!(event.tx_info.block_number, block_number);
    }

    fn assert_redeem_secret(event: &SwapEvent, expected_secret: &str) {
        if let SwapEventType::Redeem(ref s) = event.event_type {
            assert_eq!(s.as_str(), expected_secret);
        } else {
            panic!("Expected Redeem event, got {:?}", event.event_type);
        }
    }

    // ============ Transaction Builders ============
    async fn make_htlc_initiate_tx() -> (String, Transaction) {
        let indexer = get_test_bitcoin_indexer().unwrap();
        let (initiator_pubkey, redeemer_pubkey) = generate_keypair_pubkeys();
        let (_, secret_hash) = gen_secret();

        let htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());
        let htlc_address = get_htlc_address(&htlc_params, TEST_NETWORK).unwrap();

        fund_btc(&htlc_address, &indexer).await.unwrap();

        let utxos = indexer.get_utxos(&htlc_address).await.unwrap();
        let tx = indexer
            .get_tx_hex(&utxos.first().unwrap().txid.to_string())
            .await
            .unwrap();
        (htlc_address.to_string(), tx)
    }

    async fn make_batch_redeem_tx(count: usize) -> (Vec<String>, Transaction, Vec<String>) {
        let spend_requests =
            futures::future::try_join_all((0..count).map(|_| get_test_spend_request()))
                .await
                .unwrap();

        let htlc_addresses: Vec<_> = spend_requests.iter().map(|s| s.id.clone()).collect();
        let secrets: Vec<_> = spend_requests
            .iter()
            .map(|s| hex::encode(&s.witness[1]))
            .collect();

        let indexer = get_test_bitcoin_indexer().unwrap();
        let validated = ValidSpendRequests::validate(spend_requests, &indexer)
            .await
            .unwrap();

        let mut tx = build_batch_tx(&validated, FeeRate::new(TEST_FEE_RATE).unwrap()).unwrap();
        sign_batch_tx(&mut tx, &validated).unwrap();

        (htlc_addresses, tx, secrets)
    }

    async fn make_htlc_refund_tx() -> (String, Transaction) {
        let secp = Secp256k1::new();
        let indexer = get_test_bitcoin_indexer().unwrap();

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();
        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let (_, secret_hash) = gen_secret();
        let mut htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());
        htlc_params.timelock = 0; // Bypass timelock for testing

        let htlc_address = get_htlc_address(&htlc_params, TEST_NETWORK).unwrap();
        fund_btc(&htlc_address, &indexer).await.unwrap();

        let utxos = indexer.get_utxos(&htlc_address).await.unwrap();
        assert!(!utxos.is_empty(), "No UTXOs found for HTLC address");

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, TEST_NETWORK);
        sleep(Duration::from_secs(10)).await;

        let bitcoin_htlc = get_test_bitcoin_htlc(initiator_key_pair).await.unwrap();
        let refund_txid = bitcoin_htlc.refund(&htlc_params, &recipient).await.unwrap();
        let tx = indexer.get_tx_hex(&refund_txid.to_string()).await.unwrap();

        (htlc_address.to_string(), tx)
    }

    async fn make_instant_refund_tx() -> (String, Transaction) {
        let secp = Secp256k1::new();
        let indexer = get_test_bitcoin_indexer().unwrap();

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();
        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(&hex::decode(TEST_SECRET_HASH).unwrap());

        let htlc_params = get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash);
        let htlc_address = get_htlc_address(&htlc_params, Network::Regtest).unwrap();

        fund_btc(&htlc_address, &indexer).await.unwrap();
        let utxos = indexer.get_utxos(&htlc_address).await.unwrap();
        assert!(!utxos.is_empty(), "No UTXOs found for HTLC address");

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, Network::Regtest);
        const TEST_FEE: u64 = 1000;

        let hashes = generate_instant_refund_hash(
            &htlc_params,
            &utxos,
            &recipient,
            Network::Regtest,
            Some(TEST_FEE),
        )
        .unwrap();

        let signatures: Vec<InstantRefundSignatures> = hashes
            .iter()
            .map(|hash| {
                let message = bitcoin::secp256k1::Message::from_digest_slice(hash).unwrap();
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
                .unwrap();

        (htlc_address.to_string(), tx)
    }

    fn generate_keypair_pubkeys() -> (bitcoin::XOnlyPublicKey, bitcoin::XOnlyPublicKey) {
        let initiator = generate_bitcoin_random_keypair().x_only_public_key().0;
        let redeemer = generate_bitcoin_random_keypair().x_only_public_key().0;
        (initiator, redeemer)
    }

    // ============ Test Helpers ============
    fn new_processor(cache: Arc<SwapCache>) -> TxEventParser {
        TxEventParser::new(
            Arc::new(FixedStatusScreener::new(false)),
            get_test_core_bitcoin_indexer(),
            cache,
            TEST_CHAIN.to_string(),
            Network::Regtest,
        )
    }

    // ============ Tests ============
    #[tokio::test]
    #[serial_test::serial]
    async fn should_detect_init_event() {
        let (htlc_address, tx) = make_htlc_initiate_tx().await;
        let cache = setup_cache_with_swaps(std::slice::from_ref(&htlc_address), UNIT_BTC).await;
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx.clone(), 2, None, None).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, SwapEventType::Initiate);
        assert_eq!(events[0].swap_id, htlc_address);
        assert_eq!(events[0].amount, UNIT_BTC);
        assert_event_tx_info(&events[0], &tx, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_not_detect_init_with_mismatched_amount() {
        let (htlc_address, tx) = make_htlc_initiate_tx().await;
        let cache = setup_cache_with_swaps(&[htlc_address], UNIT_BTC + 1).await;
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx, 0, None, None).await;

        assert_eq!(events.len(), 0);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_not_detect_init_when_not_in_cache() {
        let (_, tx) = make_htlc_initiate_tx().await;
        let cache = new_swap_cache();
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx, 0, None, None).await;

        assert_eq!(events.len(), 0);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_detect_single_redeem_event() {
        let (htlc_addresses, tx, secrets) = make_batch_redeem_tx(1).await;
        let cache = setup_cache_with_swaps(&htlc_addresses, UNIT_BTC).await;
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx.clone(), 0, None, None).await;

        assert_eq!(events.len(), 1);
        assert_redeem_secret(&events[0], &secrets[0]);
        assert_eq!(events[0].swap_id, htlc_addresses[0]);
        assert_eq!(events[0].amount, UNIT_BTC);
        assert_event_tx_info(&events[0], &tx, 0);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_detect_batched_redeem_events() {
        let (htlc_addresses, tx, secrets) = make_batch_redeem_tx(10).await;
        let cache = setup_cache_with_swaps(&htlc_addresses, UNIT_BTC).await;
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx.clone(), 0, None, None).await;

        assert_eq!(events.len(), 10);
        for (event, secret) in events.iter().zip(secrets.iter()) {
            assert_redeem_secret(event, secret);
            assert!(htlc_addresses.contains(&event.swap_id));
            assert_eq!(event.amount, UNIT_BTC);
            assert_event_tx_info(event, &tx, 0);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_refund_event() {
        let (htlc_address, tx) = make_htlc_refund_tx().await;
        let cache = setup_cache_with_swaps(std::slice::from_ref(&htlc_address), UNIT_BTC).await;
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx.clone(), 2, None, None).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, SwapEventType::Refund);
        assert_eq!(events[0].swap_id, htlc_address);
        assert_eq!(events[0].amount, UNIT_BTC);
        assert_event_tx_info(&events[0], &tx, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_detect_instant_refund_event() {
        let (htlc_address, tx) = make_instant_refund_tx().await;
        let cache = setup_cache_with_swaps(std::slice::from_ref(&htlc_address), UNIT_BTC).await;
        let processor = new_processor(cache);
        let events = processor.parse_swap_events(tx.clone(), 2, None, None).await;

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_type, SwapEventType::Refund);
        assert_eq!(events[0].swap_id, htlc_address);
        assert_eq!(events[0].amount, UNIT_BTC);
        assert_event_tx_info(&events[0], &tx, 2);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn should_remove_duplicate_events() {
        let event = SwapEvent {
            event_type: SwapEventType::Initiate,
            swap_id: "swap1".to_string(),
            amount: 100,
            tx_info: TxInfo {
                tx_hash: "tx1".to_string(),
                block_number: 1,
                block_timestamp: None,
                detected_timestamp: None,
            },
            is_blacklisted: false,
        };
        let events = vec![event.clone(), event];

        let unique = remove_duplicates(&events);

        assert_eq!(unique.len(), 1);
        assert_eq!(unique[0].swap_id, "swap1");
        assert_eq!(unique[0].amount, 100);
        assert_eq!(unique[0].tx_info.tx_hash, "tx1");
        assert_eq!(unique[0].tx_info.block_number, 1);
    }
}
