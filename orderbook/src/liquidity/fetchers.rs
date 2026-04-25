use async_trait::async_trait;
use bigdecimal::{BigDecimal, FromPrimitive, Num, num_bigint::BigUint};
use reqwest::Url;
use serde::Deserialize;
use solana_client::nonblocking::rpc_client::RpcClient;
use solana_sdk::pubkey::Pubkey;
use spl_associated_token_account::get_associated_token_address;
use starknet::{
    core::types::{BlockId, BlockTag, Felt, FunctionCall},
    macros::selector,
    providers::{JsonRpcClient, Provider, jsonrpc::HttpTransport},
};
use std::{str::FromStr, sync::Arc};

const PRIMARY_TOKEN: &str = "primary";
const STARKNET_PRIMARY_TOKEN_ADDRESS: &str =
    "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

/// Fetches a raw on-chain token balance for a solver account.
#[async_trait]
pub trait LiquidityFetcher: Send + Sync {
    async fn fetch(&self, address: &str, token: &str) -> eyre::Result<BigDecimal>;
}

/// Balance fetcher for Bitcoin-style chains using UTXO summation.
#[derive(Clone)]
pub struct BitcoinLiquidityFetcher {
    rpc_url: Url,
}

impl BitcoinLiquidityFetcher {
    /// Creates a Bitcoin fetcher backed by the configured explorer or RPC URL.
    pub fn new(rpc_url: Url) -> Self {
        Self { rpc_url }
    }
}

#[async_trait]
impl LiquidityFetcher for BitcoinLiquidityFetcher {
    /// Sums every spendable UTXO returned for the solver address.
    async fn fetch(&self, address: &str, token: &str) -> eyre::Result<BigDecimal> {
        if token != PRIMARY_TOKEN {
            return Err(eyre::eyre!("unsupported bitcoin token type: {token}"));
        }

        #[derive(serde::Deserialize)]
        struct Utxo {
            value: u64,
        }

        let url = self.rpc_url.join(&format!("address/{address}/utxo"))?;
        let response = reqwest::get(url).await?;
        let utxos: Vec<Utxo> = response.json().await?;
        let total_value = utxos.iter().map(|utxo| utxo.value).sum::<u64>();
        Ok(BigDecimal::from_str(&total_value.to_string())?)
    }
}

/// Balance fetcher for EVM-compatible chains.
#[derive(Clone)]
pub struct EvmLiquidityFetcher {
    provider: alloy::providers::DynProvider,
}

impl EvmLiquidityFetcher {
    /// Creates a dynamic Alloy provider for the configured chain.
    pub fn new(rpc_url: Url) -> Self {
        let provider = alloy::providers::ProviderBuilder::new().connect_http(rpc_url);
        Self {
            provider: alloy::providers::DynProvider::new(provider),
        }
    }
}

#[async_trait]
impl LiquidityFetcher for EvmLiquidityFetcher {
    /// Reads either the native balance or ERC-20 `balanceOf`.
    async fn fetch(&self, address: &str, token: &str) -> eyre::Result<BigDecimal> {
        use alloy::{
            contract::{ContractInstance, Interface},
            dyn_abi::DynSolValue,
            json_abi::JsonAbi,
            primitives::Address,
            providers::Provider,
        };

        let address = Address::from_str(address)?;
        if token.eq_ignore_ascii_case(PRIMARY_TOKEN) {
            let balance = self.provider.get_balance(address).await?;
            return Ok(BigDecimal::from_str(&balance.to_string())?);
        }

        let abi =
            JsonAbi::parse(["function balanceOf(address owner) external view returns (uint256)"])?;
        let contract = ContractInstance::new(
            Address::from_str(token)?,
            self.provider.clone(),
            Interface::new(abi),
        );
        let call = contract.function("balanceOf", &[DynSolValue::Address(address)])?;
        let balance = call.call().await?;

        match balance.first() {
            Some(DynSolValue::Uint(value, _)) => Ok(BigDecimal::from_str(&value.to_string())?),
            _ => Err(eyre::eyre!("unexpected EVM balanceOf response")),
        }
    }
}

/// Balance fetcher for Solana native balances and SPL token accounts.
pub struct SolanaLiquidityFetcher {
    client: Arc<RpcClient>,
}

impl SolanaLiquidityFetcher {
    /// Creates a nonblocking Solana RPC client.
    pub fn new(rpc_url: Url) -> Self {
        Self {
            client: Arc::new(RpcClient::new(rpc_url.to_string())),
        }
    }
}

#[async_trait]
impl LiquidityFetcher for SolanaLiquidityFetcher {
    /// Resolves the solver ATA when a token mint is requested.
    async fn fetch(&self, address: &str, token: &str) -> eyre::Result<BigDecimal> {
        let address = Pubkey::from_str(address)?;
        if token == PRIMARY_TOKEN {
            let balance = self.client.get_balance(&address).await?;
            return Ok(BigDecimal::from_u64(balance)
                .ok_or_else(|| eyre::eyre!("failed to convert lamports"))?);
        }

        let mint = Pubkey::from_str(token)?;
        let ata = get_associated_token_address(&address, &mint);
        let balance = self.client.get_token_account_balance(&ata).await?;
        Ok(BigDecimal::from_str(&balance.amount)?)
    }
}

/// Balance fetcher for Starknet ERC-20 balances.
#[derive(Clone)]
pub struct StarknetLiquidityFetcher {
    provider: JsonRpcClient<HttpTransport>,
}

impl StarknetLiquidityFetcher {
    /// Creates a Starknet JSON-RPC client.
    pub fn new(rpc_url: Url) -> Self {
        Self {
            provider: JsonRpcClient::new(HttpTransport::new(rpc_url)),
        }
    }
}

#[async_trait]
impl LiquidityFetcher for StarknetLiquidityFetcher {
    /// Reads the Cairo `balance_of` result and reconstructs the uint256 value.
    async fn fetch(&self, address: &str, token: &str) -> eyre::Result<BigDecimal> {
        let contract_address = if token == PRIMARY_TOKEN {
            Felt::from_hex(STARKNET_PRIMARY_TOKEN_ADDRESS)?
        } else {
            Felt::from_hex(token)?
        };

        let address = Felt::from_hex(address)?;
        let balance = self
            .provider
            .call(
                FunctionCall {
                    contract_address,
                    entry_point_selector: selector!("balance_of"),
                    calldata: vec![address],
                },
                BlockId::Tag(BlockTag::Latest),
            )
            .await?;

        if balance.len() < 2 {
            return Err(eyre::eyre!("invalid starknet balance response"));
        }

        let low = BigUint::from_str_radix(balance[0].to_hex_string().trim_start_matches("0x"), 16)?;
        let high =
            BigUint::from_str_radix(balance[1].to_hex_string().trim_start_matches("0x"), 16)?;
        let value: BigUint = low + (high << 128);
        Ok(BigDecimal::from_str(&value.to_string())?)
    }
}

/// Balance fetcher for Sui coins and token objects via JSON-RPC.
#[derive(Clone)]
pub struct SuiLiquidityFetcher {
    rpc_url: Url,
    client: reqwest::Client,
}

impl SuiLiquidityFetcher {
    /// Creates an HTTP JSON-RPC client for Sui.
    pub fn new(rpc_url: Url) -> Self {
        Self {
            rpc_url,
            client: reqwest::Client::new(),
        }
    }
}

/// Partial Sui JSON-RPC response used by the balance fetcher.
#[derive(Debug, Deserialize)]
struct SuiBalanceResponse {
    result: SuiBalanceResult,
}

/// Inner Sui balance payload carrying the raw integer amount.
#[derive(Debug, Deserialize)]
struct SuiBalanceResult {
    #[serde(rename = "totalBalance")]
    total_balance: String,
}

#[async_trait]
impl LiquidityFetcher for SuiLiquidityFetcher {
    /// Calls `suix_getBalance` for either the native coin or a specific token type.
    async fn fetch(&self, address: &str, token: &str) -> eyre::Result<BigDecimal> {
        let params = if token == PRIMARY_TOKEN {
            serde_json::json!([address])
        } else {
            serde_json::json!([address, token])
        };

        let response = self
            .client
            .post(self.rpc_url.clone())
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "suix_getBalance",
                "params": params,
            }))
            .send()
            .await?
            .error_for_status()?;

        let payload: SuiBalanceResponse = response.json().await?;
        Ok(BigDecimal::from_str(&payload.result.total_balance)?)
    }
}

/// Builds the correct fetcher implementation for a configured chain identifier.
pub async fn build_fetcher(
    chain_identifier: &str,
    rpc_url: &str,
) -> eyre::Result<Box<dyn LiquidityFetcher>> {
    let url = Url::parse(rpc_url)?;
    if chain_identifier.contains("bitcoin")
        || chain_identifier.contains("litecoin")
        || chain_identifier.contains("alpen")
    {
        Ok(Box::new(BitcoinLiquidityFetcher::new(url)))
    } else if chain_identifier.contains("solana") {
        Ok(Box::new(SolanaLiquidityFetcher::new(url)))
    } else if chain_identifier.contains("starknet") {
        Ok(Box::new(StarknetLiquidityFetcher::new(url)))
    } else if chain_identifier.contains("sui") {
        Ok(Box::new(SuiLiquidityFetcher::new(url)))
    } else {
        Ok(Box::new(EvmLiquidityFetcher::new(url)))
    }
}
