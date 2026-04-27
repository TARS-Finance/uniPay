use alloy::{
    primitives::{Address, BlockHash, TxHash},
    sol_types::SolEventInterface,
};
use alloy_rpc_types_eth::Log;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventExt<T>
where
    T: SolEventInterface,
{
    pub event: T,
    /// The address which emitted this log.
    pub address: Address,
    /// Hash of the block the transaction that emitted this log was mined in
    pub block_hash: Option<BlockHash>,
    /// Number of the block the transaction that emitted this log was mined in
    pub block_number: Option<u64>,
    /// Transaction Hash
    pub transaction_hash: Option<TxHash>,
    /// Index of the Transaction in the block
    pub transaction_index: Option<u64>,
    /// Log Index in Block
    pub log_index: Option<u64>,
    /// Geth Compatibility Field: whether this log was removed
    pub removed: bool,
}

impl<T> From<(&Log, T)> for EventExt<T>
where
    T: SolEventInterface,
{
    fn from((log, event): (&Log, T)) -> Self {
        Self {
            event,
            address: log.inner.address,
            block_hash: log.block_hash,
            block_number: log.block_number,
            transaction_hash: log.transaction_hash,
            transaction_index: log.transaction_index,
            log_index: log.log_index,
            removed: log.removed,
        }
    }
}
