//! Primitive data types for orderbook

use crate::errors::OrderbookError;
use alloy::{
    hex::FromHex,
    primitives::{ruint::aliases::U256, Address, Bytes, FixedBytes},
};
use eyre::Result;
use primitives::{HTLCAction, HTLCVersion};
use serde::{Deserialize, Serialize};
use sqlx::types::BigDecimal;
use std::collections::HashSet;
use std::fmt::Display;
use std::str::FromStr;
use utils::deserialize_csv_field;

/// Supported Bitcoin networks
const BITCOIN_MAINNET: &str = "bitcoin";
const BITCOIN_TESTNET: &str = "bitcoin_testnet";
const BITCOIN_REGTEST: &str = "bitcoin_regtest";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SwapChain {
    Source,
    Destination,
}

impl Display for SwapChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapChain::Source => write!(f, "source"),
            SwapChain::Destination => write!(f, "destination"),
        }
    }
}
/// No of items in a page. Eg: A client can only query 500 orders per page
pub const PAGE_LIMIT: i64 = 500;

#[derive(Deserialize)]
#[serde(default)]
pub struct OrderQueryFilters {
    pub page: i64,
    pub per_page: i64,
    pub address: Option<String>,
    pub tx_hash: Option<String>,
    pub from_chain: Option<ChainName>,
    pub to_chain: Option<ChainName>,
    pub from_owner: Option<String>,
    pub to_owner: Option<String>,

    #[serde(deserialize_with = "deserialize_csv_field")]
    pub status: Option<HashSet<OrderStatusVerbose>>,
}

#[derive(Debug, Serialize, Deserialize, Hash, PartialEq, Eq)]
pub enum OrderStatusVerbose {
    #[serde(rename = "not-initiated")]
    NotInitiated,
    #[serde(rename = "in-progress", alias = "pending")]
    InProgress,
    #[serde(rename = "completed", alias = "fulfilled")]
    Completed,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "refunded")]
    Refunded,
}
impl Default for OrderQueryFilters {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: 10,
            address: None,
            tx_hash: None,
            from_chain: None,
            to_chain: None,
            status: None,
            from_owner: None,
            to_owner: None,
        }
    }
}

impl FromStr for OrderStatusVerbose {
    type Err = OrderbookError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "not-initiated" => Ok(OrderStatusVerbose::NotInitiated),
            "in-progress" | "pending" => Ok(OrderStatusVerbose::InProgress),
            "completed" | "fulfilled" => Ok(OrderStatusVerbose::Completed),
            "expired" => Ok(OrderStatusVerbose::Expired),
            "refunded" => Ok(OrderStatusVerbose::Refunded),
            _ => Err(OrderbookError::InvalidParams(format!(
                "Invalid order status: {}",
                s
            ))),
        }
    }
}

impl OrderQueryFilters {
    pub fn new(
        page_no: i64,
        limit: i64,
        address: Option<String>,
        tx_hash: Option<String>,
        from_chain: Option<ChainName>,
        to_chain: Option<ChainName>,
        status: Option<HashSet<OrderStatusVerbose>>,
        from: Option<String>,
        to: Option<String>,
    ) -> Result<Self, OrderbookError> {
        if !(0..=PAGE_LIMIT).contains(&limit) {
            return Err(OrderbookError::InvalidParams(format!(
                "Invalid page limit. Must be between 0 and {}",
                PAGE_LIMIT
            )));
        }
        Ok(Self {
            page: page_no,
            per_page: limit,
            address,
            tx_hash,
            from_chain,
            to_chain,
            status,
            from_owner: from,
            to_owner: to,
        })
    }

    /// Returns the offset for pagination queries
    pub fn offset(&self) -> i64 {
        (self.page - 1) * self.per_page
    }

    /// Returns the current page number
    pub fn page(&self) -> i64 {
        self.page
    }

    /// Returns the number of items per page
    pub fn per_page(&self) -> i64 {
        self.per_page
    }
}

/// Represents a blockchain network name
///
/// Used to identify distinct blockchain networks in a type-safe manner.
/// The inner string is stored as provided without case normalization.
#[derive(Debug, Hash, Default, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct ChainName(String);

impl ChainName {
    /// Create a new ChainName from a string slice
    pub fn new(s: &str) -> Self {
        Self(s.to_string())
    }

    /// Convenience constructor for Ethereum local network
    pub fn ethereum_localnet() -> Self {
        Self::new("ethereum_localnet")
    }

    /// Convenience constructor for Arbitrum local network
    pub fn arbitrum_localnet() -> Self {
        Self::new("arbitrum_localnet")
    }
}

impl From<&SingleSwap> for ChainName {
    fn from(value: &SingleSwap) -> Self {
        Self::new(&value.chain)
    }
}

impl AsRef<str> for ChainName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ChainName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A wrapper for a string that may or may not contain a value
///
/// Unlike Option<String>, this uses an empty string to represent None
/// and is optimized for use with SQL databases via sqlx's transparent types.
#[derive(Deserialize, Serialize, Debug, Clone, sqlx::Type)]
#[sqlx(transparent)]
pub struct MaybeString(String);

impl MaybeString {
    /// Create a new MaybeString
    #[inline]
    pub fn new(s: String) -> Self {
        MaybeString(s)
    }

    /// Returns true if the string contains a non-empty value
    #[inline]
    pub fn is_some(&self) -> bool {
        !self.0.is_empty()
    }

    /// Returns true if the string is empty
    #[inline]
    pub fn is_none(&self) -> bool {
        self.0.is_empty()
    }

    /// Get the contained string value
    ///
    /// Returns a clone of the inner string.
    #[inline]
    pub fn to_string(&self) -> String {
        self.0.clone()
    }

    /// Get a reference to the inner string
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for MaybeString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// BitcoinTimeStampData is used to store the timestamp of the initiate, redeem, and refund transactions for a Bitcoin swap
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BitcoinTimestampData {
    /// Timestamp when the initiate transaction for a Bitcoin swap was detected
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initiate_detected_timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Timestamp when the redeem transaction for a Bitcoin swap was detected
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redeem_detected_timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Timestamp when the refund transaction for a Bitcoin swap was detected
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refund_detected_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

impl Default for BitcoinTimestampData {
    fn default() -> Self {
        Self {
            initiate_detected_timestamp: None,
            redeem_detected_timestamp: None,
            refund_detected_timestamp: None,
        }
    }
}

/// Additional data attached to an order
///
/// Contains metadata and auxiliary information needed for order processing,
/// including pricing data, signatures, and optional Bitcoin-specific information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdditionalData {
    /// Unique identifier for the trading strategy used
    pub strategy_id: String,

    /// Optional Bitcoin recipient address for cross-chain orders
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitcoin_optional_recipient: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_delegator: Option<String>,
    /// Price of the input token at time of order creation
    pub input_token_price: f64,

    /// Price of the output token at time of order creation
    pub output_token_price: f64,

    /// Cryptographic signature for order verification
    pub sig: String,

    /// Unix timestamp after which the order is no longer valid
    pub deadline: i64,

    /// Optional serialized transaction for instant refunds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instant_refund_tx_bytes: Option<String>,

    /// Optional serialized transaction for redeeming funds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redeem_tx_bytes: Option<String>,

    /// Optional transaction hash to track creation of the order
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_hash: Option<String>,

    /// Flag indicating if this order is blacklisted
    #[serde(default)]
    pub is_blacklisted: bool,

    /// Optional integrator for the order
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrator: Option<String>,

    #[serde(default)]
    pub version: HTLCVersion,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitcoin: Option<BitcoinTimestampData>,
}

impl TryFrom<AdditionalData> for Bytes {
    type Error = eyre::Report;

    fn try_from(value: AdditionalData) -> Result<Self, Self::Error> {
        Ok(Bytes::from(serde_json::to_string(&value)?.into_bytes()))
    }
}

impl From<AdditionalData> for SignableAdditionalData {
    fn from(value: AdditionalData) -> Self {
        SignableAdditionalData {
            strategy_id: value.strategy_id.clone(),
            bitcoin_optional_recipient: value.bitcoin_optional_recipient.clone(),
            input_token_price: value.input_token_price,
            output_token_price: value.output_token_price,
            deadline: value.deadline,
        }
    }
}

/// Subset of AdditionalData that is used for signing
///
/// Contains only the fields that need to be signed for order verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignableAdditionalData {
    /// Unique identifier for the trading strategy used
    pub strategy_id: String,

    /// Optional Bitcoin recipient address for cross-chain orders
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitcoin_optional_recipient: Option<String>,

    /// Price of the input token at time of order creation
    pub input_token_price: f64,

    /// Price of the output token at time of order creation
    pub output_token_price: f64,

    /// Unix timestamp after which the order is no longer valid
    pub deadline: i64,
}

/// Subset of AdditionalData that is sent as part of the order creation request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatableAdditionalData {
    /// Unique identifier for the trading strategy used
    pub strategy_id: String,

    /// Optional Bitcoin recipient address for cross-chain orders
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bitcoin_optional_recipient: Option<String>,

    /// Price of the input token at time of order creation
    pub input_token_price: Option<f64>,

    /// Price of the output token at time of order creation
    pub output_token_price: Option<f64>,

    /// Cryptographic signature for order verification
    pub sig: Option<String>,

    /// Unix timestamp after which the order is no longer valid
    pub deadline: Option<i64>,

    /// Slippage for the order in basis points
    pub slippage: Option<u64>,

    /// Source delegator for the order
    pub source_delegator: Option<String>,
}

impl TryFrom<CreatableAdditionalData> for AdditionalData {
    type Error = eyre::Report;

    fn try_from(value: CreatableAdditionalData) -> Result<Self, Self::Error> {
        Ok(AdditionalData {
            strategy_id: value.strategy_id.clone(),
            bitcoin_optional_recipient: value.bitcoin_optional_recipient.clone(),
            source_delegator: value.source_delegator.clone(),
            input_token_price: value
                .input_token_price
                .ok_or_else(|| eyre::eyre!("input_token_price is required"))?,
            output_token_price: value
                .output_token_price
                .ok_or_else(|| eyre::eyre!("output_token_price is required"))?,
            sig: value
                .sig
                .ok_or_else(|| eyre::eyre!("signature is required"))?,
            deadline: value
                .deadline
                .ok_or_else(|| eyre::eyre!("deadline is required"))?,
            instant_refund_tx_bytes: None,
            redeem_tx_bytes: None,
            tx_hash: None,
            is_blacklisted: false,
            integrator: None,
            bitcoin: None,
            version: HTLCVersion::V1,
        })
    }
}

// AffiliateFee is the extra fee that the affiliate wants to charge on top of the base fee
// This is in addition to the base fee that the user pays
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffiliateFee {
    /// The address of the affiliate
    pub address: String,

    /// The chain of the affiliate
    pub chain: String,

    /// The asset, the affiliate wants to charge the fee in
    pub asset: String,

    /// The amount of the fee in basis points
    pub fee: u64,

    /// The amount of the fee in the asset
    pub amount: BigDecimal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffiliateFeeV2 {
    /// <chain:asset_id> representation
    pub asset: String,
    /// The address of the affiliate
    pub address: String,
    /// The amount of the fee in basis points
    pub fee: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffiliateFeesV2(Vec<AffiliateFeeV2>);

impl AffiliateFeesV2 {
    pub fn new(fees: Vec<AffiliateFeeV2>) -> Self {
        Self(fees)
    }

    pub fn inner(&self) -> Vec<AffiliateFeeV2> {
        self.0.clone()
    }

    pub fn as_ref(&self) -> &[AffiliateFeeV2] {
        &self.0
    }
}

impl Default for AffiliateFeesV2 {
    fn default() -> Self {
        Self(vec![])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffiliateFees(Vec<AffiliateFee>);

impl AffiliateFees {
    pub fn new(fees: Vec<AffiliateFee>) -> Self {
        Self(fees)
    }
}

impl TryFrom<AffiliateFeeV2> for Bytes {
    type Error = eyre::Report;

    fn try_from(value: AffiliateFeeV2) -> Result<Self, Self::Error> {
        Ok(Bytes::from(serde_json::to_string(&value)?.into_bytes()))
    }
}

impl TryFrom<AffiliateFeesV2> for Bytes {
    type Error = eyre::Report;

    fn try_from(value: AffiliateFeesV2) -> Result<Self, Self::Error> {
        Ok(Bytes::from(serde_json::to_string(&value)?.into_bytes()))
    }
}

impl TryFrom<AffiliateFee> for Bytes {
    type Error = eyre::Report;

    fn try_from(value: AffiliateFee) -> Result<Self, Self::Error> {
        Ok(Bytes::from(serde_json::to_string(&value)?.into_bytes()))
    }
}

impl TryFrom<AffiliateFees> for Bytes {
    type Error = eyre::Report;

    fn try_from(value: AffiliateFees) -> Result<Self, Self::Error> {
        Ok(Bytes::from(serde_json::to_string(&value)?.into_bytes()))
    }
}

impl Default for AffiliateFees {
    fn default() -> Self {
        Self(vec![])
    }
}

/// Represents a claim for affiliate fees
///
/// This struct maps to the affiliate_fees table and represents
/// a claim that an integrator can make for their earned fees.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Claim {
    /// Maps to create_order.additional_data -> 'integrator'
    pub integrator_name: String,

    /// EVM address of the integrator
    pub address: String,

    /// Chain name (e.g., base, arbitrum, ethereum)
    pub chain: String,

    /// Token address
    pub token_address: String,

    /// Total earnings for this claim
    pub total_earnings: BigDecimal,

    /// Signature that will yield the integrator the above amount
    pub claim_signature: String,

    /// The contract they have to use for claiming
    pub claim_contract: String,
}

/// EVM transaction
///
/// This struct contains the necessary parameters for an EVM transaction.
/// It includes the to address, the value, the data, the gas limit, and the chain ID.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EVMTransaction {
    /// The address of the contract to send the transaction to
    pub to: Address,

    /// The value of the transaction
    pub value: U256,

    /// The data of the transaction
    pub data: Bytes,

    /// The gas limit of the transaction
    pub gas_limit: U256,

    /// The chain ID of the transaction
    pub chain_id: u64,
}

/// Rust friendly Order struct to make create order requests
///
/// This is a generic struct that can be used with different additional data types.
/// By default, it uses the standard AdditionalData type.
///
/// This struct is converted into a CreateOrder struct (solidity struct)
/// and sent to the contract.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Order<T = AdditionalData> {
    /// Source blockchain network
    pub source_chain: String,

    /// Destination blockchain network
    pub destination_chain: String,

    /// Token address/identifier on the source chain
    pub source_asset: String,

    /// Token address/identifier on the destination chain
    pub destination_asset: String,

    /// Address of the initiator on the source chain
    pub initiator_source_address: Option<String>,

    /// Address of the initiator on the destination chain
    pub initiator_destination_address: Option<String>,

    /// Amount of tokens to swap from the source chain
    pub source_amount: BigDecimal,

    /// Amount of tokens to receive on the destination chain
    pub destination_amount: BigDecimal,

    /// Fee amount for the swap
    pub fee: Option<BigDecimal>,

    /// Optional identifier for the user who created the order
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// Unique nonce for the order
    pub nonce: Option<BigDecimal>,

    /// Minimum number of confirmations required on destination chain
    pub min_destination_confirmations: Option<u64>,

    /// Time lock for the atomic swap (in blocks)
    pub timelock: Option<u64>,

    /// Hash of the secret for the atomic swap (HTLC)
    pub secret_hash: Option<String>,

    /// Affiliate fees for the order
    #[serde(default)]
    pub affiliate_fees: AffiliateFees,

    /// Additional data for the order
    pub additional_data: T,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderV2 {
    pub source: AssetV2,
    pub destination: AssetV2,
    pub slippage: Option<u64>,
    pub secret_hash: Option<String>,
    pub nonce: Option<BigDecimal>,
    /// Affiliate fees for the order
    #[serde(default)]
    pub affiliate_fees: AffiliateFeesV2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetV2 {
    pub asset: String,
    pub owner: Option<String>,
    pub delegate: Option<String>,
    pub amount: BigDecimal,
}

impl From<Order<AdditionalData>> for Order<SignableAdditionalData> {
    fn from(order: Order<AdditionalData>) -> Self {
        Order {
            source_chain: order.source_chain.clone(),
            destination_chain: order.destination_chain.clone(),
            source_asset: order.source_asset.clone(),
            destination_asset: order.destination_asset.clone(),
            initiator_source_address: order.initiator_source_address.clone(),
            initiator_destination_address: order.initiator_destination_address.clone(),
            user_id: None,
            source_amount: order.source_amount.clone(),
            destination_amount: order.destination_amount.clone(),
            fee: order.fee.clone(),
            nonce: order.nonce.clone(),
            min_destination_confirmations: order.min_destination_confirmations,
            timelock: order.timelock,
            secret_hash: order.secret_hash.clone(),
            affiliate_fees: order.affiliate_fees.clone(),
            additional_data: order.additional_data.into(),
        }
    }
}

impl TryFrom<Order<CreatableAdditionalData>> for Order<AdditionalData> {
    type Error = eyre::Report;

    fn try_from(order: Order<CreatableAdditionalData>) -> Result<Self, Self::Error> {
        Ok(Order {
            source_chain: order.source_chain.clone(),
            destination_chain: order.destination_chain.clone(),
            source_asset: order.source_asset.clone(),
            destination_asset: order.destination_asset.clone(),
            initiator_source_address: order.initiator_source_address.clone(),
            initiator_destination_address: order.initiator_destination_address.clone(),
            user_id: order.user_id,
            source_amount: order.source_amount.clone(),
            destination_amount: order.destination_amount.clone(),
            fee: order.fee.clone(),
            nonce: order.nonce.clone(),
            min_destination_confirmations: order.min_destination_confirmations,
            timelock: order.timelock,
            secret_hash: order.secret_hash.clone(),
            affiliate_fees: order.affiliate_fees.clone(),
            additional_data: AdditionalData::try_from(order.additional_data)?,
        })
    }
}

/// CreateOrder represents the initial order that is created by the user
///
/// This struct is used to store the order in the database and includes
/// creation timestamps and blockchain-specific data.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct CreateOrder {
    /// Timestamp when the order was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp of the last update to the order
    pub updated_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the order was deleted (if applicable)
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Unique identifier for the create order
    pub create_id: String,

    /// Block number where the order was created
    pub block_number: BigDecimal,

    /// Source blockchain network
    pub source_chain: String,

    /// Destination blockchain network
    pub destination_chain: String,

    /// Token address/identifier on the source chain
    pub source_asset: String,

    /// Token address/identifier on the destination chain
    pub destination_asset: String,

    /// Address of the initiator on the source chain
    pub initiator_source_address: String,

    /// Address of the initiator on the destination chain
    pub initiator_destination_address: String,

    /// Amount of tokens to swap from the source chain
    pub source_amount: BigDecimal,

    /// Amount of tokens to receive on the destination chain
    pub destination_amount: BigDecimal,

    /// Fee amount for the swap
    pub fee: BigDecimal,

    /// Unique nonce for the order
    pub nonce: BigDecimal,

    /// Minimum number of confirmations required on destination chain
    pub min_destination_confirmations: i32,

    /// Time lock for the atomic swap (in blocks)
    pub timelock: i32,

    /// Hash of the secret for the atomic swap (HTLC)
    pub secret_hash: String,

    /// Optional identifier for the user who created the order
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,

    /// Affiliate fees for the order
    #[sqlx(json)]
    pub affiliate_fees: Option<Vec<AffiliateFee>>,

    /// Additional data for the order
    #[sqlx(json)]
    pub additional_data: AdditionalData,
}

/// Represents a fill order in the orderbook
///
/// This struct is used when an order is matched and filled.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct FillOrder {
    /// Timestamp when the fill was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp of the last update to the fill
    pub updated_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the fill was deleted (if applicable)
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Unique identifier for the fill order
    pub fill_id: String,

    /// Block timestamp when the fill occurred
    pub block_timestamp: BigDecimal,

    /// Source blockchain network
    pub source_chain: String,

    /// Destination blockchain network
    pub destination_chain: String,

    /// Token address/identifier on the source chain
    pub source_asset: String,

    /// Token address/identifier on the destination chain
    pub destination_asset: String,

    /// Address of the redeemer on the source chain
    pub redeemer_source_address: String,

    /// Address of the redeemer on the destination chain
    pub redeemer_destination_address: String,

    /// Amount of tokens swapped from the source chain
    pub source_amount: BigDecimal,

    /// Amount of tokens received on the destination chain
    pub destination_amount: BigDecimal,

    /// Fee amount for the swap
    pub fee: BigDecimal,

    /// Unique nonce for the order
    pub nonce: BigDecimal,

    /// Minimum number of confirmations required on source chain
    pub min_source_confirmations: i64,

    /// Time lock for the atomic swap (in blocks)
    pub timelock: i32,
}

/// Represents a lowercase string
///
/// This type ensures that a string is always stored in lowercase form,
/// which is useful for case-insensitive comparisons.
#[derive(Deserialize, Hash, Default, Eq, PartialEq, Serialize, Debug, Clone, sqlx::Type)]
#[sqlx(transparent)]
pub struct LCString(String);

impl From<String> for LCString {
    fn from(value: String) -> Self {
        Self(value.to_lowercase())
    }
}

impl From<&str> for LCString {
    fn from(value: &str) -> Self {
        Self::from(value.to_string())
    }
}

impl AsRef<str> for LCString {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LCString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl LCString {
    /// Get the lowercase string value
    #[inline]
    pub fn value(&self) -> String {
        self.0.clone()
    }

    /// Get a reference to the inner string
    #[inline]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Represents a single atomic swap
///
/// This struct tracks the state of an individual atomic swap (HTLC)
/// on a single blockchain.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SingleSwap {
    /// Timestamp when the swap was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp of the last update to the swap
    pub updated_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the swap was deleted (if applicable)
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Unique identifier for the swap
    pub swap_id: String,

    /// Blockchain network for this swap
    pub chain: String,

    /// Token address/identifier for this swap
    pub asset: String,

    /// HTLC address for this swap
    pub htlc_address: Option<String>,

    /// token address for this swap
    pub token_address: Option<String>,

    /// Address of the initiator
    pub initiator: String,

    /// Address of the redeemer
    pub redeemer: String,

    /// Time lock for the atomic swap (in blocks)
    pub timelock: i32,

    /// Amount of tokens that have been filled
    pub filled_amount: BigDecimal,

    /// Total amount of tokens in the swap
    pub amount: BigDecimal,

    /// Hash of the secret for the atomic swap (HTLC)
    pub secret_hash: String,

    /// The revealed secret (when available)
    pub secret: MaybeString,

    /// Transaction hash for the initiate transaction
    pub initiate_tx_hash: MaybeString,

    /// Transaction hash for the redeem transaction
    pub redeem_tx_hash: MaybeString,

    /// Transaction hash for the refund transaction
    pub refund_tx_hash: MaybeString,

    /// Block number of the initiate transaction
    pub initiate_block_number: Option<BigDecimal>,

    /// Block number of the redeem transaction
    pub redeem_block_number: Option<BigDecimal>,

    /// Block number of the refund transaction
    pub refund_block_number: Option<BigDecimal>,

    /// Number of confirmations required for this swap
    pub required_confirmations: i32,

    /// Current number of confirmations for this swap
    pub current_confirmations: i32,

    /// Timestamp of the initiate transaction
    pub initiate_timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Timestamp of the redeem transaction
    pub redeem_timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Timestamp of the refund transaction
    pub refund_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

impl SingleSwap {
    /// Convert to an EVM-compatible swap structure
    ///
    /// Transforms this swap into the format expected by EVM contracts.
    pub fn get_evm_swap(&self) -> Result<EVMSwap> {
        // Handle scientific notation in the amount string
        let amt_str = if self.amount.to_string().contains('e') {
            let amt_str = self.amount.to_string();
            if let Some((mantissa, exponent)) = amt_str.split_once('e') {
                let exponent_value: i32 = exponent.parse()?;
                format!("{}{}", mantissa, "0".repeat(exponent_value as usize))
            } else {
                amt_str
            }
        } else {
            self.amount.to_string()
        };

        Ok(EVMSwap {
            initiator: Address::from_hex(&self.initiator)?,
            redeemer: Address::from_hex(&self.redeemer)?,
            secret_hash: FixedBytes::from_hex(&self.secret_hash)?,
            timelock: U256::from_str(&self.timelock.to_string())?,
            amount: U256::from_str(&amt_str)?,
            order_id: FixedBytes::from_hex(&self.swap_id)?,
            destination_data: None,
        })
    }

    /// Validates that there is only one initiate transaction
    ///
    /// # Returns
    /// A boolean value representing, whether there are multiple inits or not.
    pub fn has_multiple_inits(&self) -> bool {
        let compound_tx_hash = self.initiate_tx_hash.as_str();
        let tx_hashes = compound_tx_hash.split(',').collect::<Vec<&str>>();

        tx_hashes.len() > 1
    }

    /// Extracts the transaction hash from a formatted string that may include block numbers.
    ///
    /// Handles formats like:
    /// - `txhash:blocknumber,txhash2:blocknumber2`
    /// - Prioritizes entries with block numbers > 0
    /// - Falls back to the last entry if no valid block numbers are found
    ///
    /// # Returns
    /// The extracted transaction hash without any block number suffix
    pub fn get_init_tx_hash(&self) -> Result<String> {
        // First try to find any hash with a block number > 0
        let compound_tx_hash = self.initiate_tx_hash.as_str();
        let selected_hash = compound_tx_hash
            .split(',')
            .find(|hash| {
                let parts: Vec<&str> = hash.split(':').collect();
                parts.len() == 2 && parts[1].parse::<u64>().is_ok_and(|num| num > 0)
            })
            .or_else(|| compound_tx_hash.split(',').last())
            .ok_or_else(|| eyre::eyre!("Invalid transaction hash format: {}", compound_tx_hash))?;

        // Extract just the hash part without the block number
        let tx_hash = selected_hash
            .split(':')
            .next()
            .ok_or_else(|| eyre::eyre!("Invalid transaction hash format: {}", compound_tx_hash))?
            .to_string();

        Ok(tx_hash)
    }

    /// Check if this swap is on a Bitcoin network
    #[inline]
    pub fn is_bitcoin(&self) -> bool {
        matches!(
            self.chain.to_lowercase().as_str(),
            BITCOIN_MAINNET | BITCOIN_REGTEST | BITCOIN_TESTNET
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct SingleSwapV2 {
    /// Timestamp when the swap was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Unique identifier for the swap
    pub swap_id: String,

    /// Blockchain network for this swap
    pub chain: String,

    /// Token address/identifier for this swap
    pub asset: String,

    /// Address of the initiator
    pub initiator: String,

    /// Address of the redeemer
    pub redeemer: String,

    /// Delegate
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegate: Option<String>,

    /// Time lock for the atomic swap (in blocks)
    pub timelock: i32,

    /// Amount of tokens that have been filled
    pub filled_amount: BigDecimal,

    /// Asset price at the time of creation
    pub asset_price: f64,

    /// Total amount of tokens in the swap
    pub amount: BigDecimal,

    /// Hash of the secret for the atomic swap (HTLC)
    pub secret_hash: String,

    /// The revealed secret (when available)
    pub secret: MaybeString,

    /// Instant refund tx bytes. Only for source swap
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instant_refund_tx: Option<String>,

    /// Transaction hash for the initiate transaction
    pub initiate_tx_hash: MaybeString,

    /// Transaction hash for the redeem transaction
    pub redeem_tx_hash: MaybeString,

    /// Transaction hash for the refund transaction
    pub refund_tx_hash: MaybeString,

    /// Block number of the initiate transaction
    pub initiate_block_number: Option<BigDecimal>,

    /// Block number of the redeem transaction
    pub redeem_block_number: Option<BigDecimal>,

    /// Block number of the refund transaction
    pub refund_block_number: Option<BigDecimal>,

    /// Number of confirmations required for this swap
    pub required_confirmations: i32,

    /// Current number of confirmations for this swap
    pub current_confirmations: i32,

    /// Timestamp of the initiate transaction
    pub initiate_timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Timestamp of the redeem transaction
    pub redeem_timestamp: Option<chrono::DateTime<chrono::Utc>>,

    /// Timestamp of the refund transaction
    pub refund_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Represents a matched order in the database
///
/// Links a create order with its corresponding source and destination swaps.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MatchedOrder {
    /// Timestamp when the match was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp of the last update to the match
    pub updated_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the match was deleted (if applicable)
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,

    /// ID of the create order
    pub create_order_id: i32,

    /// ID of the source swap
    pub source_swap_id: i32,

    /// ID of the destination swap
    pub destination_swap_id: i32,
}

/// Verbose representation of a matched order
///
/// Includes the full details of the create order and both swaps.
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MatchedOrderVerbose {
    /// Timestamp when the match was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp of the last update to the match
    pub updated_at: chrono::DateTime<chrono::Utc>,

    /// Timestamp when the match was deleted (if applicable)
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Full details of the source swap
    #[sqlx(json)]
    pub source_swap: SingleSwap,

    /// Full details of the destination swap
    #[sqlx(json)]
    pub destination_swap: SingleSwap,

    /// Full details of the create order
    #[sqlx(flatten)]
    pub create_order: CreateOrder,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MatchedOrderVerboseV2 {
    /// Timestamp when the match was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Full details of the source swap
    #[sqlx(json)]
    pub source_swap: SingleSwapV2,

    /// Full details of the destination swap
    #[sqlx(json)]
    pub destination_swap: SingleSwapV2,

    /// Nonce for the order. Random if not provided by the user
    pub nonce: BigDecimal,

    /// Unique ID of the order
    pub order_id: String,

    /// Affiliate fees for the order
    #[sqlx(json)]
    pub affiliate_fees: Option<Vec<AffiliateFeeV2>>,

    /// Optional integrator for the order
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrator: Option<String>,

    /// Version of the HTLC Contract being used
    pub version: HTLCVersion,
}

impl MatchedOrderVerbose {
    /// Get a string representation of the order pair
    ///
    /// Returns a formatted string in the format:
    /// "source_chain:source_asset::destination_chain:destination_asset"
    pub fn get_order_pair(&self) -> String {
        format!(
            "{}:{}::{}:{}",
            self.source_swap.chain,
            self.source_swap.asset,
            self.destination_swap.chain,
            self.destination_swap.asset
        )
    }

    /// Gets the optional Bitcoin recipient address string from the create order's additional data.
    /// Returns an error if the bitcoin_optional_recipient field is not set.
    ///
    /// # Errors
    /// Returns `eyre::Error` if bitcoin_optional_recipient is None
    pub fn get_bitcoin_recipient_address(&self) -> Result<String> {
        let recipient_address_str = self
            .create_order
            .additional_data
            .bitcoin_optional_recipient
            .clone()
            .ok_or_else(|| eyre::eyre!("bitcoin_optional_recipient is not set"))?;

        Ok(recipient_address_str)
    }
}

/// Represents an EVM-compatible atomic swap
///
/// This struct is used for interacting with EVM smart contracts.
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EVMSwap {
    /// Address of the initiator
    pub initiator: Address,

    /// Address of the redeemer
    pub redeemer: Address,

    /// Hash of the secret for the atomic swap (HTLC)
    pub secret_hash: FixedBytes<32>,

    /// Time lock for the atomic swap
    pub timelock: U256,

    /// Amount of tokens in the swap
    pub amount: U256,

    /// Swap id of the swap
    pub order_id: FixedBytes<32>,

    /// Destination data for the swap
    pub destination_data: Option<Bytes>,
}

impl EVMSwap {
    pub fn new(
        initiator: Address,
        redeemer: Address,
        secret_hash: FixedBytes<32>,
        timelock: U256,
        amount: U256,
        order_id: FixedBytes<32>,
        destination_data: Option<Bytes>,
    ) -> Result<Self> {
        let swap = Self {
            initiator,
            redeemer,
            secret_hash,
            timelock,
            amount,
            order_id,
            destination_data,
        };

        swap.validate()?;
        Ok(swap)
    }

    pub fn validate(&self) -> Result<()> {
        if self.amount.is_zero() {
            return Err(eyre::eyre!("Amount cannot be zero"));
        }

        if self.initiator == Address::ZERO {
            return Err(eyre::eyre!("Initiator cannot be zero address"));
        }

        if self.redeemer == Address::ZERO {
            return Err(eyre::eyre!("Redeemer cannot be zero address"));
        }

        if self.timelock.is_zero() {
            return Err(eyre::eyre!("Timelock cannot be zero"));
        }

        Ok(())
    }
}

/// Generic paginated data container
///
/// Used for API responses that return paginated lists of items.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaginatedData<T> {
    /// Collection of items for the current page
    pub data: Vec<T>,

    /// Current page number
    pub page: i64,

    /// Total number of pages
    pub total_pages: i64,

    /// Total number of items across all pages
    pub total_items: i64,

    /// Number of items per page
    pub per_page: i64,
}

impl<T> PaginatedData<T> {
    /// Create a new paginated data container
    ///
    /// Automatically calculates the total number of pages based on the
    /// total items and items per page.
    pub fn new(data: Vec<T>, page: i64, total_items: i64, per_page: i64) -> Self {
        // Calculate total pages, rounding up when there's a remainder
        let total_pages = (total_items + per_page - 1) / per_page;

        Self {
            data,
            page,
            total_items,
            per_page,
            total_pages,
        }
    }
}

/// Query parameters for actions.
///
/// This struct is intended to be used at the API level for parsing and serializing
/// action-related query parameters.
#[derive(Deserialize, Serialize)]
pub struct HTLCActionQueryParam {
    pub action: ActionType,
}

/// ActionType enum without any bindings to action, used to specify additional checks
#[derive(Debug, Clone, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActionType {
    // Represents initiate action
    Initiate,
    // Represents initiate with signature action
    InitiateWithSignature,
    // Represents redeem action
    Redeem,
    // Represents refund action
    Refund,
    // Represents instant refund action
    InstantRefund,
}

/// Display implementation for ActionType
impl Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ActionType::Initiate => write!(f, "initiate"),
            ActionType::InitiateWithSignature => write!(f, "initiate-with-signature"),
            ActionType::Redeem => write!(f, "redeem"),
            ActionType::Refund => write!(f, "refund"),
            ActionType::InstantRefund => write!(f, "instant-refund"),
        }
    }
}

impl ActionType {
    /// Returns the chain on which the action is to be performed.
    pub fn on_chain(&self) -> SwapChain {
        match self {
            ActionType::Initiate
            | ActionType::InitiateWithSignature
            | ActionType::InstantRefund
            | ActionType::Refund => SwapChain::Source,
            ActionType::Redeem => SwapChain::Destination,
        }
    }
}

/// Represents an action along with swap info
///
/// This struct is used to represent an action with swap info
#[derive(Debug, Clone)]
pub struct ActionWithInfo {
    /// The action to perform
    pub action: HTLCAction,
    /// The swap info
    ///
    /// This is optional as NoOp action does not require swap info
    pub swap: Option<SingleSwap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TimeInterval {
    #[serde(rename = "second")]
    Second,
    #[serde(rename = "minute")]
    Minute,
    #[serde(rename = "hour")]
    Hour,
    #[serde(rename = "day")]
    Day,
}

/// Filters and parameters for querying order book statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsQueryFilters {
    /// Optional chain identifier to filter statistics by specific blockchain
    pub source_chain: Option<String>,

    /// Optional chain identifier to filter statistics by specific blockchain
    pub destination_chain: Option<String>,

    /// Optional address to filter statistics by specific address
    pub address: Option<String>,

    /// Optional Unix timestamp marking the start time for the query window
    pub from: Option<i64>, // Unix timestamp

    /// Optional Unix timestamp marking the end time for the query window
    pub to: Option<i64>, // Unix timestamp
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestStruct {
        #[serde(default)]
        version: HTLCVersion,
    }

    #[test]
    fn test_htlc_version_json_serialization() {
        // Test V1 serialization
        let v1 = HTLCVersion::V1;
        let json = serde_json::to_string(&v1).unwrap();
        assert_eq!(json, r#""v1""#);

        // Test V2 serialization
        let v2 = HTLCVersion::V2;
        let json = serde_json::to_string(&v2).unwrap();
        assert_eq!(json, r#""v2""#);

        // Test in a struct
        let test = TestStruct {
            version: HTLCVersion::V1,
        };
        let json = serde_json::to_string(&test).unwrap();
        assert_eq!(json, r#"{"version":"v1"}"#);
    }

    #[test]
    fn test_htlc_version_json_deserialization() {
        // Test V1 deserialization (case insensitive)
        let v1: HTLCVersion = serde_json::from_str(r#""v1""#).unwrap();
        assert_eq!(v1, HTLCVersion::V1);

        // Test V2 deserialization (case insensitive)
        let v2: HTLCVersion = serde_json::from_str(r#""v2""#).unwrap();
        assert_eq!(v2, HTLCVersion::V2);

        // Test in a struct
        let test: TestStruct = serde_json::from_str(r#"{"version":"v1"}"#).unwrap();
        assert_eq!(test.version, HTLCVersion::V1);

        // Test missing field in struct (should use default V1)
        let test: TestStruct = serde_json::from_str("{}").unwrap();
        assert_eq!(test.version, HTLCVersion::V1);
    }

    #[test]
    fn test_htlc_version_from_str() {
        assert_eq!(HTLCVersion::from_str("v1"), Some(HTLCVersion::V1));
        assert_eq!(HTLCVersion::from_str("v2"), Some(HTLCVersion::V2));
        assert_eq!(HTLCVersion::from_str("v3"), Some(HTLCVersion::V3));
        assert_eq!(HTLCVersion::from_str(""), None);
    }

    #[test]
    fn test_htlc_version_as_str() {
        assert_eq!(HTLCVersion::V1.as_str(), "v1");
        assert_eq!(HTLCVersion::V2.as_str(), "v2");
    }

    #[test]
    fn test_htlc_version_default() {
        assert_eq!(HTLCVersion::default(), HTLCVersion::V1);
    }
}
