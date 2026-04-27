use crate::{
    executor::UnipayActionExecutor,
    htlc::UnipayHTLC,
    primitives::{AlloyProvider, UnipayHandlerType},
    traits::UnipayActionHandler,
    ERC20Contract,
    UnipayHTLC::UnipayHTLCInstance,
    UnipayHTLCContract, Multicall3Contract, OrderbookContract,
    ERC20::ERC20Instance,
};
use alloy::{
    hex::FromHex,
    network::EthereumWallet,
    primitives::{address, fixed_bytes, Address, Bytes, FixedBytes, B256, U256},
    providers::{Provider, ProviderBuilder},
    signers::{
        k256::{
            ecdsa::SigningKey,
            sha2::{self, Digest},
        },
        local::{LocalSigner, PrivateKeySigner},
    },
    sol_types::SolValue,
    transports::http::reqwest,
};
use orderbook::primitives::EVMSwap;
use primitives::HTLCVersion;
use std::{collections::HashMap, process::Command, sync::Arc, thread::sleep, time::Duration};
use tracing::info;
use utils::gen_secret;

type Contracts = (
    Arc<UnipayHTLC>,
    UnipayHTLCInstance<AlloyProvider>,
    ERC20Instance<AlloyProvider>,
    PrivateKeySigner,
    u64,
    UnipayActionExecutor,
    AlloyProvider,
);

/// Returns a tuple containing:
/// * The HTLCHandler
/// * The UnipayHTLC contract
/// * The ERC20 contract
/// * The PrivateKeySigner
/// * The chain ID
/// * The action executor
/// * The provider
pub async fn get_contracts(version: HTLCVersion) -> Contracts {
    let signer = PrivateKeySigner::random();
    fund(signer.address().to_string());

    let wallet = EthereumWallet::from(signer.clone());
    let provider = ethereum_provider(Some(wallet));
    let (htlc_contract, erc20_contract, multicall3) =
        htlc_contract(provider.clone(), version).await;
    let chain_id = provider
        .get_chain_id()
        .await
        .expect("Failed to get chain ID");
    let multicall3 = Arc::new(multicall3);
    let htlc = Arc::new(UnipayHTLC::new(signer.clone(), provider.clone()));

    let mut handlers = HashMap::new();
    handlers.insert(
        UnipayHandlerType::HTLC,
        Arc::new(UnipayHTLC::new(signer.clone(), provider.clone())) as Arc<dyn UnipayActionHandler>,
    );

    let contract_wrapper = UnipayActionExecutor::new(multicall3.clone(), handlers);
    (
        htlc,
        htlc_contract,
        erc20_contract,
        signer,
        chain_id,
        contract_wrapper,
        provider,
    )
}

/// Represents the different blockchain networks supported by the system.
///
/// This enum is used to specify which network to connect to when creating
/// providers and contracts.
pub enum Network {
    /// Represents the Ethereum blockchain network
    Ethereum,
    /// Represents the Arbitrum blockchain network
    Arbitrum,
}

/// Funds a wallet address using the merry faucet command.
///
/// This function executes the 'merry faucet' command to send localnet tokens
/// to the specified address. If the command fails, it retries after a short delay.
///
/// # Arguments
///
/// * `addr` - The wallet address to fund.
///
/// # Side Effects
///
/// - Executes a command-line tool.
/// - Sleeps for 15 seconds after funding to allow transaction confirmation
pub fn fund(addr: String) {
    let res = Command::new("merry")
        .arg("faucet")
        .arg("--to")
        .arg(addr.clone())
        .output()
        .expect("Failed to execute command");
    if !res.stderr.is_empty() {
        sleep(Duration::from_secs(1));
        return fund(addr);
    }
    sleep(Duration::from_secs(15));
}

/// Creates a new random wallet and funds it with localnet tokens.
///
/// This function generates a random Ethereum wallet with its associated signer,
/// then uses the faucet to provide it with initial funds for testing.
///
/// # Returns
/// A tuple containing:
/// * The Ethereum wallet
/// * The local signer for transaction signing
pub fn random_wallet() -> (EthereumWallet, LocalSigner<SigningKey>) {
    let signer = PrivateKeySigner::random();
    fund(signer.address().to_string());
    (EthereumWallet::from(signer.clone()), signer)
}

/// Returns a default wallet with a fixed private key for testing.
///
/// This function creates a wallet using a predefined private key and
/// returns both the wallet and its signer.
///
/// # Returns
/// A tuple containing:
/// * The Ethereum wallet
/// * The local signer for transaction signing
///
/// # Warning
/// The private key is hardcoded and should never be used in production.
pub fn get_default_wallet() -> (EthereumWallet, LocalSigner<SigningKey>) {
    let signer = PrivateKeySigner::from_bytes(&fixed_bytes!(
        "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d"
    ))
    .unwrap();
    (EthereumWallet::from(signer.clone()), signer)
}

/// Creates an Ethereum provider connected to a local Anvil node.
///
/// This function returns a provider configured to connect to a local
/// Ethereum node (Anvil) running on port 8545.
///
/// # Arguments
/// * `wallet` - Optional Ethereum wallet to use for transactions
///
/// # Returns
/// An `AlloyProvider` instance connected to the local Ethereum network
pub fn ethereum_provider(wallet: Option<EthereumWallet>) -> AlloyProvider {
    provider(wallet, "http://localhost:8545".to_string())
}

/// Approves the HTLC contract to spend tokens on behalf of the wallet.
///
/// This function:
/// 1. Identifies the appropriate HTLC contract based on the chain ID
/// 2. Gets the token address from the HTLC contract
/// 3. Creates an ERC20 contract instance
/// 4. Calls approve on the token to allow the HTLC contract to spend tokens
///
/// # Arguments
/// * `provider` - The provider to use for contract interactions
///
/// # Returns
/// Nothing, but logs the token address and submits an approval transaction
pub async fn approve_htlc_token(provider: AlloyProvider) {
    let chain = provider.get_chain_id().await.unwrap();
    let addr = if chain == 31337 {
        "6d49021ebF8172F4B51A52a621C7Fc94BD8364cF"
    } else {
        "6d49021ebF8172F4B51A52a621C7Fc94BD8364cF"
    };
    let htlc_contract = UnipayHTLCContract::new(Address::from_hex(addr).unwrap(), provider.clone());

    let token = htlc_contract.token().call().await.unwrap();

    let erc20 = ERC20Contract::new(token, provider);

    info!("{}", erc20.address().to_string());

    erc20
        .approve(*htlc_contract.address(), U256::MAX)
        .send()
        .await
        .unwrap()
        .watch()
        .await
        .unwrap();
}

/// Gets the HTLC contract and related contracts for the specified provider.
///
/// This function creates contract instances for:
/// 1. The Unipay HTLC contract
/// 2. The associated ERC20 token contract
/// 3. A multicall contract for batched execution
///
/// It also approves the HTLC contract to spend the maximum amount of tokens.
///
/// # Arguments
/// * `provider` - An Ethereum provider to use for contract interactions
///
/// # Returns
/// A tuple containing:
/// * The Unipay HTLC contract
/// * The ERC20 token contract
/// * A multicall contract for batched calls
///
/// # Panics
/// If the chain ID is not supported (must be 31337 for Ethereum or 31338 for Arbitrum)
pub async fn htlc_contract(
    provider: AlloyProvider,
    version: HTLCVersion,
) -> (UnipayHTLCContract, ERC20Contract, Multicall3Contract) {
    let chain = provider.get_chain_id().await.unwrap();
    let addr;
    let network;
    (addr, network) = match version {
        HTLCVersion::V3 => match chain {
            31337 => (
                "6d49021ebF8172F4B51A52a621C7Fc94BD8364cF",
                Network::Ethereum,
            ),
            31338 => (
                "6d49021ebF8172F4B51A52a621C7Fc94BD8364cF",
                Network::Arbitrum,
            ),
            _ => panic!("chain not supported"),
        },
        HTLCVersion::V1 => match chain {
            31337 => (
                "9fe46736679d2d9a65f0992f2272de9f3c7fa6e0",
                Network::Ethereum,
            ),
            31338 => (
                "0165878a594ca255338adfa4d48449f69242eb8f",
                Network::Arbitrum,
            ),
            _ => panic!("chain not supported"),
        },
        _ => panic!("version not supported"),
    };
    let htlc = UnipayHTLCContract::new(Address::from_hex(addr).unwrap(), provider.clone());

    let token_address = htlc
        .token()
        .call()
        .await
        .expect("Failed to get token address");

    info!("Token address: {:?}", token_address);

    let erc20 = ERC20Contract::new(token_address, provider.clone());
    let multicall = multicall_contract(provider, network);

    (htlc, erc20, multicall)
}

/// Creates a provider for a specified network URL.
///
/// This is a helper function that creates an Alloy provider with
/// recommended fillers and a wallet for transaction signing.
///
/// # Arguments
/// * `wallet` - Optional Ethereum wallet to use with the provider
/// * `url` - The URL of the RPC endpoint to connect to
///
/// # Returns
/// An `AlloyProvider` configured with the specified wallet and URL
fn provider(wallet: Option<EthereumWallet>, url: String) -> AlloyProvider {
    let w = match wallet {
        Some(w) => w,
        None => get_default_wallet().0,
    };

    let url = reqwest::Url::parse(&url).expect("Invalid URL");
    ProviderBuilder::new()
        .disable_recommended_fillers()
        .with_gas_estimation()
        .with_simple_nonce_management()
        .fetch_chain_id()
        .wallet(w)
        .connect_http(url)
}

/// Creates a new swap transaction with random secret.
///
/// This function generates a new EVM swap with:
/// - The provided initiator address
/// - The default wallet as the redeemer
/// - A randomly generated secret and hash
/// - Fixed timelock and amount values
/// - Computes the order ID based on the HTLC version
///
/// # Arguments
///
/// * `initiator` - The address initiating the swap.
/// * `chain_id` - The chain ID for the swap.
/// * `asset` - The asset address for the swap.
/// * `version` - The HTLC version to use for order ID computation.
///
/// # Returns
///
/// A tuple containing:
/// * The `EVMSwap` structure with all swap details.
/// * The secret byte string that can be used to redeem the swap
pub fn new_swap(
    initiator: Address,
    chain_id: u64,
    asset: Address,
    version: HTLCVersion,
) -> (EVMSwap, Bytes) {
    let (secret, x) = gen_secret();
    let redeemer = get_default_wallet().1.address();
    let secret_hash = B256::new(x.into());
    let timelock = U256::from(10);
    let amount = U256::from(10000);
    let order_id = order_id(
        version,
        chain_id,
        &secret_hash.into(),
        &initiator,
        &redeemer,
        &amount,
        &timelock,
        &asset,
    );
    (
        EVMSwap {
            initiator,
            redeemer,
            secret_hash: B256::new(x.into()),
            timelock: U256::from(10),
            amount: U256::from(10000),
            order_id,
            destination_data: Some(Bytes::new()),
        },
        secret,
    )
}

/// Creates an Arbitrum provider connected to a local node.
///
/// This function returns a provider configured to connect to a local
/// Arbitrum node running on port 8546.
///
/// # Arguments
/// * `wallet` - Optional Ethereum wallet to use for transactions
///
/// # Returns
/// An `AlloyProvider` instance connected to the local Arbitrum network
pub fn arbitrum_provider(wallet: Option<EthereumWallet>) -> AlloyProvider {
    provider(wallet, "http://localhost:8546".to_string())
}

/// Creates an orderbook contract instance connected to Arbitrum.
///
/// This function instantiates the Unipay orderbook contract on the Arbitrum
/// network using a hardcoded address and the default wallet.
///
/// # Returns
/// An `OrderbookContract` instance
pub fn orderbook_contract() -> OrderbookContract {
    let provider = arbitrum_provider(None);
    OrderbookContract::new(
        address!("2279B7A0a67DB372996a5FaB50D91eAA73d2eBe6"),
        provider,
    )
}

/// Creates a multicall contract instance for the specified network.
///
/// This function instantiates a multicall contract that allows batching
/// multiple contract calls into a single transaction for efficiency.
///
/// # Arguments
/// * `provider` - The provider to use for contract interactions
/// * `network` - The network (Ethereum or Arbitrum) to create the contract for
///
/// # Returns
/// A `MulticallContract` instance configured for the specified network
pub fn multicall_contract(provider: AlloyProvider, network: Network) -> Multicall3Contract {
    let addr = match network {
        Network::Arbitrum => "A51c1fc2f0D1a1b8494Ed1FE312d7C3a78Ed91C0",
        Network::Ethereum => "0x2279B7A0a67DB372996a5FaB50D91eAA73d2eBe6",
    };
    Multicall3Contract::new(Address::from_hex(addr).unwrap(), provider)
}

/// Computes the unique order ID for a swap based on the HTLC version and swap parameters.
///
/// For V1, the order ID is the SHA256 hash of (chain_id, secret_hash, initiator).
/// For V2, the order ID is the SHA256 hash of (chain_id, secret_hash, initiator, redeemer, timelock, amount).
///
/// # Arguments
///
/// * `version` - The HTLC version (V1 or V2).
/// * `chain_id` - The chain ID.
/// * `secret_hash` - The hash of the secret.
/// * `initiator` - The address of the swap initiator.
/// * `redeemer` - The address of the swap redeemer.
/// * `amount` - The swap amount.
/// * `timelock` - The swap timelock.
/// * `asset` - The asset address for the swap.
///
/// # Returns
///
/// The computed order ID as a `FixedBytes<32>`.
pub fn order_id(
    version: HTLCVersion,
    chain_id: u64,
    secret_hash: &FixedBytes<32>,
    initiator: &Address,
    redeemer: &Address,
    amount: &U256,
    timelock: &U256,
    asset: &Address,
) -> FixedBytes<32> {
    let hash = match version {
        HTLCVersion::V1 => {
            let components = (chain_id, secret_hash, initiator);
            sha2::Sha256::digest(components.abi_encode())
        }
        HTLCVersion::V2 => {
            let components = (chain_id, secret_hash, initiator, redeemer, timelock, amount);
            sha2::Sha256::digest(components.abi_encode())
        }
        HTLCVersion::V3 => {
            let components = (
                chain_id,
                secret_hash,
                initiator,
                redeemer,
                timelock,
                amount,
                asset,
            );
            sha2::Sha256::digest(components.abi_encode())
        }
    };

    FixedBytes::new(hash.into())
}