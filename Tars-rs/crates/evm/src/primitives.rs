use crate::{
    htlc::traits::HTLCInterface, UnipayHTLCContract, UnipayHTLCv2Contract, UnipayHTLCv3Contract, NativeHTLCContract, NativeHTLCv2Contract, NativeHTLCv3Contract
};
use alloy::{
    dyn_abi::Eip712Domain,
    hex,
    network::EthereumWallet,
    primitives::{Address, Bytes, U256},
    providers::{
        fillers::{
            ChainIdFiller, GasFiller, JoinFill, NonceFiller, SimpleNonceManager, WalletFiller,
        },
        Identity, RootProvider,
    },
    sol,
};
use alloy_rpc_types_eth::AccessList;
use orderbook::primitives::EVMSwap;
use primitives::HTLCAction;
use serde::{Deserialize, Serialize};

/// Provider type alias for Alloy contracts with necessary fillers for transaction execution
///
/// This type combines several fillers to handle:
/// - Gas estimation
/// - Nonce management
/// - Chain ID retrieval
/// - Wallet operations like signing
pub type AlloyProvider = alloy::providers::fillers::FillProvider<
    JoinFill<
        JoinFill<
            JoinFill<JoinFill<Identity, GasFiller>, NonceFiller<SimpleNonceManager>>,
            ChainIdFiller,
        >,
        WalletFiller<EthereumWallet>,
    >,
    RootProvider,
>;

sol! {
    struct Refund {
        bytes32 orderId;
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UnipayHandlerType {
    HTLC,
    // HTLCRegistry, (Currently Unsupported)
}

impl From<&UnipayActionType> for UnipayHandlerType {
    fn from(action_type: &UnipayActionType) -> Self {
        match action_type {
            UnipayActionType::HTLC(_) => UnipayHandlerType::HTLC,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UnipayActionType {
    HTLC(HTLCAction),
    // Any other contract type and its action can be added here
    // For example:
    // HTLCRegistry(HTLCRegistryAction)
} 

impl std::fmt::Display for UnipayActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UnipayActionType::HTLC(action) => write!(f, "HTLC({})", action),
        }
    }
}

/// A request to interact with an HTLC contract.
///
/// This struct combines the method to be executed, the swap metadata, and the asset address.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnipayActionRequest {
    /// The action to invoke.
    pub action: UnipayActionType,

    /// The swap data containing information about the transaction and parties involved.
    pub swap: EVMSwap,

    /// The address HTLC.
    pub asset: String,

    /// The unique identifier for the HTLC request.
    pub id: String,
}

/// Describes the type of token involved in the HTLC.
#[derive(Debug, Clone)]
pub enum TokenType {
    /// An ERC-20 compliant token.
    ERC20,

    /// A native blockchain token (e.g., ETH).
    Native,
    // /// A token filled by Aave bridge (currently unsupported).
    // Aave,
}

// Enum that directly holds trait objects
pub enum HTLCContract {
    /// HTLC contract handling ERC-20 token swaps.
    ERC20HTLC {
        contract: Box<dyn HTLCInterface + Send + Sync>,
    },

    /// HTLC v2 contract handling ERC-20 token swaps with additional features.
    ERC20HTLCv2 {
        contract: Box<dyn HTLCInterface + Send + Sync>,
    },

    /// HTLC v3 contract handling ERC-20 token swaps with additional features.
    ERC20HTLCv3 {
        contract: Box<dyn HTLCInterface + Send + Sync>,
    },

    /// HTLC contract handling native token swaps.
    NativeHTLC {
        contract: Box<dyn HTLCInterface + Send + Sync>,
    },

    /// HTLC contract handling native token swaps.
    NativeHTLCv2 {
        contract: Box<dyn HTLCInterface + Send + Sync>,
    },

    /// HTLC contract handling native token swaps.
    NativeHTLCv3 {
        contract: Box<dyn HTLCInterface + Send + Sync>,
    },
    // /// HTLC contract for Aave-wrapped assets (currently unsupported).
    // AaveHTLC { contract: Box<dyn HTLCCalldata + Send + Sync> }
}

impl Clone for HTLCContract {
    fn clone(&self) -> Self {
        match self {
            HTLCContract::ERC20HTLC { contract } => HTLCContract::ERC20HTLC {
                contract: contract.clone_box(),
            },
            HTLCContract::ERC20HTLCv2 { contract } => HTLCContract::ERC20HTLCv2 {
                contract: contract.clone_box(),
            },
            HTLCContract::ERC20HTLCv3 { contract } => HTLCContract::ERC20HTLCv3 {
                contract: contract.clone_box(),
            },
            HTLCContract::NativeHTLC { contract } => HTLCContract::NativeHTLC {
                contract: contract.clone_box(),
            },
            HTLCContract::NativeHTLCv2 { contract } => HTLCContract::NativeHTLCv2 {
                contract: contract.clone_box(),
            },
            HTLCContract::NativeHTLCv3 { contract } => HTLCContract::NativeHTLCv3 {
                contract: contract.clone_box(),
            },
        }
    }
}

impl HTLCContract {
    // Factory methods for creating the enum variants
    pub fn new_erc20_htlc(contract: UnipayHTLCContract) -> Self {
        Self::ERC20HTLC {
            contract: Box::new(contract),
        }
    }

    pub fn new_erc20_htlc_v2(contract: UnipayHTLCv2Contract) -> Self {
        Self::ERC20HTLCv2 {
            contract: Box::new(contract),
        }
    }

    pub fn new_erc20_htlc_v3(contract: UnipayHTLCv3Contract) -> Self {
        Self::ERC20HTLCv3 {
            contract: Box::new(contract),
        }
    }

    pub fn new_native_htlc(contract: NativeHTLCContract) -> Self {
        Self::NativeHTLC {
            contract: Box::new(contract),
        }
    }

    pub fn new_native_htlc_v2(contract: NativeHTLCv2Contract) -> Self {
        Self::NativeHTLCv2 {
            contract: Box::new(contract),
        }
    }

    pub fn new_native_htlc_v3(contract: NativeHTLCv3Contract) -> Self {
        Self::NativeHTLCv3 {
            contract: Box::new(contract),
        }
    }

    pub fn is_native(&self) -> bool {
        matches!(
            self,
            HTLCContract::NativeHTLC { .. } | HTLCContract::NativeHTLCv3 { .. } | HTLCContract::NativeHTLCv2 { .. }
        )
    }

    // Direct access to the trait object
    pub fn contract(&self) -> &dyn HTLCInterface {
        match self {
            HTLCContract::ERC20HTLC { contract } => contract.as_ref(),
            HTLCContract::ERC20HTLCv2 { contract } => contract.as_ref(),
            HTLCContract::ERC20HTLCv3 { contract } => contract.as_ref(),
            HTLCContract::NativeHTLC { contract } => contract.as_ref(),
            HTLCContract::NativeHTLCv2 { contract } => contract.as_ref(),
            HTLCContract::NativeHTLCv3 { contract } => contract.as_ref(),
        }
    }
}

/// Additional options for transaction execution
#[derive(Debug, Clone)]
pub struct TxOptions {
    /// The maximum priority fee per gas to be paid for the transaction.
    pub max_priority_fee_per_gas: Option<u128>,
    /// The maximum fee per gas to be paid for the transaction.
    pub max_fee_per_gas: Option<u128>,
    /// The gas limit for the transaction.
    pub gas_limit: Option<u64>,
    /// The nonce for the transaction.
    pub nonce: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct CallParams {
    pub to: Address,
    pub data: Bytes,
    pub value: Option<U256>,
    pub access_list: Option<AccessList>,
}

impl CallParams {
    pub fn new(to: Address, data: Bytes) -> Self {
        Self {
            to,
            data,
            value: None,
            access_list: None,
        }
    }

    pub fn value(mut self, value: U256) -> Self {
        self.value = Some(value);
        self
    }

    pub fn access_list(mut self, access_list: AccessList) -> Self {
        self.access_list = Some(access_list);
        self
    }
    
}

#[derive(Debug, Clone)]
pub struct SwapInfo {
    pub initiator: Address,
    pub redeemer: Address,
    pub timelock: U256,
    pub amount: U256,
    pub is_fulfilled: bool,
    pub initiated_at: U256,
}

impl From<crate::UnipayHTLC::ordersReturn> for SwapInfo {
    fn from(order: crate::UnipayHTLC::ordersReturn) -> Self {
        Self {
            initiator: order.initiator,
            redeemer: order.redeemer,
            timelock: order.timelock,
            amount: order.amount,
            is_fulfilled: order.isFulfilled,
            initiated_at: order.initiatedAt,
        }
    }
}

impl From<crate::NativeHTLC::ordersReturn> for SwapInfo {
    fn from(order: crate::NativeHTLC::ordersReturn) -> Self {
        Self {
            initiator: order.initiator,
            redeemer: order.redeemer,
            timelock: order.timelock,
            amount: order.amount,
            is_fulfilled: order.isFulfilled,
            initiated_at: order.initiatedAt,
        }
    }
}

impl From<crate::UnipayHTLCv2::ordersReturn> for SwapInfo {
    fn from(order: crate::UnipayHTLCv2::ordersReturn) -> Self {
        let is_fulfilled = order.fulfilledAt > U256::ZERO;
        Self {
            initiator: order.initiator,
            redeemer: order.redeemer,
            timelock: order.timelock,
            amount: order.amount,
            is_fulfilled,
            initiated_at: order.initiatedAt,
        }
    }
}

impl From<crate::NativeHTLCv2::ordersReturn> for SwapInfo {
    fn from(order: crate::NativeHTLCv2::ordersReturn) -> Self {
        let is_fulfilled = order.fulfilledAt > U256::ZERO;
        Self {
            initiator: order.initiator,
            redeemer: order.redeemer,
            timelock: order.timelock,
            amount: order.amount,
            is_fulfilled,
            initiated_at: order.initiatedAt,
        }
    }
}

impl From<crate::UnipayHTLCv3::ordersReturn> for SwapInfo {
    fn from(order: crate::UnipayHTLCv3::ordersReturn) -> Self {
        let is_fulfilled = order.fulfilledAt > U256::ZERO;
        Self {
            initiator: order.initiator,
            redeemer: order.redeemer,
            timelock: order.timelock,
            amount: order.amount,
            is_fulfilled,
            initiated_at: order.initiatedAt,
        }
    }
}

impl From<crate::NativeHTLCv3::ordersReturn> for SwapInfo {
    fn from(order: crate::NativeHTLCv3::ordersReturn) -> Self {
        let is_fulfilled = order.fulfilledAt > U256::ZERO;
        Self {
            initiator: order.initiator,
            redeemer: order.redeemer,
            timelock: order.timelock,
            amount: order.amount,
            is_fulfilled,
            initiated_at: order.initiatedAt,
        }
    }
}

/// Represents a single request's calls and metadata for multicall execution
#[derive(Debug)]
pub struct RequestCallBatch {
    /// Index of the original request
    pub request_index: usize,
    /// Starting position of this request's calls in the multicall
    pub call_start_index: usize,
    /// Number of calls for this request
    pub call_count: usize,
}

/// Result for a single HTLC request simulation
#[derive(Debug, Clone)]
pub enum SimulationResult {
    /// The request validation passed and all calls succeeded
    Success,
    /// Validation failed or any call failed (includes decoded return data)
    Error(String),
}

impl Default for SimulationResult {
    fn default() -> Self {
        SimulationResult::Error("".to_string())
    }
}

impl SimulationResult {
    /// Creates an error result from a string error message
    pub fn error(error: impl Into<String>) -> Self {
        SimulationResult::Error(error.into())
    }

    /// Creates an error result from bytes (converts bytes to readable error string)
    pub fn error_bytes(bytes: &Bytes) -> Self {
        SimulationResult::Error(bytes_to_error_string(bytes))
    }

    /// Creates a success result
    pub fn success() -> Self {
        SimulationResult::Success
    }

    /// Returns true if the result is success
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn error_message(&self) -> Option<String> {
        match &self {
            &SimulationResult::Error(msg) if !msg.is_empty() => Some(msg.clone()),
            _ => None,
        }
    }
}

/// Converts bytes to a readable string for error reporting
fn bytes_to_error_string(bytes: &Bytes) -> String {
    if bytes.is_empty() {
        return "Empty error data".to_string();
    }

    // Try to decode as UTF-8 string first
    if let Ok(utf8_str) = std::str::from_utf8(bytes) {
        return format!("Error data (UTF-8): {}", utf8_str);
    }

    // Fall back to hex representation
    format!("Error data (hex): 0x{}", hex::encode(bytes))
}

/// Typed data for an EVM swap
///
/// This struct contains the necessary parameters for an EVM swap.
/// It includes the domain, the primary type, the types, and the message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InitiateTypedData {
    pub domain: Eip712Domain,
    #[serde(rename = "primaryType")]
    pub primary_type: String,
    pub types: serde_json::Value,
    pub message: serde_json::Value,
}
