use async_trait::async_trait;
use tars::bitcoin::TransactionMetadata;

#[async_trait]
pub trait BlockchainIndexer: Send + Sync {
    async fn get_block_height(&self) -> eyre::Result<u64>;
    async fn get_tx(&self, txid: &str) -> eyre::Result<TransactionMetadata>;
}
