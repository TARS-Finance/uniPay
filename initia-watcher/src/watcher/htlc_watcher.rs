use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::rpc::types::{BlockNumberOrTag, Filter};
use alloy::sol_types::SolEvent;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use tokio::time::interval;
use tracing::{error, info, warn};

use orderbook::traits::Orderbook;
use orderbook::OrderbookProvider;

use crate::errors::{Result, WatcherError};
use crate::events::{Initiated, Redeemed, Refunded};
use crate::storage::PgStore;

pub struct HtlcWatcher<P: Provider + Clone + Send + Sync + 'static> {
    pub chain_name: String,
    pub htlc_address: Address,
    pub token_address: Address,
    pub start_block: u64,
    pub interval_ms: u64,
    pub block_span: u64,
    pub provider: P,
    pub orderbook: Arc<OrderbookProvider>,
    pub checkpoints: PgStore,
}

impl<P: Provider + Clone + Send + Sync + 'static> HtlcWatcher<P> {
    pub async fn run(mut self, mut shutdown: tokio::sync::broadcast::Receiver<()>) {
        let mut tick = interval(Duration::from_millis(self.interval_ms));

        info!(
            chain = %self.chain_name,
            htlc  = %self.htlc_address,
            "HTLC watcher started from block {}",
            self.start_block
        );

        loop {
            tokio::select! {
                _ = shutdown.recv() => {
                    info!(
                        chain = %self.chain_name,
                        htlc  = %self.htlc_address,
                        "Shutdown — saving checkpoint at block {}",
                        self.start_block
                    );
                    let _ = self.checkpoints
                        .update_checkpoint(&self.chain_name, &self.htlc_address.to_string(), self.start_block)
                        .await;
                    break;
                }

                _ = tick.tick() => {
                    match self.fetch_and_process().await {
                        Ok(next_block) if next_block > self.start_block => {
                            self.start_block = next_block;
                            let _ = self.checkpoints
                                .update_checkpoint(&self.chain_name, &self.htlc_address.to_string(), self.start_block)
                                .await;
                        }
                        Ok(_) => {}
                        Err(e) => {
                            // Do not advance the checkpoint so the range is retried next tick.
                            error!(
                                chain = %self.chain_name,
                                htlc  = %self.htlc_address,
                                "Error processing logs: {e}"
                            );
                        }
                    }
                }
            }
        }
    }

    async fn fetch_and_process(&self) -> Result<u64> {
        let current_block = self
            .provider
            .get_block_number()
            .await
            .map_err(|e| WatcherError::Rpc(e.to_string()))?;

        if current_block == 0 || self.start_block >= current_block {
            return Ok(self.start_block);
        }

        // One block behind tip for finality safety.
        let end_block = (self.start_block + self.block_span).min(current_block - 1);

        let filter = Filter::new()
            .address(self.htlc_address)
            .from_block(self.start_block)
            .to_block(end_block);

        let logs = self
            .provider
            .get_logs(&filter)
            .await
            .map_err(|e| {
                WatcherError::FetchLogs(
                    self.htlc_address.to_string(),
                    self.start_block,
                    end_block,
                    e.to_string(),
                )
            })?;

        for log in &logs {
            if let Err(e) = self.handle_log(log).await {
                warn!(
                    chain = %self.chain_name,
                    htlc  = %self.htlc_address,
                    tx    = ?log.transaction_hash,
                    "Failed to handle log: {e}"
                );
            }
        }

        Ok(end_block + 1)
    }

    async fn handle_log(&self, log: &alloy::rpc::types::Log) -> Result<()> {
        let topic0 = log
            .topics()
            .first()
            .copied()
            .ok_or(WatcherError::MissingTopic(0))?;

        let tx_hash = log
            .transaction_hash
            .map(|h| h.to_string())
            .unwrap_or_default();

        let block_number = log.block_number.unwrap_or(self.start_block);

        match topic0 {
            t if t == Initiated::SIGNATURE_HASH => self.on_initiated(log, tx_hash, block_number).await,
            t if t == Redeemed::SIGNATURE_HASH  => self.on_redeemed(log, tx_hash, block_number).await,
            t if t == Refunded::SIGNATURE_HASH  => self.on_refunded(log, tx_hash, block_number).await,
            _ => Ok(()),
        }
    }

    async fn on_initiated(
        &self,
        log: &alloy::rpc::types::Log,
        tx_hash: String,
        block_number: u64,
    ) -> Result<()> {
        let decoded = Initiated::decode_log(&log.inner)
            .map_err(|e| WatcherError::Decode(e.to_string()))?;

        let swap_id = hex::encode(decoded.data.orderID.0);
        let filled_amount = BigDecimal::from_str(&decoded.data.amount.to_string())
            .map_err(|e| WatcherError::Decode(e.to_string()))?;
        let timestamp = self.block_timestamp(block_number).await;

        info!(chain = %self.chain_name, swap_id, "Initiated");

        self.orderbook
            .update_swap_initiate(&swap_id, filled_amount, &tx_hash, block_number as i64, timestamp)
            .await
            .map_err(|e| WatcherError::Database(e.to_string()))
    }

    async fn on_redeemed(
        &self,
        log: &alloy::rpc::types::Log,
        tx_hash: String,
        block_number: u64,
    ) -> Result<()> {
        let decoded = Redeemed::decode_log(&log.inner)
            .map_err(|e| WatcherError::Decode(e.to_string()))?;

        let swap_id = hex::encode(decoded.data.orderID.0);
        // secret is bytes (variable length), hex-encode without 0x prefix
        let secret = hex::encode(&decoded.data.secret);
        let timestamp = self.block_timestamp(block_number).await;

        info!(chain = %self.chain_name, swap_id, "Redeemed");

        self.orderbook
            .update_swap_redeem(&swap_id, &tx_hash, &secret, block_number as i64, timestamp)
            .await
            .map_err(|e| WatcherError::Database(e.to_string()))
    }

    async fn on_refunded(
        &self,
        log: &alloy::rpc::types::Log,
        tx_hash: String,
        block_number: u64,
    ) -> Result<()> {
        let decoded = Refunded::decode_log(&log.inner)
            .map_err(|e| WatcherError::Decode(e.to_string()))?;

        let swap_id = hex::encode(decoded.data.orderID.0);
        let timestamp = self.block_timestamp(block_number).await;

        info!(chain = %self.chain_name, swap_id, "Refunded");

        self.orderbook
            .update_swap_refund(&swap_id, &tx_hash, block_number as i64, timestamp)
            .await
            .map_err(|e| WatcherError::Database(e.to_string()))
    }

    /// Fetches the block timestamp from the chain; falls back to now() on failure.
    async fn block_timestamp(&self, block_number: u64) -> DateTime<Utc> {
        match self
            .provider
            .get_block_by_number(BlockNumberOrTag::Number(block_number))
            .await
        {
            Ok(Some(block)) => {
                DateTime::from_timestamp(block.header.timestamp as i64, 0)
                    .unwrap_or_else(Utc::now)
            }
            _ => {
                warn!(
                    chain = %self.chain_name,
                    "Could not fetch timestamp for block {block_number}, using now()"
                );
                Utc::now()
            }
        }
    }
}
