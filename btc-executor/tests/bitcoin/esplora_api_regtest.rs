//! Regtest integration tests validating Esplora API assumptions used by the
//! Bitcoin confirmed-tx reconciler.
//!
//! These tests ensure that the Esplora `/address/:addr/txs/chain` endpoint
//! behaves as the reconciler expects:
//! - Returns confirmed txs newest-first (descending block height)
//! - Returns at most 25 txs per page
//! - Pagination via `last_seen_txid` returns the next batch after that txid
//! - A final page with < 25 results means no more pages exist
//! - `EsploraTx.vin[].prevout` contains `scriptpubkey_address` and `value`

use serial_test::serial;
use std::time::Duration;

use btc_executor::infrastructure::chain::bitcoin::clients::BitcoinClientError;

use super::common::BitcoinTestEnv;

/// Send BTC from the regtest default wallet to `target_address` via bitcoind RPC.
/// Returns the txid as a string. Does NOT mine a block.
async fn send_to_address(env: &BitcoinTestEnv, target_address: &str, sats: u64) -> String {
    let btc_amount = sats as f64 / 100_000_000.0;
    let result = env
        .bitcoind_rpc(
            "sendtoaddress",
            serde_json::json!([target_address, btc_amount]),
        )
        .await;
    result
        .as_str()
        .expect("sendtoaddress should return txid string")
        .to_string()
}

/// Fetch the full confirmed tx chain via Esplora pagination.
async fn fetch_confirmed_txs_chain(
    env: &BitcoinTestEnv,
    address: &str,
) -> Result<Vec<String>, BitcoinClientError> {
    let mut txids = Vec::new();
    let mut last_seen_txid: Option<String> = None;

    loop {
        let page = env
            .electrs
            .get_confirmed_address_txs_chain(address, last_seen_txid.as_deref())
            .await?;

        if page.is_empty() {
            break;
        }

        last_seen_txid = page.last().map(|tx| tx.txid.clone());
        txids.extend(page.iter().map(|tx| tx.txid.clone()));

        if page.len() < 25 {
            break;
        }
    }

    Ok(txids)
}

/// Wait for electrs to index all confirmed txs. Polls until the expected count
/// of confirmed txs is visible across the full paginated Esplora response.
async fn wait_for_electrs_indexing(env: &BitcoinTestEnv, address: &str, expected_total: usize) {
    for _ in 0..40 {
        if let Ok(txids) = fetch_confirmed_txs_chain(env, address).await
            && txids.len() >= expected_total
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("electrs did not index {expected_total} confirmed txs for {address} within timeout");
}

/// Generate `count` confirmed txs to `target_address` by sending from the
/// miner and mining one block per tx. Returns txids in mine order (oldest first).
async fn generate_confirmed_txs_to_address(
    env: &BitcoinTestEnv,
    target_address: &str,
    count: usize,
) -> Vec<String> {
    let mut txids = Vec::with_capacity(count);

    for _ in 0..count {
        let txid = send_to_address(env, target_address, 10_000).await;
        txids.push(txid);
        env.mine_blocks(1).await;
    }

    wait_for_electrs_indexing(env, target_address, count).await;

    txids
}

#[serial]
#[tokio::test]
async fn esplora_txs_chain_returns_newest_first() {
    let env = BitcoinTestEnv::new().await;
    let wallet = BitcoinTestEnv::random_wallet();
    let address = wallet.address().to_string();

    // Generate 5 confirmed txs
    let mined_txids = generate_confirmed_txs_to_address(&env, &address, 5).await;

    let txs = env
        .electrs
        .get_confirmed_address_txs_chain(&address, None)
        .await
        .expect("fetch confirmed txs");

    assert!(txs.len() >= 5, "expected at least 5 txs, got {}", txs.len());

    // Verify newest-first ordering: block heights must be descending
    for window in txs.windows(2) {
        let height_a = window[0]
            .status
            .block_height
            .expect("confirmed tx should have block_height");
        let height_b = window[1]
            .status
            .block_height
            .expect("confirmed tx should have block_height");
        assert!(
            height_a >= height_b,
            "expected descending block heights, got {height_a} before {height_b}"
        );
    }

    // The last mined tx should be first in the response
    assert_eq!(
        txs[0].txid,
        mined_txids[mined_txids.len() - 1],
        "newest mined tx should be first in response"
    );
}

#[serial]
#[tokio::test]
async fn esplora_txs_chain_page_size_is_25() {
    let env = BitcoinTestEnv::new().await;
    let wallet = BitcoinTestEnv::random_wallet();
    let address = wallet.address().to_string();

    // Generate 30 confirmed txs (more than one page)
    generate_confirmed_txs_to_address(&env, &address, 30).await;

    let first_page = env
        .electrs
        .get_confirmed_address_txs_chain(&address, None)
        .await
        .expect("fetch first page");

    assert_eq!(
        first_page.len(),
        25,
        "first page should contain exactly 25 txs, got {}",
        first_page.len()
    );
}

#[serial]
#[tokio::test]
async fn esplora_txs_chain_pagination_returns_next_batch_after_last_seen() {
    let env = BitcoinTestEnv::new().await;
    let wallet = BitcoinTestEnv::random_wallet();
    let address = wallet.address().to_string();

    // Generate 30 confirmed txs
    let mined_txids = generate_confirmed_txs_to_address(&env, &address, 30).await;

    let first_page = env
        .electrs
        .get_confirmed_address_txs_chain(&address, None)
        .await
        .expect("fetch first page");

    assert_eq!(first_page.len(), 25);

    // Use the last txid from page 1 as the cursor for page 2
    let last_txid_page1 = &first_page[24].txid;
    let second_page = env
        .electrs
        .get_confirmed_address_txs_chain(&address, Some(last_txid_page1))
        .await
        .expect("fetch second page");

    // Should have the remaining txs (< 25)
    assert!(
        second_page.len() < 25,
        "second page should have < 25 txs (final page), got {}",
        second_page.len()
    );
    assert!(
        !second_page.is_empty(),
        "second page should not be empty — we have 30 txs total"
    );

    // No txid from page 2 should appear in page 1
    let page1_txids: std::collections::HashSet<&str> =
        first_page.iter().map(|tx| tx.txid.as_str()).collect();
    for tx in &second_page {
        assert!(
            !page1_txids.contains(tx.txid.as_str()),
            "page 2 txid {} should not appear in page 1",
            tx.txid
        );
    }

    // Combined pages should cover all 30 mined txids
    let all_fetched_txids: std::collections::HashSet<&str> = first_page
        .iter()
        .chain(second_page.iter())
        .map(|tx| tx.txid.as_str())
        .collect();
    for txid in &mined_txids {
        assert!(
            all_fetched_txids.contains(txid.as_str()),
            "mined txid {txid} should appear in paginated results"
        );
    }
}

#[serial]
#[tokio::test]
async fn esplora_txs_chain_final_page_has_fewer_than_25() {
    let env = BitcoinTestEnv::new().await;
    let wallet = BitcoinTestEnv::random_wallet();
    let address = wallet.address().to_string();

    // Generate exactly 10 txs (fits in one page)
    generate_confirmed_txs_to_address(&env, &address, 10).await;

    let page = env
        .electrs
        .get_confirmed_address_txs_chain(&address, None)
        .await
        .expect("fetch single page");

    assert_eq!(
        page.len(),
        10,
        "single page with 10 txs should return exactly 10, got {}",
        page.len()
    );

    // Paginating past the last tx should return empty
    let last_txid = &page[page.len() - 1].txid;
    let next_page = env
        .electrs
        .get_confirmed_address_txs_chain(&address, Some(last_txid))
        .await
        .expect("fetch page after last tx");

    assert!(
        next_page.is_empty(),
        "page after last tx should be empty, got {} txs",
        next_page.len()
    );
}

#[serial]
#[tokio::test]
async fn esplora_tx_vin_prevout_contains_address_and_value() {
    let env = BitcoinTestEnv::new().await;
    let wallet = env.funded_random_wallet().await;
    let recipient = BitcoinTestEnv::random_wallet();

    // Send from our wallet to recipient — this creates a tx where vin has our prevout
    let utxos = env.get_confirmed_cover_utxos(wallet.address()).await;
    assert!(!utxos.is_empty(), "wallet should have funded utxos");

    let tx = env
        .build_send_from_wallet_utxo(
            wallet.clone(),
            utxos[0].clone(),
            recipient.address().clone(),
            5_000,
            2.0,
        )
        .await;
    let txid = env.broadcast_and_confirm("prevout-test", &tx).await;

    // Wait for electrs to index
    wait_for_electrs_indexing(&env, &recipient.address().to_string(), 1).await;

    // Fetch the tx via Esplora and verify prevout data
    let esplora_tx = env
        .electrs
        .get_tx(&txid.to_string())
        .await
        .expect("fetch tx from esplora");

    // At least one vin should have a prevout with our wallet's address
    let wallet_addr = wallet.address().to_string();
    let has_our_prevout = esplora_tx.vin.iter().any(|vin| {
        vin.prevout
            .as_ref()
            .and_then(|p| p.scriptpubkey_address.as_deref())
            == Some(&wallet_addr)
    });
    assert!(
        has_our_prevout,
        "at least one vin.prevout.scriptpubkey_address should be our wallet address"
    );

    // Prevout value should be positive
    for vin in &esplora_tx.vin {
        if let Some(prevout) = &vin.prevout {
            assert!(
                prevout.value > 0,
                "prevout.value should be positive, got {}",
                prevout.value
            );
        }
    }
}
