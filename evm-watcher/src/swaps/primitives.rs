use sqlx::types::{BigDecimal, chrono};

/// Represents the type of swap event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum SwapEventType {
    /// Swap initiation event.
    Initiate,
    /// Swap redeem event, contains the order secret.
    Redeem(OrderSecret),
    /// Swap refund event.
    Refund,
}

/// Wrapper for a swap order secret, expected to be a 64-character hex string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrderSecret(String);
impl OrderSecret {
    #[allow(dead_code)]
    /// Creates a new OrderSecret from a string.
    pub fn new(secret: String) -> Result<Self, eyre::Report> {
        Self::try_from(secret)
    }
    /// Returns the inner string representation of the order secret.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Returns the inner string representation of the order secret.
    #[allow(dead_code)]
    pub fn to_string(&self) -> String {
        self.0.clone()
    }
}

impl TryFrom<String> for OrderSecret {
    type Error = eyre::Report;

    /// Tries to create an OrderSecret from a string.
    /// The string must be a 64-character hex (optionally prefixed with "0x").
    fn try_from(secret: String) -> Result<Self, Self::Error> {
        let s = secret.strip_prefix("0x").unwrap_or(&secret);
        if s.len() != 64 {
            eyre::bail!("Invalid secret length, expected 64, got {}", s.len());
        }
        Ok(Self(s.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OrderSwapId(String);
impl OrderSwapId {
    /// Returns the inner string representation of the order swap ID.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Returns the inner string representation of the order swap ID.
    #[allow(dead_code)]
    pub fn to_string(&self) -> String {
        self.0.clone()
    }
}
impl From<String> for OrderSwapId {
    /// Tries to create an OrderSwapId from a string.
    fn from(swap_id: String) -> Self {
        let s = swap_id.strip_prefix("0x").unwrap_or(&swap_id);
        Self(s.to_string())
    }
}

/// Represents an order in a swap event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct HTLCOrder {
    /// Redeemer
    pub redeemer: String,
    /// Timelock
    pub timelock: BigDecimal,
    /// Amount involved in the swap.
    pub amount: BigDecimal,
    /// asset address
    pub asset_address: String,
    /// chain
    pub chain: String,
}
impl HTLCOrder {
    /// Creates a new HTLCOrder.
    pub fn new(
        redeemer: String,
        timelock: BigDecimal,
        amount: BigDecimal,
        asset_address: String,
        chain: String,
    ) -> Self {
        Self {
            redeemer,
            timelock,
            amount,
            asset_address,
            chain,
        }
    }
}

/// Represents transaction information for a swap event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct EventTxInfo {
    /// Transaction hash associated with the event.
    pub tx_hash: String,
    /// Block number in which the event occurred.
    pub block_number: BigDecimal,
    /// Timestamp of the event.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}
impl EventTxInfo {
    /// Creates a new EventTxInfo.
    pub fn new(
        tx_hash: String,
        block_number: BigDecimal,
        time_stamp: chrono::DateTime<chrono::Utc>,
    ) -> Self {
        Self {
            tx_hash,
            block_number,
            timestamp: time_stamp,
        }
    }
}
/// Represents a swap event with all relevant details for updates in swap store
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SwapEvent {
    /// The type of the swap event.
    pub event_type: SwapEventType,
    /// Unique identifier for the swap.
    pub swap_id: OrderSwapId,
    /// Transaction information.
    pub tx_info: EventTxInfo,
    /// Order information.
    pub order: HTLCOrder,
}

impl SwapEvent {
    /// Creates a new SwapEvent.
    pub fn new(
        event_type: SwapEventType,
        swap_id: OrderSwapId,
        tx_info: EventTxInfo,
        order: HTLCOrder,
    ) -> Self {
        Self {
            event_type,
            swap_id,
            tx_info,
            order,
        }
    }
}
