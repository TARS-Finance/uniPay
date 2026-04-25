use crate::core::{OrderSecret, Swap, SwapEvent, SwapEventType, SwapStore, TxInfo, Vec1};
use chrono::Utc;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(Debug, serde::Deserialize)]
struct EsploraTx {
    txid: String,
    status: EsploraStatus,
    vin: Vec<EsploraVin>,
    vout: Vec<EsploraVout>,
}

#[derive(Debug, serde::Deserialize)]
struct EsploraStatus {
    confirmed: bool,
    block_height: Option<i64>,
    block_time: Option<i64>,
}

#[derive(Debug, serde::Deserialize)]
struct EsploraVin {
    prevout: Option<EsploraVout>,
    witness: Option<Vec<String>>, // hex-encoded witness stack items
}

#[derive(Debug, serde::Deserialize)]
struct EsploraVout {
    scriptpubkey_address: Option<String>,
    value: u64, // satoshis
}

/// Periodically polls the esplora API for all pending swap addresses and emits
/// Initiate/Redeem/Refund events for any on-chain activity that was missed.
/// Runs every 10 seconds as a background task.
pub async fn poll_pending_swap_addresses(
    chain: String,
    indexer_url: String,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
) {
    let client = reqwest::Client::new();
    loop {
        sleep(POLL_INTERVAL).await;
        if let Err(e) = poll_once(&client, &chain, &indexer_url, swap_store.clone()).await {
            tracing::warn!(error = %e, "address_poller: poll cycle failed");
        }
    }
}

async fn poll_once(
    client: &reqwest::Client,
    chain: &str,
    indexer_url: &str,
    swap_store: Arc<dyn SwapStore + Send + Sync>,
) -> eyre::Result<()> {
    let swaps = swap_store.get_swaps(chain).await?;
    if swaps.is_empty() {
        return Ok(());
    }

    tracing::info!(
        count = swaps.len(),
        "address_poller: checking {} pending swap addresses",
        swaps.len()
    );

    let mut events: Vec<SwapEvent> = Vec::new();
    for swap in &swaps {
        match query_swap_events(client, indexer_url, swap).await {
            Ok(mut evs) => events.append(&mut evs),
            Err(e) => tracing::warn!(
                swap_id = %swap.swap_id,
                error = %e,
                "address_poller: failed to query address"
            ),
        }
    }

    if let Ok(vec1) = Vec1::new(events) {
        tracing::info!(count = vec1.len(), "address_poller: submitting events");
        swap_store.update_swaps(vec1).await?;
    }

    Ok(())
}

/// Queries the esplora API for all txs involving the swap address and returns
/// all matching Initiate / Redeem / Refund events found.
async fn query_swap_events(
    client: &reqwest::Client,
    indexer_url: &str,
    swap: &Swap,
) -> eyre::Result<Vec<SwapEvent>> {
    let url = format!(
        "{}/address/{}/txs",
        indexer_url.trim_end_matches('/'),
        swap.swap_id
    );

    let txs: Vec<EsploraTx> = client.get(&url).send().await?.json().await?;

    let mut events = Vec::new();

    for tx in &txs {
        let block_number = tx.status.block_height.unwrap_or(0);
        let block_timestamp = tx
            .status
            .block_time
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0));

        let tx_hash = format!("{}:{}", tx.txid, block_number);

        // ── Deposit detection (Initiate) ────────────────────────────────
        for vout in &tx.vout {
            let matches = vout
                .scriptpubkey_address
                .as_deref()
                .map(|a| a == swap.swap_id)
                .unwrap_or(false);

            if matches && vout.value == swap.amount as u64 {
                // Only record if the tx is confirmed (unconfirmed inits should
                // be left to the ZMQ path; we fill gaps for confirmed ones).
                tracing::info!(
                    swap_id = %swap.swap_id,
                    txid = %tx.txid,
                    confirmed = tx.status.confirmed,
                    "address_poller: found deposit"
                );
                let filled = if block_number > 0 { swap.amount } else { 0 };
                events.push(SwapEvent {
                    event_type: SwapEventType::Initiate,
                    swap_id: swap.swap_id.clone(),
                    amount: filled,
                    tx_info: TxInfo {
                        tx_hash: tx_hash.clone(),
                        block_number,
                        block_timestamp,
                        detected_timestamp: Some(Utc::now()),
                    },
                    is_blacklisted: false,
                });
                break; // at most one deposit per tx
            }
        }

        // ── Spend detection (Redeem / Refund) ───────────────────────────
        for vin in &tx.vin {
            let prevout_matches = vin
                .prevout
                .as_ref()
                .and_then(|p| p.scriptpubkey_address.as_deref())
                .map(|a| a == swap.swap_id)
                .unwrap_or(false);

            if !prevout_matches {
                continue;
            }

            let witness = match &vin.witness {
                Some(w) => w,
                None => continue,
            };

            let event_type = match classify_spend(witness) {
                Some(et) => et,
                None => {
                    tracing::warn!(
                        swap_id = %swap.swap_id,
                        txid = %tx.txid,
                        witness_len = witness.len(),
                        "address_poller: unrecognised witness pattern, skipping"
                    );
                    continue;
                }
            };

            tracing::info!(
                swap_id = %swap.swap_id,
                txid = %tx.txid,
                event = ?event_type,
                "address_poller: found spend event"
            );

            events.push(SwapEvent {
                event_type,
                swap_id: swap.swap_id.clone(),
                amount: swap.amount,
                tx_info: TxInfo {
                    tx_hash: tx_hash.clone(),
                    block_number,
                    block_timestamp,
                    detected_timestamp: Some(Utc::now()),
                },
                is_blacklisted: false,
            });
            break; // at most one spend per tx per address
        }
    }

    Ok(events)
}

/// Mirrors the logic in `get_htlc_spend_type` but operates on hex-encoded
/// witness items returned by the esplora API.
///
/// Witness patterns (same as on-chain Taproot HTLC):
///   len == 4, witness[0].len != witness[1].len  →  Redeem (secret = witness[1])
///   len == 4, witness[0].len == witness[1].len  →  Instant Refund
///   len == 3                                    →  Timelock Refund
fn classify_spend(witness: &[String]) -> Option<SwapEventType> {
    match witness.len() {
        4 => {
            // hex strings: byte-length in hex is 2× the decoded byte length
            let is_redeem = witness[0].len() != witness[1].len();
            if is_redeem {
                let secret_hex = witness[1].clone();
                OrderSecret::new(secret_hex)
                    .map(SwapEventType::Redeem)
                    .ok()
            } else {
                Some(SwapEventType::Refund)
            }
        }
        3 => Some(SwapEventType::Refund),
        _ => None,
    }
}
