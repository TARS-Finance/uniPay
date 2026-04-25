use bitcoin::consensus::Decodable;
use bitcoin::{Block, Transaction};
use eyre::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::time::sleep;
use zeromq::SubSocket;
use zeromq::prelude::*;

const RAW_TX_TOPIC: &str = "rawtx";
const RAW_BLOCK_TOPIC: &str = "rawblock";
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
const INITIAL_RETRY_DELAY: Duration = Duration::from_secs(1);
const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZmqSettings {
    pub raw_tx_url: String,
    pub raw_block_url: String,
}

/// Listens to Bitcoin ZMQ feed for raw transactions and blocks.
/// Handles automatic reconnection with exponential backoff and deduplication via sequence numbers.
pub struct ZmqListener {
    config: ZmqSettings,
    txs_sender: mpsc::Sender<Transaction>,
    blocks_sender: mpsc::Sender<Block>,
}

impl ZmqListener {
    /// Creates a new ZmqListener with the given configuration and channel senders.
    pub fn new(
        config: ZmqSettings,
        txs_sender: mpsc::Sender<Transaction>,
        blocks_sender: mpsc::Sender<Block>,
    ) -> Self {
        Self {
            config,
            txs_sender,
            blocks_sender,
        }
    }

    /// Starts listening to the ZMQ feed. Runs indefinitely with automatic reconnection.
    pub async fn listen(&self) {
        let mut retry_delay = INITIAL_RETRY_DELAY;

        loop {
            match self.connect_and_listen().await {
                Ok(()) => {
                    tracing::warn!("ZMQ connection closed unexpectedly, reconnecting...");
                    retry_delay = INITIAL_RETRY_DELAY;
                }
                Err(e) => {
                    tracing::error!(error = ?e, delay = ?retry_delay, "ZMQ listener error, retrying");
                    sleep(retry_delay).await;
                    retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
                }
            }
        }
    }

    /// Establishes connection and starts processing messages.
    async fn connect_and_listen(&self) -> Result<()> {
        let mut subscriber = self.create_subscriber().await?;

        tracing::info!(
            tx_endpoint = %self.config.raw_tx_url,
            block_endpoint = %self.config.raw_block_url,
            "Connected to Bitcoin ZMQ feed"
        );

        self.poll_messages(&mut subscriber).await
    }

    /// Creates and configures the ZMQ subscriber socket.
    async fn create_subscriber(&self) -> Result<SubSocket> {
        let mut socket = SubSocket::new();

        socket
            .connect(&self.config.raw_tx_url)
            .await
            .wrap_err_with(|| {
                format!(
                    "Failed to connect to tx endpoint: {}",
                    self.config.raw_tx_url
                )
            })?;

        socket
            .connect(&self.config.raw_block_url)
            .await
            .wrap_err_with(|| {
                format!(
                    "Failed to connect to block endpoint: {}",
                    self.config.raw_block_url
                )
            })?;

        socket
            .subscribe(RAW_TX_TOPIC)
            .await
            .wrap_err("Failed to subscribe to rawtx")?;

        socket
            .subscribe(RAW_BLOCK_TOPIC)
            .await
            .wrap_err("Failed to subscribe to rawblock")?;

        Ok(socket)
    }

    /// Main polling loop that receives and processes ZMQ messages.
    async fn poll_messages(&self, subscriber: &mut SubSocket) -> Result<()> {
        let mut last_message_time = Instant::now();
        let mut sequences = SequenceTracker::default();

        loop {
            match tokio::time::timeout(HEARTBEAT_TIMEOUT, subscriber.recv()).await {
                Ok(Ok(message)) => {
                    last_message_time = Instant::now();
                    self.handle_message(&message, &mut sequences).await;
                }
                Ok(Err(e)) => {
                    return Err(eyre::eyre!(e)).wrap_err("ZMQ receive error");
                }
                Err(_) => {
                    tracing::warn!(
                        elapsed = ?last_message_time.elapsed(),
                        timeout = ?HEARTBEAT_TIMEOUT,
                        "Heartbeat timeout, reconnecting"
                    );
                    eyre::bail!("Heartbeat timeout exceeded");
                }
            }
        }
    }

    /// Processes a single ZMQ message, filtering duplicates and dispatching to appropriate channel.
    async fn handle_message(&self, message: &zeromq::ZmqMessage, sequences: &mut SequenceTracker) {
        let Some(parsed) = Self::parse_zmq_message(message) else {
            return;
        };

        // Skip duplicate messages based on sequence number
        if !sequences.is_new(parsed.topic, parsed.sequence) {
            return;
        }

        match parsed.topic {
            RAW_TX_TOPIC => {
                if Self::deserialize_and_send(&self.txs_sender, parsed.payload, RAW_TX_TOPIC).await
                {
                    let (tx_sequence, block_sequence) = sequences.sequence();
                    tracing::info!(tx_sequence = ?tx_sequence, block_sequence = ?block_sequence, "Sent tx");
                }
            }
            RAW_BLOCK_TOPIC => {
                if Self::deserialize_and_send(&self.blocks_sender, parsed.payload, RAW_BLOCK_TOPIC)
                    .await
                {
                    let (tx_sequence, block_sequence) = sequences.sequence();
                    tracing::info!(tx_sequence = ?tx_sequence, block_sequence = ?block_sequence, "Sent block");
                }
            }
            _ => tracing::debug!(topic = parsed.topic, "Received unknown topic"),
        }
    }

    /// Deserializes payload and sends to the appropriate channel.
    async fn deserialize_and_send<T: Decodable + Send>(
        sender: &mpsc::Sender<T>,
        payload: &[u8],
        kind: &str,
    ) -> bool {
        if let Ok(item) = bitcoin::consensus::deserialize(payload) {
            if let Err(e) = sender.send(item).await {
                tracing::error!("Failed to send {kind}: {e}");
                return false;
            } else {
                return true;
            }
        }
        false
    }

    /// Parses a ZMQ multipart message into topic, payload, and sequence number.
    /// Returns None if the message is malformed or invalid.
    fn parse_zmq_message(message: &zeromq::ZmqMessage) -> Option<ParsedZmqMessage<'_>> {
        if message.len() < 3 {
            tracing::warn!(parts = message.len(), "Received malformed ZMQ message");
            return None;
        }

        let topic_frame = message.get(0)?;
        let topic = std::str::from_utf8(topic_frame).ok()?;
        let payload = message.get(1)?;

        // Extract sequence number (4 bytes, little-endian)
        let seq_frame = message.get(2)?;
        let sequence_bytes: [u8; 4] = seq_frame.get(..4)?.try_into().ok()?;
        let sequence = u32::from_le_bytes(sequence_bytes);

        Some(ParsedZmqMessage {
            topic,
            payload,
            sequence,
        })
    }
}

/// Parsed components of a ZMQ message.
struct ParsedZmqMessage<'a> {
    topic: &'a str,
    payload: &'a [u8],
    sequence: u32,
}

/// Tracks sequence numbers for ZMQ messages to filter duplicates.
#[derive(Default)]
struct SequenceTracker {
    tx_sequence: Option<u32>,
    block_sequence: Option<u32>,
}

impl SequenceTracker {
    /// Returns true if this sequence is new, updating the tracker.
    /// Handles sequence resets (Bitcoin Core restarts) and u32 wrap-around.
    fn is_new(&mut self, topic: &str, sequence: u32) -> bool {
        match topic {
            RAW_TX_TOPIC => {
                if Self::is_sequence_valid(sequence, self.tx_sequence) {
                    self.tx_sequence = Some(sequence);
                    true
                } else {
                    false
                }
            }
            RAW_BLOCK_TOPIC => {
                if Self::is_sequence_valid(sequence, self.block_sequence) {
                    self.block_sequence = Some(sequence);
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Determines if a sequence number is valid (new or reset/wrap-around).
    fn is_sequence_valid(new_sequence: u32, last_sequence: Option<u32>) -> bool {
        const RESET_THRESHOLD: u32 = 1000;
        const WRAP_AROUND_THRESHOLD: u32 = u32::MAX - 10000;

        let Some(last_seq) = last_sequence else {
            return true;
        };

        if new_sequence > last_seq {
            return true;
        }

        // Sequence reset detection: Bitcoin Core restarted and reset to 0
        if last_seq > RESET_THRESHOLD && new_sequence < RESET_THRESHOLD {
            tracing::warn!(
                last_sequence = last_seq,
                new_sequence,
                "Detected sequence reset (likely Bitcoin Core restart), accepting new sequence"
            );
            return true;
        }

        // Wrap-around detection: u32 overflow
        if last_seq > WRAP_AROUND_THRESHOLD && new_sequence < RESET_THRESHOLD {
            tracing::warn!(
                last_sequence = last_seq,
                new_sequence,
                "Detected sequence wrap-around, accepting new sequence"
            );
            return true;
        }

        false
    }

    fn sequence(&self) -> (Option<u32>, Option<u32>) {
        (self.tx_sequence, self.block_sequence)
    }
}

#[cfg(test)]
mod tests {
    use bitcoin::Txid;
    use tars::bitcoin::{
        FeeRate, ValidSpendRequests,
        batcher::{batch_tx::build_batch_tx, sign::sign_batch_tx},
        test_utils::{TEST_FEE_RATE, get_test_bitcoin_indexer, get_test_spend_request},
    };
    use std::process::Command;
    use tracing::info;

    use super::*;

    async fn make_test_tx() -> Result<Txid> {
        const REQUEST_COUNT: usize = 5;

        let spend_requests: Result<Vec<_>> =
            futures::future::try_join_all((0..REQUEST_COUNT).map(|_| get_test_spend_request()))
                .await;
        let indexer = get_test_bitcoin_indexer()?;

        let spend_requests = ValidSpendRequests::validate(spend_requests?, &indexer).await?;

        let mut tx = build_batch_tx(&spend_requests, FeeRate::new(TEST_FEE_RATE)?)?;

        sign_batch_tx(&mut tx, &spend_requests)?;
        indexer.submit_tx(&tx).await?;

        Ok(tx.compute_txid())
    }

    /// Mines `cnt` blocks using the merry RPC interface ("merry rpc --generate <cnt>")
    fn mine_block(cnt: u32) -> Result<()> {
        let output = Command::new("merry")
            .args(["rpc", "--generate", &cnt.to_string()])
            .output()
            .context("Failed to execute merry generate command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eyre::bail!("Failed to generate blocks: {}", stderr);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let clean_output = stdout
            .replace("\u{1b}[94m", "")
            .replace("\u{1b}[96m", "")
            .replace("\u{1b}[0m", "");
        info!("Mined {} block(s):\n{}", cnt, clean_output.trim());

        Ok(())
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_listen() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        info!("Starting test_listen");
        let config = ZmqSettings {
            raw_tx_url: "tcp://localhost:28333".to_string(),
            raw_block_url: "tcp://localhost:28332".to_string(),
        };

        info!("Creating channels");
        let (txs_sender, mut txs_receiver) = mpsc::channel::<Transaction>(100);
        let (blocks_sender, mut blocks_receiver) = mpsc::channel::<Block>(100);

        info!("Starting listener");
        let zmq_listener = ZmqListener::new(config, txs_sender, blocks_sender);
        tokio::spawn(async move { zmq_listener.listen().await });

        info!("Making test tx");
        let tx_id = make_test_tx().await.unwrap();
        info!("Test tx made");
        sleep(Duration::from_secs(10)).await;

        info!("Receiving tx");
        let tx = txs_receiver.recv().await.unwrap();
        info!("Tx received");
        dbg!(&tx.compute_txid(), &tx_id);

        info!("Mining block");
        mine_block(1).unwrap();
        info!("Block mined");

        info!("Receiving block");
        let block = blocks_receiver.recv().await.unwrap();
        info!("Block received");
        dbg!(&block.header.block_hash());
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_multiple_txs_and_blocks() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .try_init();

        const NUM_TXS: usize = 3;
        const NUM_BLOCKS: u32 = 2;

        info!("Starting test_multiple_txs_and_blocks");
        let config = ZmqSettings {
            raw_tx_url: "tcp://localhost:28333".to_string(),
            raw_block_url: "tcp://localhost:28332".to_string(),
        };

        info!("Creating channels");
        let (txs_sender, mut txs_receiver) = mpsc::channel::<Transaction>(100);
        let (blocks_sender, mut blocks_receiver) = mpsc::channel::<Block>(100);

        info!("Starting listener");
        let zmq_listener = ZmqListener::new(config, txs_sender, blocks_sender);
        tokio::spawn(async move { zmq_listener.listen().await });

        info!("Submitting {} transactions", NUM_TXS);
        let mut submitted_txids = Vec::new();
        for i in 0..NUM_TXS {
            let txid = make_test_tx().await.unwrap();
            info!("Submitted tx {}: {}", i + 1, txid);
            submitted_txids.push(txid);
        }

        sleep(Duration::from_secs(5)).await;

        info!("Collecting received transactions...");
        let mut received_txs = Vec::new();
        while let Ok(tx) = tokio::time::timeout(Duration::from_secs(2), txs_receiver.recv()).await {
            if let Some(tx) = tx {
                received_txs.push(tx);
            }
        }

        info!("=== Received {} transactions ===", received_txs.len());
        for (i, tx) in received_txs.iter().enumerate() {
            info!("  TX {}: {}", i + 1, tx.compute_txid());
        }

        info!("Mining {} blocks", NUM_BLOCKS);
        mine_block(NUM_BLOCKS).unwrap();

        sleep(Duration::from_secs(3)).await;

        info!("Collecting received blocks...");
        let mut received_blocks = Vec::new();
        while let Ok(block) =
            tokio::time::timeout(Duration::from_secs(2), blocks_receiver.recv()).await
        {
            if let Some(block) = block {
                received_blocks.push(block);
            }
        }

        info!("=== Received {} blocks ===", received_blocks.len());
        for (i, block) in received_blocks.iter().enumerate() {
            let height = block.bip34_block_height().ok();
            info!(
                "  Block {}: hash={}, height={:?}, txs={}",
                i + 1,
                block.header.block_hash(),
                height,
                block.txdata.len()
            );
            for (j, tx) in block.txdata.iter().enumerate() {
                info!("    TX {}: {}", j + 1, tx.compute_txid());
            }
        }

        info!("=== Summary ===");
        info!("Submitted txids: {:?}", submitted_txids);
        info!("Received {} txs via ZMQ", received_txs.len());
        info!("Received {} blocks via ZMQ", received_blocks.len());
    }
}
