use crate::ChainType;
use alloy::primitives::Bytes;
use axum::http::HeaderMap;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Asset identifier in format "chain:token"
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetId {
    chain: String,
    token: String,
}

impl AssetId {
    /// Create a new AssetId from chain and token
    pub fn new(chain: impl Into<String>, token: impl Into<String>) -> Self {
        AssetId {
            chain: chain.into(),
            token: token.into(),
        }
    }

    /// Get the chain
    pub fn chain(&self) -> &str {
        &self.chain
    }

    /// Get the token
    pub fn token(&self) -> &str {
        &self.token
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.chain, self.token)
    }
}

impl FromStr for AssetId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() != 2 {
            return Err(format!("Invalid asset ID format: '{}'. Expected 'chain:token'", s));
        }
        
        let chain = parts[0].trim();
        let token = parts[1].trim();
        
        if chain.is_empty() || token.is_empty() {
            return Err("Invalid asset ID: chain and token cannot be empty".to_string());
        }
        
        Ok(AssetId {
            chain: chain.to_string(),
            token: token.to_string(),
        })
    }
}

impl Serialize for AssetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AssetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// Implement From conversions for seamless integration
impl From<AssetId> for String {
    fn from(asset_id: AssetId) -> Self {
        asset_id.to_string()
    }
}

impl From<&AssetId> for String {
    fn from(asset_id: &AssetId) -> Self {
        asset_id.to_string()
    }
}

// Represents action that can be perfomed on the HTLC
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum HTLCAction {
    // Represents initiate action
    Initiate,
    /// Represents an HTLC initiate action with a self-generated signature.
    ///
    /// The signature generation must be performed within the action executor.
    InitiateWithSignature,
    /// Represents an HTLC initiate action with a user generated signature.
    InitiateWithUserSignature {
        signature: Bytes,
    },
    // Represents an HTLC redeem action
    Redeem {
        secret: Bytes,
    },
    // Represents an HTLC Refund action
    Refund,
    // Represents an HTLC Instant Refund action
    InstantRefund,
    // Represents no action
    NoOp,
}

impl fmt::Display for HTLCAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HTLCAction::Initiate => write!(f, "Initiate"),
            HTLCAction::InitiateWithSignature => write!(f, "InitiateWithSignature"),
            HTLCAction::InitiateWithUserSignature { .. } => write!(f, "InitiateWithUserSignature"),
            HTLCAction::Redeem { .. } => write!(f, "Redeem"),
            HTLCAction::Refund => write!(f, "Refund"),
            HTLCAction::InstantRefund => write!(f, "InstantRefund"),
            HTLCAction::NoOp => write!(f, "NoOp"),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum ExecuteActionRequest {
    // Represents an HTLC initiate action with a user signature.
    Initiate { signature: String },
    // Represents an HTLC redeem action
    Redeem { secret: String },
    // Represents submitting signatures for instant refund
    InstantRefund { signatures: Vec<String> },
    // Represents a refund action
    Refund { recipient: String },
}

#[derive(Debug, Clone)]
pub struct HTLCActionRequest {
    pub action: ExecuteActionRequest,
    pub headers: HeaderMap,
}

impl fmt::Display for ExecuteActionRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecuteActionRequest::Initiate { .. } => write!(f, "initiate"),
            ExecuteActionRequest::Redeem { .. } => write!(f, "redeem"),
            ExecuteActionRequest::InstantRefund { .. } => write!(f, "instant-refund"),
            ExecuteActionRequest::Refund { .. } => write!(f, "refund"),
        }
    }
}

/// Represents the version of the HTLC contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum HTLCVersion {
    /// Version 1 of the HTLC contract.
    V1,

    /// Version 2 of the HTLC contract.
    V2,

    /// Version 3 of the HTLC contract.
    V3,
}

impl HTLCVersion {
    /// Returns the version as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            HTLCVersion::V1 => "v1",
            HTLCVersion::V2 => "v2",
            HTLCVersion::V3 => "v3",
        }
    }

    /// Parse a string into an HTLCVersion.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "v1" => Some(HTLCVersion::V1),
            "v2" => Some(HTLCVersion::V2),
            "v3" => Some(HTLCVersion::V3),
            _ => None,
        }
    }

    /// Returns an iterator over all HTLC versions.
    pub fn all() -> impl Iterator<Item = Self> {
        [HTLCVersion::V1, HTLCVersion::V2, HTLCVersion::V3].into_iter()
    }
}

impl Default for HTLCVersion {
    fn default() -> Self {
        HTLCVersion::V1
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Chain {
    /// The blockchain network identifier (e.g., "bitcoin_testnet", "arbitrum_sepolia")
    pub chain: String,
    /// Chain id for the chain (e.g., "evm:1", "bitcoin")
    pub id: String,
    /// URL to the chain's icon
    pub icon: String,
    /// Explorer url for the chain
    pub explorer_url: String,
    /// Confirmation target for the chain
    pub confirmation_target: u64,
    /// Time in blocks for the source chain's HTLC expiry
    pub source_timelock: String,
    /// Time in blocks for the destination chain's HTLC expiry
    pub destination_timelock: String,
    /// List of supported HTLC schemas
    pub supported_htlc_schemas: Vec<String>,
    /// List of supported token schemas
    pub supported_token_schemas: Vec<String>,
    /// List of assets supported on this chain
    pub assets: Vec<Asset>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ContractInfo {
    pub address: String,
    pub schema: Option<String>,
}

impl ContractInfo {
    pub fn is_primary(&self) -> bool {
        self.address == "primary"
            && (self.schema.is_none() || self.schema.as_ref() == Some(&"primary".to_string()))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    /// Unique identifier for the asset (e.g., "bitcoin:btc", "ethereum:wbtc", "solana:sol")
    pub id: AssetId,
    /// Chain identifier for the asset (e.g., "bitcoin_testnet", "arbitrum_sepolia")
    pub chain: String,
    /// URL to the asset's icon
    pub icon: String,
    /// HTLC contract info
    pub htlc: ContractInfo,
    /// Token contract info
    pub token: ContractInfo,
    /// Number of decimal places for the asset
    pub decimals: u8,
    /// Minimum supported amount (in smallest unit)
    pub min_amount: String,
    /// Maximum supported amount (in smallest unit)
    pub max_amount: String,
    /// Chain id for the asset
    pub chain_id: Option<String>,
    /// URL to the chain's logo
    pub chain_icon: String,
    /// Chain type for the asset
    pub chain_type: ChainType,
    /// Explorer url for the chain
    pub explorer_url: String,
    /// FIAT price of the token
    pub price: Option<f64>, // Option as this does not exist in cdn
    /// Version of the HTLC Contract
    pub version: HTLCVersion,
    /// Minimum timelock for the asset
    pub min_timelock: u64,
    /// Id for fetching the price of the token
    pub token_id: String,
    /// Solver address
    pub solver: String
}

impl Asset {
    /// Custom serialization method for the chain field that combines chain_type and chain_id
    pub fn serialize_chain(&self) -> String {
        match &self.chain_id {
            Some(chain_id) => format!("{}:{}", self.chain_type, chain_id),
            None => self.chain_type.to_string(),
        }
    }
}

impl serde::Serialize for Asset {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("Asset", 9)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("chain", &self.serialize_chain())?;
        state.serialize_field("icon", &self.icon)?;

        // Serialize htlc with custom logic
        if self.htlc.is_primary() {
            state.serialize_field("htlc", &Option::<ContractInfo>::None)?;
        } else {
            state.serialize_field("htlc", &self.htlc)?;
        }

        // Serialize token with custom logic
        if self.token.is_primary() {
            state.serialize_field("token", &Option::<ContractInfo>::None)?;
        } else {
            state.serialize_field("token", &self.token)?;
        }

        state.serialize_field("decimals", &self.decimals)?;
        state.serialize_field("min_amount", &self.min_amount)?;
        state.serialize_field("max_amount", &self.max_amount)?;
        state.serialize_field("price", &self.price)?;
        state.end()
    }
}

/// Enum for direction of pair
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairDirection {
    /// Forward direction (one way)
    Forward,
    /// Both directions (two way)
    Both,
}

impl fmt::Display for PairDirection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PairDirection::Forward => write!(f, "->"),
            PairDirection::Both => write!(f, "<->"),
        }
    }
}

impl FromStr for PairDirection {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "->" => Ok(PairDirection::Forward),
            "<->" => Ok(PairDirection::Both),
            _ => Err(format!(
                "Invalid pair direction: '{}'. Expected '->' or '<->'",
                s
            )),
        }
    }
}

impl Serialize for PairDirection {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            PairDirection::Forward => serializer.serialize_str("->"),
            PairDirection::Both => serializer.serialize_str("<->"),
        }
    }
}

impl<'de> Deserialize<'de> for PairDirection {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for AssetPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} {}", self.0, self.1, self.2)
    }
}

/// Type for (from_ident, direction, to_ident)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPair(pub AssetId, pub PairDirection, pub AssetId);

impl FromStr for AssetPair {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split_whitespace().collect();

        if parts.len() != 3 {
            return Err(format!(
                "Invalid asset pair format: '{}'. Expected 'from_asset <direction> to_asset'",
                s
            ));
        }
        let from_ident = AssetId::from_str(parts[0])?;
        let direction = parts[1].parse::<PairDirection>()?;
        let to_ident = AssetId::from_str(parts[2])?;

        Ok(AssetPair(
            from_ident,
            direction,
            to_ident,
        ))
    }
}

impl Serialize for AssetPair {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for AssetPair {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Into<String> for AssetPair {
    fn into(self) -> String {
        self.to_string()
    }
}

impl Into<(AssetId, PairDirection, AssetId)> for AssetPair {
    fn into(self) -> (AssetId, PairDirection, AssetId) {
        (self.0, self.1, self.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_chain_serialization() {
        // Test case 1: chain_id is Some
        let asset_with_chain_id = Asset {
            id: AssetId::new("ethereum_localnet", "wbtc"),
            chain: "ethereum_localnet".to_string(),
            icon: "icon.png".to_string(),
            htlc: ContractInfo {
                address: "0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0".to_string(),
                schema: Some("evm:htlc_erc20".to_string()),
            },
            token: ContractInfo {
                address: "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512".to_string(),
                schema: Some("evm:erc20".to_string()),
            },
            decimals: 8,
            min_amount: "1000".to_string(),
            max_amount: "1000000".to_string(),
            chain_id: Some("31337".to_string()),
            chain_icon: "chain_icon.png".to_string(),
            chain_type: ChainType::Evm,
            price: None,
            version: HTLCVersion::V2,
            explorer_url: "http://localhost:5100".to_string(),
            min_timelock: 100,
            token_id: "wbtc".to_string(),
            solver: "0x0000000000000000000000000000000000000000".to_string(),
        };

        let serialized = serde_json::to_string(&asset_with_chain_id).unwrap();
        println!("{}", serialized);
        assert!(serialized.contains("\"chain\":\"evm:31337\""));
        assert!(serialized.contains("\"htlc\":{\"address\":\"0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0\",\"schema\":\"evm:htlc_erc20\"}"));
        assert!(serialized.contains("\"token\":{\"address\":\"0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512\",\"schema\":\"evm:erc20\"}"));

        // Test case 2: chain_id is None
        let asset_without_chain_id = Asset {
            id: AssetId::new("bitcoin_regtest", "btc"),
            chain: "bitcoin_regtest".to_string(),
            icon: "icon.png".to_string(),
            htlc: ContractInfo {
                address: "primary".to_string(),
                schema: Some("primary".to_string()),
            },
            token: ContractInfo {
                address: "primary".to_string(),
                schema: Some("primary".to_string()),
            },
            decimals: 8,
            min_amount: "1000".to_string(),
            max_amount: "1000000".to_string(),
            chain_id: None,
            chain_icon: "chain_icon.png".to_string(),
            chain_type: ChainType::Bitcoin,
            price: None,
            version: HTLCVersion::V2,
            explorer_url: "http://localhost:5050".to_string(),
            min_timelock: 100,
            token_id: "btc".to_string(),
            solver: "0x0000000000000000000000000000000000000000".to_string(),
        };

        let serialized = serde_json::to_string(&asset_without_chain_id).unwrap();
        assert!(serialized.contains("\"chain\":\"bitcoin\""));
        assert!(serialized.contains("\"htlc\":null"));
        assert!(serialized.contains("\"token\":null"));

        assert_eq!(asset_with_chain_id.serialize_chain(), "evm:31337");
        assert_eq!(asset_without_chain_id.serialize_chain(), "bitcoin");
    }

    #[test]
    fn test_htlc_version_ordering() {
        assert!(HTLCVersion::V1 < HTLCVersion::V2);
        assert!(HTLCVersion::V2 < HTLCVersion::V3);
        assert_eq!(HTLCVersion::V1.max(HTLCVersion::V2), HTLCVersion::V2);
    }

    #[test]
    fn test_pair_direction() {
        // Test Display trait and to_string()
        assert_eq!(PairDirection::Forward.to_string(), "->");
        assert_eq!(PairDirection::Both.to_string(), "<->");
        assert_eq!(format!("{}", PairDirection::Forward), "->");
        assert_eq!(format!("{}", PairDirection::Both), "<->");

        // Test FromStr trait - Valid cases
        assert_eq!(
            "->".parse::<PairDirection>().unwrap(),
            PairDirection::Forward
        );
        assert_eq!("<->".parse::<PairDirection>().unwrap(), PairDirection::Both);

        // Test FromStr with whitespace
        assert_eq!(
            " -> ".parse::<PairDirection>().unwrap(),
            PairDirection::Forward
        );
        assert_eq!(
            " <-> ".parse::<PairDirection>().unwrap(),
            PairDirection::Both
        );

        // Test FromStr - Invalid cases
        assert!("invalid".parse::<PairDirection>().is_err());
        assert!("-->".parse::<PairDirection>().is_err());
        assert!("<-->".parse::<PairDirection>().is_err());
        assert!("".parse::<PairDirection>().is_err());

        // Test JSON serialization
        let single_json = serde_json::to_string(&PairDirection::Forward).unwrap();
        let bidirectional_json = serde_json::to_string(&PairDirection::Both).unwrap();
        assert_eq!(single_json, "\"->\"");
        assert_eq!(bidirectional_json, "\"<->\"");

        // Test JSON deserialization - Valid cases
        let single: PairDirection = serde_json::from_str("\"->\"").unwrap();
        let bidirectional: PairDirection = serde_json::from_str("\"<->\"").unwrap();
        assert_eq!(single, PairDirection::Forward);
        assert_eq!(bidirectional, PairDirection::Both);

        // Test JSON deserialization with whitespace
        let single_trimmed: PairDirection = serde_json::from_str("\" -> \"").unwrap();
        let bidirectional_trimmed: PairDirection = serde_json::from_str("\" <-> \"").unwrap();
        assert_eq!(single_trimmed, PairDirection::Forward);
        assert_eq!(bidirectional_trimmed, PairDirection::Both);

        // Test JSON deserialization errors
        assert!(serde_json::from_str::<PairDirection>("\"invalid\"").is_err());
        assert!(serde_json::from_str::<PairDirection>("\"-->\"").is_err());
        assert!(serde_json::from_str::<PairDirection>("\"<-->\"").is_err());
        assert!(serde_json::from_str::<PairDirection>("\"\"").is_err());

        // Test roundtrip serialization/deserialization
        let original_single = PairDirection::Forward;
        let original_bidirectional = PairDirection::Both;

        let serialized_single = serde_json::to_string(&original_single).unwrap();
        let deserialized_single: PairDirection = serde_json::from_str(&serialized_single).unwrap();
        assert_eq!(original_single, deserialized_single);

        let serialized_bidirectional = serde_json::to_string(&original_bidirectional).unwrap();
        let deserialized_bidirectional: PairDirection =
            serde_json::from_str(&serialized_bidirectional).unwrap();
        assert_eq!(original_bidirectional, deserialized_bidirectional);
    }

    #[test]
    fn test_asset_pair() {
        let asset_pair = AssetPair(
            AssetId::new("arbitrum", "seed"),
            PairDirection::Both,
            AssetId::new("ethereum", "seed"),
        );

        // Test Display
        assert_eq!(asset_pair.to_string(), "arbitrum:seed <-> ethereum:seed");
        assert_eq!(format!("{}", asset_pair), "arbitrum:seed <-> ethereum:seed");

        // Test FromStr
        let parsed_pair: AssetPair = "arbitrum:seed <-> ethereum:seed".parse().unwrap();
        assert_eq!(parsed_pair, asset_pair);

        let single_pair: AssetPair = "bitcoin:btc -> ethereum:eth".parse().unwrap();
        assert_eq!(
            single_pair,
            AssetPair(
                AssetId::new("bitcoin", "btc"),
                PairDirection::Forward,
                AssetId::new("ethereum", "eth")
            )
        );

        // Test with whitespace
        let parsed_with_whitespace: AssetPair =
            "  arbitrum:seed  <->  ethereum:seed  ".parse().unwrap();
        assert_eq!(parsed_with_whitespace, asset_pair);

        // Test invalid formats
        assert!("invalid format".parse::<AssetPair>().is_err());
        assert!("arbitrum:seed".parse::<AssetPair>().is_err());
        assert!("arbitrum:seed ->".parse::<AssetPair>().is_err());
        assert!("-> ethereum:seed".parse::<AssetPair>().is_err());

        // Test JSON serialization/deserialization
        let serialized = serde_json::to_string(&asset_pair).unwrap();
        assert_eq!(serialized, "\"arbitrum:seed <-> ethereum:seed\"");

        let deserialized: AssetPair = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, asset_pair);

        // Test roundtrip for AssetPair
        let roundtrip_serialized = serde_json::to_string(&asset_pair).unwrap();
        let roundtrip_deserialized: AssetPair =
            serde_json::from_str(&roundtrip_serialized).unwrap();
        assert_eq!(asset_pair, roundtrip_deserialized);

        let asset_pair = AssetPair::from_str("bitcoin:btc -> ethereum:eth").unwrap();
        let from_ident = asset_pair.0.clone();
        let direction = asset_pair.1.clone();
        let to_ident = asset_pair.2.clone();
        assert_eq!(from_ident, AssetId::new("bitcoin", "btc"));
        assert_eq!(direction, PairDirection::Forward);
        assert_eq!(to_ident, AssetId::new("ethereum", "eth"));

        let asset_string = asset_pair.to_string();
        assert_eq!(asset_string, "bitcoin:btc -> ethereum:eth");

        let asset_tuple: (AssetId, PairDirection, AssetId) = asset_pair.into();
        assert_eq!(asset_tuple, (AssetId::new("bitcoin", "btc"), PairDirection::Forward, AssetId::new("ethereum", "eth")));
    }

    #[test]
    fn test_asset_id() {
        // Test creation
        let asset_id = AssetId::new("bitcoin", "btc");
        assert_eq!(asset_id.chain(), "bitcoin");
        assert_eq!(asset_id.token(), "btc");
        assert_eq!(asset_id.to_string(), "bitcoin:btc");

        // Test FromStr parsing
        let parsed: AssetId = "ethereum:wbtc".parse().unwrap();
        assert_eq!(parsed.chain(), "ethereum");
        assert_eq!(parsed.token(), "wbtc");

        // Test with whitespace
        let parsed_trimmed: AssetId = "  arbitrum  :  seed  ".parse().unwrap();
        assert_eq!(parsed_trimmed.chain(), "arbitrum");
        assert_eq!(parsed_trimmed.token(), "seed");

        // Test invalid formats
        assert!("invalid".parse::<AssetId>().is_err());
        assert!("chain:".parse::<AssetId>().is_err());
        assert!(":token".parse::<AssetId>().is_err());
        assert!("chain:token:extra".parse::<AssetId>().is_err());

        // Test conversion to String
        let asset_id = AssetId::new("bitcoin", "btc");
        let string: String = asset_id.clone().into();
        assert_eq!(string, "bitcoin:btc");

        // Test JSON serialization/deserialization
        let serialized = serde_json::to_string(&asset_id).unwrap();
        assert_eq!(serialized, "\"bitcoin:btc\"");

        let deserialized: AssetId = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, asset_id);

        // Test roundtrip
        let roundtrip_serialized = serde_json::to_string(&asset_id).unwrap();
        let roundtrip_deserialized: AssetId = serde_json::from_str(&roundtrip_serialized).unwrap();
        assert_eq!(asset_id, roundtrip_deserialized);
    }
}
