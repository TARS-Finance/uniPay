mod common;
pub mod native;
pub mod nativev2;
pub mod nativev3;
pub mod traits;
pub mod v1;
pub mod v2;
pub mod v3;

use crate::{
    errors::{EvmError, HTLCError, MulticallError},
    primitives::{AlloyProvider, CallParams, UnipayActionType, HTLCContract, SwapInfo, TokenType},
    traits::UnipayActionHandler,
    UnipayHTLCContract, UnipayHTLCv2Contract, UnipayHTLCv3Contract, NativeHTLCContract,
    NativeHTLCv2Contract, NativeHTLCv3Contract,
    ERC20::ERC20Instance,
};
use alloy::{
    dyn_abi::Eip712Domain,
    hex::FromHex,
    primitives::{Address, U256},
    providers::{Provider, WalletProvider},
    signers::{k256::ecdsa::SigningKey, local::LocalSigner},
};
use eyre::Result;
use moka::future::Cache;
use orderbook::primitives::EVMSwap;
use primitives::{HTLCAction, HTLCVersion};
use std::sync::OnceLock;
use tokio::time::{sleep, Duration, Instant};

/// Ethereum address used to represent native tokens like ETH
pub const NATIVE_TOKEN_ADDRESS: &str = "0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE";
/// Address used to represent Aave bridge tokens
pub const AAVE_TOKEN_IDENTIFIER_ADDRESS: &str = "0xaAaAaAaaAaAaAaaAaAAAAAAAAaaaAaAaAaaAaaAa";

/// Timeout for approval transaction confirmation
pub const APPROVAL_CONFIRMATION_TIMEOUT: Duration = Duration::from_secs(60);
/// Delay between approval transaction polling
pub const APPROVAL_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// A struct that represents a Unipay HTLC for each chain.
/// This struct provides functionality to interact with different types of HTLC contracts
/// on EVM-compatible chains.
///
/// It wraps the multicall contract and provides methods to interact with the HTLC
/// to provide a high-level interface for atomic swaps.
#[derive(Clone)]
pub struct UnipayHTLC {
    token_cache: Cache<Address, Address>,
    htlc_cache: Cache<Address, HTLCContract>,
    domain_cache: Cache<Address, Eip712Domain>,
    version_cache: Cache<Address, HTLCVersion>,
    signer: LocalSigner<SigningKey>,
    provider: AlloyProvider,
}

/// Implementation of the UnipayHTLC functionality
///
/// This implementation provides methods to interact with HTLC contracts including:
/// - Initiating swaps
/// - Redeeming swaps
/// - Refunding expired swaps
impl UnipayHTLC {
    /// Creates a new instance of UnipayHTLC.
    ///
    /// # Arguments
    ///
    /// * `signer` - The local signer.
    /// * `provider` - The provider.
    ///
    /// # Returns
    ///
    /// Returns a new UnipayHTLC instance.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use evm::{UnipayHTLC};
    ///
    /// async fn example() {
    ///     let chain_htlc = UnipayHTLC::new(signer, provider);
    /// }
    /// ```
    pub fn new(signer: LocalSigner<SigningKey>, provider: AlloyProvider) -> Self {
        let token_cache = Cache::builder().max_capacity(10000).build();
        let htlc_cache = Cache::builder().max_capacity(10000).build();
        let domain_cache = Cache::builder().max_capacity(10000).build();
        let version_cache = Cache::builder().max_capacity(10000).build();

        Self {
            token_cache,
            htlc_cache,
            domain_cache,
            version_cache,
            signer,
            provider,
        }
    }

    /// Returns the token contract address.
    ///
    /// # Arguments
    ///
    /// * `htlc_address` - The address of the HTLC contract.
    ///
    /// # Returns
    ///
    /// The address of the ERC20 token contract or identifier used in the HTLC.
    pub async fn token(&self, htlc_address: &Address) -> Result<Address, HTLCError> {
        if let Some(token) = self.token_cache.get(htlc_address).await {
            return Ok(token);
        }

        let htlc_contract = UnipayHTLCContract::new(*htlc_address, self.provider.clone());
        let token = htlc_contract
            .token()
            .call()
            .await
            .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))?;

        self.token_cache.insert(*htlc_address, token).await;

        Ok(token)
    }

    /// Returns a HTLC instance based on the token type.
    ///
    /// This function initializes either a Native HTLC or ERC20 HTLC contract depending on the
    /// provided token type. For ERC20 tokens, it also handles the approval process before
    /// creating the contract instance.
    ///
    /// # Arguments
    ///
    /// * `htlc_address` - The address where the HTLC contract is deployed.
    ///
    /// # Returns
    ///
    /// * `HTLCContract` - An enum containing either a Native, ERC20, or ERC20V2 HTLC contract instance.
    pub async fn htlc(&self, htlc_address: &Address) -> Result<HTLCContract, HTLCError> {
        if let Some(htlc) = self.htlc_cache.get(htlc_address).await {
            return Ok(htlc);
        }

        let token_type = self.token_type(htlc_address).await?;
        let version = self.version(htlc_address).await?;

        let htlc_contract = match (token_type, version) {
            (TokenType::Native, version) => match version {
                HTLCVersion::V1 => HTLCContract::new_native_htlc(NativeHTLCContract::new(
                    *htlc_address,
                    self.provider.clone(),
                )),
                HTLCVersion::V2 => HTLCContract::new_native_htlc_v2(NativeHTLCv2Contract::new(
                    *htlc_address,
                    self.provider.clone(),
                )),
                HTLCVersion::V3 => HTLCContract::new_native_htlc_v3(NativeHTLCv3Contract::new(
                    *htlc_address,
                    self.provider.clone(),
                )),
            },

            (TokenType::ERC20, version) => {
                let token_address = self.token(htlc_address).await?;
                approve(htlc_address, &token_address, self.provider.clone()).await?;

                match version {
                    HTLCVersion::V1 => {
                        let unipay_v1_contract =
                            UnipayHTLCContract::new(*htlc_address, self.provider.clone());
                        HTLCContract::new_erc20_htlc(unipay_v1_contract)
                    }
                    HTLCVersion::V2 => {
                        let unipay_v2_contract =
                            UnipayHTLCv2Contract::new(*htlc_address, self.provider.clone());
                        HTLCContract::new_erc20_htlc_v2(unipay_v2_contract)
                    }
                    HTLCVersion::V3 => {
                        let unipay_v3_contract =
                            UnipayHTLCv3Contract::new(*htlc_address, self.provider.clone());
                        HTLCContract::new_erc20_htlc_v3(unipay_v3_contract)
                    }
                }
            }
        };

        self.htlc_cache
            .insert(*htlc_address, htlc_contract.clone())
            .await;

        Ok(htlc_contract)
    }

    /// Gets the calldata and value for a HTLC method.
    ///
    /// # Arguments
    ///
    /// * `action` - The HTLC method (Initiate, Redeem, or Refund).
    /// * `swap` - The swap details.
    /// * `asset` - The HTLC contract address.
    ///
    /// # Returns
    ///
    /// Vector of tuples containing target address, calldata bytes and value
    pub async fn get_calldata(
        &self,
        action: &HTLCAction,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, HTLCError> {
        let htlc = self.htlc(asset).await?;
        let contract = htlc.contract();
        let domain = self.domain(asset).await?;

        match action {
            HTLCAction::InitiateWithSignature | HTLCAction::Initiate => {
                contract
                    .initiate_with_signature_calldata(swap, asset, &domain, &self.signer)
                    .await
            }
            HTLCAction::InitiateWithUserSignature { signature } => {
                contract
                    .initiate_with_user_signature_calldata(signature, swap, asset)
                    .await
            }
            HTLCAction::Redeem { secret } => contract.redeem_calldata(secret, swap, asset).await,
            HTLCAction::Refund => contract.refund_calldata(swap, asset).await,
            HTLCAction::InstantRefund => {
                contract
                    .instant_refund_calldata(swap, asset, &domain, &self.signer)
                    .await
            }
            HTLCAction::NoOp => Ok(vec![]),
        }
    }

    /// Retrieves the on-chain order information for a given swap and asset.
    ///
    /// # Arguments
    ///
    /// * `swap` - Reference to the swap details.
    /// * `asset` - Reference to the HTLC contract address.
    ///
    /// # Returns
    ///
    /// The `SwapInfo` for the given swap and asset, or an `HTLCError`.
    pub async fn get_order(&self, swap: &EVMSwap, asset: &Address) -> Result<SwapInfo, HTLCError> {
        let htlc = self.htlc(asset).await?;
        let contract = htlc.contract();
        let order = contract.get_order(swap).await?;
        Ok(order)
    }

    /// Gets the EIP712 domain for signing.
    ///
    /// # Arguments
    ///
    /// * `asset` - HTLC contract address.
    ///
    /// # Returns
    ///
    /// The EIP712 domain for the HTLC contract
    ///
    /// # Example
    /// ```rust,ignore
    /// use evm::UnipayHTLC;
    /// use alloy::network::EthereumWallet;
    ///
    /// async fn example(htlc: &UnipayHTLC, wallet: &EthereumWallet, message: &impl SolValue) {
    ///     let domain = htlc.domain(asset).await?;
    ///     let signature = wallet.sign_typed_data(message, &domain).await?;
    /// }
    /// ```
    pub async fn domain(&self, asset: &Address) -> Result<Eip712Domain, HTLCError> {
        // Check if the domain is in the cache
        if let Some(domain) = self.domain_cache.get(asset).await {
            return Ok(domain);
        }

        // Use the htlc method to get the contract from cache or create a new one
        let htlc = self.htlc(asset).await?;
        let contract = htlc.contract();

        let domain = contract.domain().await?;

        // Add to cache
        self.domain_cache.insert(*asset, domain.clone()).await;

        Ok(domain)
    }

    /// Determines the version of the HTLC contract.
    ///
    /// # Arguments
    ///
    /// * `asset` - The address of the HTLC contract.
    ///
    /// # Returns
    ///
    /// Returns a Result containing the HTLCVersion (V1 or V2) or an HTLCError.
    pub async fn version(&self, asset: &Address) -> Result<HTLCVersion, HTLCError> {
        if let Some(version) = self.version_cache.get(asset).await {
            return Ok(version);
        }

        // Using the same abi for all versions, we can determine the version by checking the contract
        let htlc = UnipayHTLCContract::new(*asset, self.provider.clone());

        let domain = htlc
            .eip712Domain()
            .call()
            .await
            .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))?;

        let version = match domain.version.as_str() {
            "1" => HTLCVersion::V1,
            "2" => HTLCVersion::V2,
            "3" => HTLCVersion::V3,
            _ => {
                return Err(HTLCError::UnsupportedVersion {
                    version: domain.version,
                    asset: (*asset).to_string(),
                })
            }
        };

        self.version_cache.insert(*asset, version.clone()).await;

        Ok(version)
    }

    /// Determines the type of token used in the HTLC contract.
    ///
    /// # Arguments
    ///
    /// * `htlc_address` - The address of the HTLC contract.
    ///
    /// # Returns
    ///
    /// Returns a Result containing the TokenType (Native or ERC20) or an HTLCError.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use evm::UnipayHTLC;
    /// use alloy::primitives::Address;
    ///
    /// async fn example(htlc: &UnipayHTLC) {
    ///     let htlc_address = Address::from_hex("0x1234...").unwrap();
    ///     let token_type = htlc.token_type(&htlc_address).await?;
    ///     match token_type {
    ///         TokenType::Native => println!("Native token"),
    ///         TokenType::ERC20 => println!("ERC20 token"),
    ///     }
    /// }
    /// ```
    pub async fn token_type(&self, htlc_address: &Address) -> Result<TokenType, HTLCError> {
        static NATIVE_TOKEN: OnceLock<Address> = OnceLock::new();

        let token = self.token(htlc_address).await?;

        if token
            == *NATIVE_TOKEN.get_or_init(|| {
                Address::from_hex(NATIVE_TOKEN_ADDRESS)
                    .expect("Invalid native token address constant")
            })
        {
            return Ok(TokenType::Native);
        } else {
            return Ok(TokenType::ERC20);
        }
    }
}

#[async_trait::async_trait]
impl UnipayActionHandler for UnipayHTLC {
    async fn get_calldata(
        &self,
        action: &UnipayActionType,
        swap: &EVMSwap,
        asset: &Address,
    ) -> Result<Vec<CallParams>, MulticallError> {
        let action = match action {
            UnipayActionType::HTLC(action) => action,
            // Each handler should return an error for any ActionType variant other than the one it is designed to handle.
            // _ => return Err(MulticallError::Error("Unsupported action".to_string())),
        };

        let calldata = self.get_calldata(action, swap, asset).await?;

        Ok(calldata)
    }
}

/// Executes an ERC20 approval transaction for the HTLC contract if needed
///
/// # Arguments
/// * `htlc` - The HTLC contract instance
/// * `erc20` - The ERC20 contract instance
///
/// # Returns
///
/// Transaction hash as a string if approval was needed and successful,
/// or Ok(String::new()) if no approval was needed
///
/// # Errors
/// * `HTLCError::ContractError` if the approval transaction fails
/// * `HTLCError::PendingTransactionError` if watching the pending transaction fails
pub async fn approve(
    htlc_address: &Address,
    erc20_address: &Address,
    provider: AlloyProvider,
) -> Result<String, HTLCError> {
    let erc20 = ERC20Instance::new(*erc20_address, provider.clone());
    let owner = provider
        .signer_addresses()
        .next()
        .ok_or(HTLCError::EvmError(EvmError::AllowanceError(
            "Failed to get signer address".to_string(),
        )))?;
    let current_allowance = erc20
        .allowance(owner, *htlc_address)
        .call()
        .await
        .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))?;

    // If allowance is already sufficient, return empty string
    if current_allowance == U256::MAX {
        return Ok(String::new());
    }

    // Set approval if needed
    let tx_hash = erc20
        .approve(*htlc_address, U256::MAX)
        .send()
        .await
        .map_err(|e| HTLCError::EvmError(EvmError::ContractError(e)))?
        .tx_hash()
        .clone();

    let start = Instant::now();
    loop {
        if start.elapsed() >= APPROVAL_CONFIRMATION_TIMEOUT {
            return Err(HTLCError::EvmError(EvmError::RequestFailed(
                "Failed to confirm approval transaction within timeout".to_string(),
            )));
        }

        match provider.get_transaction_receipt(tx_hash).await {
            Ok(Some(receipt)) => {
                if receipt.status() == true {
                    return Ok(tx_hash.to_string());
                } else {
                    return Err(HTLCError::EvmError(EvmError::RequestFailed(
                        "Approval transaction failed".to_string(),
                    )));
                }
            }
            Ok(None) | Err(_) => {
                sleep(APPROVAL_POLL_INTERVAL).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        executor::UnipayActionExecutor,
        htlc::{approve, UnipayHTLC},
        primitives::{UnipayActionRequest, UnipayActionType, UnipayHandlerType},
        test_utils::{self, get_contracts, multicall_contract, Network},
        traits::UnipayActionHandler,
    };
    use alloy::providers::ext::AnvilApi;
    use primitives::{HTLCAction, HTLCVersion};
    use std::{collections::HashMap, sync::Arc, time::Duration};
    use tokio::time::sleep;
    use tracing::info;

    #[tokio::test]
    async fn test_initiate() {
        let _ = tracing_subscriber::fmt::try_init();

        // Test different HTLC versions
        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (chain_htlc, htlc_contract, _, initiator, chain_id, action_executor, _) =
                get_contracts(version.clone()).await;
            let asset = htlc_contract.address();

            let (swap, _) =
                test_utils::new_swap(initiator.address(), chain_id, *asset, version.clone());

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::Initiate),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            info!("Initiating {:?} swap", version);
            let tx_hash = action_executor.multicall(&[request], None).await.unwrap();
            info!("Initiate {:?} transaction: {}", version, tx_hash);

            sleep(Duration::from_secs(10)).await;

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(!order.is_fulfilled);
            assert_eq!(order.initiator, initiator.address());
        }
    }

    #[tokio::test]
    async fn test_init_sig() {
        let _ = tracing_subscriber::fmt::try_init();

        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (chain_htlc, htlc_contract, _, initiator, chain_id, action_executor, _) =
                get_contracts(version.clone()).await;
            let asset = htlc_contract.address();
            let (swap, _) =
                test_utils::new_swap(initiator.address(), chain_id, *asset, version.clone());
            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::InitiateWithSignature),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            info!("Initiating {:?} swap", version);
            let txid = action_executor.multicall(&[request], None).await.unwrap();

            info!("Transaction ID: {}", txid);

            sleep(Duration::from_secs(10)).await;

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(!order.is_fulfilled);
            assert_eq!(order.initiator, initiator.address());
        }
    }

    #[tokio::test]
    async fn test_redeem() {
        let _ = tracing_subscriber::fmt::try_init();

        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (
                chain_htlc,
                htlc_contract,
                _erc20_contract,
                initiator,
                chain_id,
                action_executor,
                _,
            ) = get_contracts(version.clone()).await;
            let asset = htlc_contract.address();
            let (swap, secret) =
                test_utils::new_swap(initiator.address(), chain_id, *asset, version.clone());

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::Initiate),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            let tx_hash = action_executor.multicall(&[request], None).await.unwrap();
            sleep(Duration::from_secs(5)).await;
            info!("Initiate {:?} transaction: {}", version, tx_hash);

            chain_htlc
                .provider
                .anvil_mine(Some(1), Some(1))
                .await
                .unwrap();

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(!order.is_fulfilled);
            info!("Order initiator: {:#?}", order.initiator);

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::Redeem {
                    secret: secret.clone(),
                }),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            let tx_hash = action_executor.multicall(&[request], None).await.unwrap();
            sleep(Duration::from_secs(10)).await;
            info!("Redeem {:?} transaction: {}", version, tx_hash);

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(order.is_fulfilled);
        }
    }

    #[tokio::test]
    async fn test_refund() {
        let _ = tracing_subscriber::fmt::try_init();

        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (
                chain_htlc,
                htlc_contract,
                _erc20_contract,
                initiator,
                chain_id,
                action_executor,
                _,
            ) = get_contracts(version.clone()).await;
            let provider = test_utils::ethereum_provider(None);
            let asset = htlc_contract.address();
            let (swap, _) =
                test_utils::new_swap(initiator.address(), chain_id, *asset, version.clone());

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::Initiate),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            info!("Initiating {:?} swap", version);
            let tx_hash = action_executor.multicall(&[request], None).await.unwrap();
            info!("Initiate {:?} transaction: {}", version, tx_hash);

            provider
                .anvil_mine(Some(10), Some(1))
                .await
                .expect("Failed to mine");

            sleep(Duration::from_secs(10)).await;

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::Refund),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            let refund_tx = action_executor.multicall(&[request], None).await.unwrap();
            info!("Refund {:?} transaction: {}", version, refund_tx);

            sleep(Duration::from_secs(10)).await;
            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(order.is_fulfilled);
        }
    }

    #[tokio::test]
    async fn test_instant_refund() {
        let _ = tracing_subscriber::fmt::try_init();

        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (chain_htlc, htlc_contract, _, initiator, chain_id, action_executor, _) =
                get_contracts(version.clone()).await;
            let asset = htlc_contract.address();
            // swap with default wallet address as redeemer
            let (swap, _) =
                test_utils::new_swap(initiator.address(), chain_id, *asset, version.clone());

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(HTLCAction::Initiate),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            let tx_hash = action_executor.multicall(&[request], None).await.unwrap();
            info!("Initiate {:?} transaction: {}", version, tx_hash);

            sleep(Duration::from_secs(10)).await;

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(!order.is_fulfilled);
            assert_eq!(order.initiator, initiator.address());

            // Provider with the redeemer wallet
            let (wallet, signer) = test_utils::get_default_wallet();
            let provider = test_utils::ethereum_provider(Some(wallet));
            let multicall3_instance =
                Arc::new(multicall_contract(provider.clone(), Network::Ethereum));
            let redeemer_htlc = Arc::new(UnipayHTLC::new(signer, provider));
            let mut handlers = HashMap::new();
            handlers.insert(
                UnipayHandlerType::HTLC,
                redeemer_htlc as Arc<dyn UnipayActionHandler>,
            );
            let multicall = UnipayActionExecutor::new(multicall3_instance, handlers);

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(primitives::HTLCAction::InstantRefund),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            let tx_hash = multicall.multicall(&[request], None).await.unwrap();
            tracing::info!("Instant refund {:?} transaction ID: {}", version, tx_hash);

            sleep(Duration::from_secs(10)).await;

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(order.is_fulfilled);
        }
    }

    #[tokio::test]
    async fn test_initiate_with_executor_sig() {
        let _ = tracing_subscriber::fmt::try_init();
        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (chain_htlc, htlc_contract, _, initiator, chain_id, contract_wrapper, _) =
                get_contracts(version.clone()).await;
            let asset = htlc_contract.address();
            let (swap, _) =
                test_utils::new_swap(initiator.address(), chain_id, *asset, version.clone());

            let request = UnipayActionRequest {
                action: UnipayActionType::HTLC(primitives::HTLCAction::InitiateWithSignature),
                swap: swap.clone(),
                asset: htlc_contract.address().to_string(),
                id: swap.secret_hash.to_string(),
            };

            let tx_hash = contract_wrapper.multicall(&[request], None).await.unwrap();
            tracing::info!(
                "Initiate with executor {:?} signature transaction ID: {}",
                version,
                tx_hash
            );

            sleep(Duration::from_secs(10)).await;

            let order = chain_htlc
                .get_order(&swap, htlc_contract.address())
                .await
                .unwrap();
            assert!(!order.is_fulfilled);
            assert_eq!(order.initiator, initiator.address());
        }
    }

    #[tokio::test]
    async fn test_approve() {
        let (_chain_htlc, htlc_contract, erc20_contract, _initiator, _chain_id, _, _) =
            get_contracts(HTLCVersion::V3).await;
        let provider = test_utils::ethereum_provider(None);
        let res = approve(htlc_contract.address(), erc20_contract.address(), provider).await;
        assert!(res.is_ok(), "Failed to approve");
    }

    #[tokio::test]
    async fn test_version() {
        let versions = vec![HTLCVersion::V1, HTLCVersion::V3];

        for version in versions {
            let (chain_htlc, htlc_contract, _, _, _, _, _) = get_contracts(version.clone()).await;
            let version = chain_htlc.version(htlc_contract.address()).await.unwrap();
            assert_eq!(version, version, "Expected HTLC version {:?}", version);
        }
    }
}
