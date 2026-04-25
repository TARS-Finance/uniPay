use alloy::primitives::Address;
use tokio::time::Interval;

#[derive(Debug, Clone)]
pub struct ContractAddresses {
    pub v2: Vec<Address>,
    pub v3: Vec<Address>,
}
impl ContractAddresses {
    pub fn new(v2: Vec<Address>, v3: Vec<Address>) -> Self {
        Self { v2, v3 }
    }
}
/// Configuration for blockchain chain monitoring
#[derive(Debug)]
pub struct ChainConfig {
    pub name: String,
    pub polling_interval: Interval,
    pub max_block_span: u64,
    pub contract_addresses: ContractAddresses,
}

impl ChainConfig {
    pub fn new(
        chain_name: String,
        polling_interval: Interval,
        max_block_span: u64,
        contract_addresses: ContractAddresses,
    ) -> Self {
        Self {
            name: chain_name,
            polling_interval,
            max_block_span,
            contract_addresses,
        }
    }
}
