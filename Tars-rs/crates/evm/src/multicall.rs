use crate::{
    errors::MulticallError,
    primitives::{AlloyProvider, CallParams},
    Multicall3::Call3Value,
    Multicall3Contract,
};
use alloy::{
    contract::CallBuilder,
    network::Ethereum,
    primitives::{Bytes, FixedBytes, U256},
};
use alloy_rpc_types_eth::AccessList;
use eyre::Result;
use std::sync::Arc;

/// Represents a result from a multicall operation
#[derive(Debug, Clone)]
pub struct MulticallResult {
    /// Whether the call was successful
    pub success: bool,
    /// The return data from the call
    pub return_data: Bytes,
}

/// Builder for Multicall to allow flexible construction
pub struct MulticallBuilder {
    multicall_contract: Arc<Multicall3Contract>,
    calls: Vec<Call3Value>,
    max_priority_fee_per_gas: Option<u128>,
    max_fee_per_gas: Option<u128>,
    gas_limit: Option<u64>,
    nonce: Option<u64>,
    access_list: AccessList,
}

impl MulticallBuilder {
    /// Create a new MulticallBuilder with the required multicall contract
    ///
    /// # Arguments
    /// * `multicall_contract` - The multicall contract to use
    pub fn new(multicall_contract: Arc<Multicall3Contract>) -> Self {
        Self {
            multicall_contract,
            calls: Vec::new(),
            max_priority_fee_per_gas: None,
            max_fee_per_gas: None,
            gas_limit: None,
            nonce: None,
            access_list: AccessList::default(),
        }
    }

    /// Set the max priority fee per gas
    ///
    /// # Arguments
    /// * `max_priority_fee_per_gas` - The max priority fee per gas to use
    pub fn with_max_priority_fee_per_gas(mut self, max_priority_fee_per_gas: u128) -> Self {
        self.max_priority_fee_per_gas = Some(max_priority_fee_per_gas);
        self
    }

    /// Set the max fee per gas
    ///
    /// # Arguments
    /// * `max_fee_per_gas` - The max fee per gas to use
    pub fn with_max_fee_per_gas(mut self, max_fee_per_gas: u128) -> Self {
        self.max_fee_per_gas = Some(max_fee_per_gas);
        self
    }

    /// Set the gas limit
    ///
    /// # Arguments
    /// * `gas_limit` - The gas limit to use
    pub fn with_gas_limit(mut self, gas_limit: u64) -> Self {
        self.gas_limit = Some(gas_limit);
        self
    }

    /// Set the nonce
    ///
    /// # Arguments
    /// * `nonce` - The nonce to use
    pub fn with_nonce(mut self, nonce: u64) -> Self {
        self.nonce = Some(nonce);
        self
    }

    /// Add a new call to the batch
    ///
    /// # Arguments
    /// * `target` - The address of the contract to call
    /// * `allow_failure` - Whether the call should be allowed to fail
    /// * `call_data` - The encoded function call data
    /// * `value` - The amount of ETH to send with the call
    pub fn add_call(&mut self, call: CallParams, allow_failure: bool) {
        self.calls.push(Call3Value {
            target: call.to,
            allowFailure: allow_failure,
            value: call.value.unwrap_or_default(),
            callData: call.data,
        });

        if let Some(access_list) = call.access_list {
            self.access_list.0.extend(access_list.0);
        }
    }

    /// Build the Multicall
    pub fn build(self) -> Multicall {
        Multicall {
            multicall: self.multicall_contract,
            calls: self.calls,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            max_fee_per_gas: self.max_fee_per_gas,
            gas_limit: self.gas_limit,
            nonce: self.nonce,
            access_list: self.access_list,
        }
    }
}

/// A wrapper around the Multicall3 contract that allows batching multiple contract calls into a single transaction.
/// This implementation supports both value-less and value-containing calls, with functionality to manage
/// and execute multiple ethereum contract interactions atomically.
///
/// # Examples
/// ```rust,ignore
/// use alloy::primitives::{Address, Bytes, U256};
/// use evm::Multicall3Contract;
/// use evm::multicall::Multicall;
/// use evm::primitives::AlloyProvider;
///
/// # async fn example() -> eyre::Result<()> {
/// # let multicall_contract = Multicall3Contract::new(Address::ZERO, Provider);
/// let target_addr = Address::ZERO;
/// let call_data = Bytes::default();
/// let value = U256::from(1);
///
/// let mut multicall = Multicall::builder(multicall_contract)
///     .with_nonce(0)
///     .with_gas_limit(1)
///     .with_max_priority_fee_per_gas(1)
///     .with_max_fee_per_gas(1);
/// multicall.add_call(target_addr, false, call_data.clone(), U256::ZERO);
/// multicall.add_call(target_addr, false, call_data, value);
/// let tx_hash = multicall.build().execute().await?;
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct Multicall {
    multicall: Arc<Multicall3Contract>,
    calls: Vec<Call3Value>,
    max_priority_fee_per_gas: Option<u128>,
    max_fee_per_gas: Option<u128>,
    gas_limit: Option<u64>,
    nonce: Option<u64>,
    access_list: AccessList,
}

impl Multicall {
    /// Creates a new instance of the Multicall wrapper.
    ///
    /// # Arguments
    /// * `multicall_contract` - An instance of the Multicall3 contract
    ///
    /// # Returns
    /// A new Multicall instance with an empty call list and zero total value
    pub fn new(
        multicall_contract: Arc<Multicall3Contract>,
        calls: Option<Vec<Call3Value>>,
        max_priority_fee_per_gas: Option<u128>,
        max_fee_per_gas: Option<u128>,
        gas_limit: Option<u64>,
        nonce: Option<u64>,
        access_list: AccessList,
    ) -> Self {
        Self {
            multicall: multicall_contract,
            calls: calls.unwrap_or(Vec::new()),
            max_priority_fee_per_gas,
            max_fee_per_gas,
            gas_limit,
            nonce,
            access_list,
        }
    }

    pub fn builder(multicall_contract: Arc<Multicall3Contract>) -> MulticallBuilder {
        MulticallBuilder::new(multicall_contract)
    }

    /// Checks if there are any calls in the batch.
    ///
    /// # Returns
    /// `true` if there are no calls, `false` otherwise
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// Calculates the total value of all calls in the batch.
    ///
    /// # Returns
    /// The total value of all calls in the batch
    pub fn total_value(&self) -> U256 {
        self.calls.iter().map(|call| call.value).sum::<U256>()
    }

    /// Executes all batched calls in a single transaction.
    ///
    /// This method will submit the multicall transaction to the blockchain and wait for it to be mined.
    /// The total value of all calls will be sent along with the transaction.
    ///
    /// # Returns
    /// * `Ok(String)` - The transaction hash if successful
    /// * `Err(Error)` - If the execution fails or if there are no calls to execute
    ///
    /// # Errors
    /// Returns an error if:
    /// - There are no calls to execute
    /// - The multicall transaction fails
    /// - There are any issues with the blockchain interaction
    pub async fn execute(&self) -> Result<FixedBytes<32>> {
        if self.calls.is_empty() {
            return Err(eyre::eyre!("No calls to execute"));
        }

        let multicall = self.generate_multicall().await?;

        let tx_hash = multicall
            .send()
            .await
            .map_err(|e| MulticallError::Error(format!("Failed to execute multicall: {}", e)))?
            .tx_hash()
            .clone();

        Ok(tx_hash)
    }

    /// Executes a read-only call to get results without sending a transaction
    ///
    /// This method executes all batched calls without submitting a transaction to the blockchain.
    /// It returns the results of each call in the batch.
    ///
    /// # Returns
    /// * `Ok(Vec<MulticallResult>)` - The results of each call in the batch
    /// * `Err(Error)` - If the execution fails or if there are no calls to execute
    ///
    /// # Errors
    /// Returns an error if:
    /// - There are no calls to execute
    /// - The multicall execution fails
    pub async fn call(&self) -> Result<Vec<MulticallResult>> {
        if self.calls.is_empty() {
            return Err(eyre::eyre!("No calls to execute"));
        }

        let multicall = self.generate_multicall().await?;

        let results = multicall
            .call()
            .await
            .map_err(|e| MulticallError::Error(format!("Failed to execute multicall: {}", e)))?;

        Ok(results
            .into_iter()
            .map(|result| MulticallResult {
                success: result.success,
                return_data: result.returnData,
            })
            .collect())
    }

    async fn generate_multicall(
        &self,
    ) -> Result<
        CallBuilder<
            &AlloyProvider,
            std::marker::PhantomData<crate::Multicall3::aggregate3ValueCall>,
            Ethereum,
        >,
    > {
        let total_value = self.total_value();

        let mut multicall = self
            .multicall
            .aggregate3Value(self.calls.clone())
            .value(total_value);

        if let Some(max_priority_fee_per_gas) = self.max_priority_fee_per_gas {
            multicall = multicall.max_priority_fee_per_gas(max_priority_fee_per_gas);
        }

        if let Some(max_fee_per_gas) = self.max_fee_per_gas {
            multicall = multicall.max_fee_per_gas(max_fee_per_gas);
        }

        if let Some(gas_limit) = self.gas_limit {
            multicall = multicall.gas(gas_limit);
        }

        if let Some(nonce) = self.nonce {
            multicall = multicall.nonce(nonce);
        }

        multicall = multicall.access_list(self.access_list.clone());

        Ok(multicall)
    }
}

impl std::fmt::Debug for Call3Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Call3Value")
            .field("target", &self.target)
            .field("allowFailure", &self.allowFailure)
            .field("value", &self.value)
            .field("callData", &self.callData)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use crate::test_utils::{
        ethereum_provider, get_default_wallet, multicall_contract, random_wallet, Network,
    };

    use super::*;
    use alloy::{
        eips::BlockId,
        hex::FromHex,
        primitives::{Address, U256},
        providers::Provider,
    };

    #[tokio::test]
    async fn test_multicall() {
        let (wallet, _) = random_wallet();
        let provider = ethereum_provider(Some(wallet));
        let multicall3_instance = Arc::new(multicall_contract(provider.clone(), Network::Ethereum));

        let target_addr = Address::from_hex("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();

        let mut multicall_builder = Multicall::builder(multicall3_instance);

        let call = CallParams {
            to: target_addr,
            data: Bytes::default(),
            value: Some(U256::from(10000)),
            access_list: None,
        };

        for _ in 0..10 {
            multicall_builder.add_call(call.clone(), false);
            // 1 gwei
        }
        let tx_hash = multicall_builder.build().execute().await.unwrap();
        dbg!(&tx_hash);
        assert!(!tx_hash.is_empty(), "Transaction hash should not be empty");
    }

    #[tokio::test]
    async fn test_multicall_call_builder() {
        let (wallet, signer) = get_default_wallet();
        let provider = ethereum_provider(Some(wallet));
        let multicall3_instance = Arc::new(multicall_contract(provider.clone(), Network::Ethereum));

        let target_addr = Address::from_hex("0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266").unwrap();
        dbg!(&signer.address());

        let block_info = provider
            .get_block(BlockId::latest())
            .await
            .unwrap()
            .unwrap();
        let base_fee_per_gas = block_info.header.base_fee_per_gas.unwrap();

        let max_priority_fee_per_gas = provider.get_max_priority_fee_per_gas().await.unwrap();

        let nonce = provider
            .get_transaction_count(signer.address())
            .await
            .unwrap();

        dbg!(&nonce, &base_fee_per_gas, &max_priority_fee_per_gas);

        let max_fee_per_gas = (2 * base_fee_per_gas) as u128 + max_priority_fee_per_gas;

        let mut multicall_builder = Multicall::builder(multicall3_instance)
            .with_max_priority_fee_per_gas(max_priority_fee_per_gas)
            .with_max_fee_per_gas(max_fee_per_gas)
            .with_nonce(nonce);

        let call = CallParams {
            to: target_addr,
            data: Bytes::default(),
            value: Some(U256::from(10000)),
            access_list: None,
        };

        for _ in 0..10 {
            multicall_builder.add_call(call.clone(), false);
            // 1 gwei
        }
        let multicall = multicall_builder.build();
        let results = multicall.execute().await;
        dbg!(&results);
        assert!(results.is_ok());
    }
}
