use async_trait::async_trait;

#[async_trait]
pub trait RPCClient: Send + Sync {
    async fn get_mempool_entry(&self, tx_id: &str) -> eyre::Result<Option<serde_json::Value>>;
}
