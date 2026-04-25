use crate::core::BlockchainIndexer;
use async_trait::async_trait;
use tars::bitcoin::{BitcoinIndexerClient, Indexer, TransactionMetadata};

pub struct GardenBitcoinIndexer {
    client: BitcoinIndexerClient,
}

impl GardenBitcoinIndexer {
    pub fn new(url: String) -> eyre::Result<Self> {
        let client = BitcoinIndexerClient::new(url, None)
            .map_err(|e| eyre::eyre!("Failed to create indexer: {}", e))?;
        Ok(Self { client })
    }
}

#[async_trait]
impl BlockchainIndexer for GardenBitcoinIndexer {
    async fn get_block_height(&self) -> eyre::Result<u64> {
        self.client
            .get_block_height()
            .await
            .map_err(|e| eyre::eyre!("Failed to get block height: {}", e))
    }

    async fn get_tx(&self, txid: &str) -> eyre::Result<TransactionMetadata> {
        self.client
            .get_tx(txid)
            .await
            .map_err(|e| eyre::eyre!("Failed to get tx: {}", e))
    }
}
