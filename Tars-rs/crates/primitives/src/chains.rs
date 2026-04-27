use serde::{Deserialize, Serialize};
use std::fmt::{self, Display};

const STARKNET: &str = "starknet";
const STARKNET_SEPOLIA: &str = "starknet_sepolia";
const STARKNET_DEVNET: &str = "starknet_devnet";

const SOLANA: &str = "solana";
const SOLANA_TESTNET: &str = "solana_testnet";
const SOLANA_LOCALNET: &str = "solana_localnet";

pub const BITCOIN: &str = "bitcoin";
pub const BITCOIN_TESTNET: &str = "bitcoin_testnet";
pub const BITCOIN_REGTEST: &str = "bitcoin_regtest";

const SUI: &str = "sui";
const SUI_TESTNET: &str = "sui_testnet";
const SUI_LOCALNET: &str = "sui_localnet";

/// Supported blockchain networks in the Unipay Ecosystem
///
/// This enum represents the different networks that are supported
/// in the Unipay Ecosystem. Each variant corresponds to a specific blockchain
/// or category of blockchains.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChainType {
    Starknet,
    /// EVM-compatible blockchains (Ethereum, Arbitrum, Base, etc.)
    Evm,
    Solana,
    Bitcoin,
    Sui
}

impl ChainType {
    /// Returns true if addresses on this chain are case-sensitive.
    ///
    /// Bitcoin (bech32/base58) and Solana (base58) addresses must preserve case;
    /// EVM/Starknet/Sui addresses are normalized to lowercase.
    pub fn is_address_case_sensitive(&self) -> bool {
        matches!(self, ChainType::Bitcoin | ChainType::Solana)
    }
}

/// Returns true if the given chain identifier maps to a chain whose addresses
/// are case-sensitive (Bitcoin or Solana).
pub fn is_address_case_sensitive(chain: &str) -> bool {
    ChainType::from(chain).is_address_case_sensitive()
}

/// Normalize an address string for the given chain: lowercases for chains with
/// case-insensitive addresses (EVM and friends), preserves case for Bitcoin/Solana.
pub fn normalize_address(addr: &str, chain: &str) -> String {
    if is_address_case_sensitive(chain) {
        addr.to_string()
    } else {
        addr.to_lowercase()
    }
}

impl From<&str> for ChainType {
    /// Converts a string identifier to a `ChainType` variant
    ///
    /// # Arguments
    ///
    /// * `chain` - A string representing the blockchain identifier
    ///
    /// # Returns
    ///
    /// Returns the corresponding `ChainType` variant. If the string
    /// doesn't match any known chain identifiers, it defaults to `EVM`.
    ///
    /// # Examples
    ///
    /// ```
    /// use unipay_primitives::chains::ChainType;
    ///
    /// let chain = ChainType::from("starknet");
    /// assert!(matches!(chain, ChainType::Starknet));
    ///
    /// let chain = ChainType::from("ethereum");
    /// assert!(matches!(chain, ChainType::EVM));
    /// ```
    fn from(chain: &str) -> Self {
        match chain {
            STARKNET | STARKNET_SEPOLIA | STARKNET_DEVNET => Self::Starknet,
            SOLANA | SOLANA_TESTNET | SOLANA_LOCALNET => Self::Solana,
            BITCOIN | BITCOIN_TESTNET | BITCOIN_REGTEST => Self::Bitcoin,
            SUI | SUI_TESTNET | SUI_LOCALNET => Self::Sui,
            _ => Self::Evm,
        }
    }
}

impl From<String> for ChainType {
    /// Converts a `String` identifier to a `ChainType` variant
    ///
    /// # Arguments
    ///
    /// * `chain` - A `String` representing the blockchain identifier
    ///
    /// # Returns
    ///
    /// Returns the corresponding `ChainType` variant. If the string
    /// doesn't match any known chain identifiers, it defaults to `EVM`.
    ///
    /// # Examples
    ///
    /// ```
    /// use unipay_primitives::chains::ChainType;
    ///
    /// let chain = ChainType::from("starknet".to_string());
    /// assert!(matches!(chain, ChainType::Starknet));
    /// ```
    fn from(chain: String) -> Self {
        Self::from(chain.as_str())
    }
}

impl Display for ChainType {
    /// Formats the `ChainType` for display
    ///
    /// Returns the debug representation of the chain type as a string.
    ///
    /// # Examples
    ///
    /// ```
    /// use unipay_primitives::chains::ChainType;
    ///
    /// let chain = ChainType::Starknet;
    /// assert_eq!(chain.to_string(), "Starknet");
    /// ```
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            ChainType::Evm => "evm",
            ChainType::Bitcoin => "bitcoin",
            ChainType::Solana => "solana",
            ChainType::Starknet => "starknet",
            ChainType::Sui => "sui",
        };
        write!(f, "{}", s)
    }
}
