/// Represents the type of swap event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SwapEventType {
    /// Swap initiation event.
    Initiate,
    /// Swap redeem event, contains the order secret.
    Redeem(OrderSecret),
    /// Swap refund event.
    Refund,
}

/// Wrapper for a swap order secret, expected to be a 64-character hex string.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OrderSecret(String);
impl OrderSecret {
    /// Creates a new OrderSecret from a string.
    pub fn new(secret: String) -> Result<Self, eyre::Report> {
        Self::try_from(secret)
    }

    #[cfg(test)]
    /// Returns the inner string representation of the order secret.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for OrderSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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

/// Represents transaction information for a swap event.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TxInfo {
    /// Transaction hash associated with the event.
    pub tx_hash: String,
    /// Block number in which the event occurred.
    pub block_number: i64,
    /// Block timestamp in which the event occurred.
    pub block_timestamp: Option<chrono::DateTime<chrono::Utc>>,
    /// Detected timestamp in which the event was detected.
    pub detected_timestamp: Option<chrono::DateTime<chrono::Utc>>,
}

/// Represents a swap event with all relevant details for updates in swap store.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SwapEvent {
    /// The type of the swap event.
    pub event_type: SwapEventType,
    /// Unique identifier for the swap.
    pub swap_id: String,
    /// Amount of the swap.
    pub amount: i64,
    /// Transaction information.
    pub tx_info: TxInfo,
    /// Is the swap blacklisted
    pub is_blacklisted: bool,
}
