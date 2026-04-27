use alloy::hex;
use bitcoin::XOnlyPublicKey;
use eyre::{eyre, Result};
use orderbook::primitives::SingleSwap;
use serde::{Deserialize, Serialize};
use std::convert::TryFrom;
use std::str::FromStr;

/// Represents the different spending paths in the HTLC Taproot tree
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HTLCLeaf {
    /// Requires secret preimage and redeemer signature
    Redeem,
    /// Allows initiator to claim funds after timelock expires
    Refund,
    /// Requires both parties signatures
    InstantRefund,
}

/// A structure containing all necessary information for a Bitcoin HTLC swap.
/// This includes public keys, amount, recipient address, and other parameters
/// required to create and manage the HTLC.
#[derive(Clone, Debug)]
pub struct HTLCParams {
    /// Public key of the party initiating the swap
    pub initiator_pubkey: XOnlyPublicKey,
    /// Public key of the party redeeming the swap
    pub redeemer_pubkey: XOnlyPublicKey,
    /// Amount in satoshis
    pub amount: u64,
    /// SHA256 hash of the secret (32 bytes)
    pub secret_hash: [u8; 32],
    /// Number of blocks before refund path becomes valid
    pub timelock: u64,
}

/// Implementation of TryFrom for converting SingleSwap to HTLCParams
impl TryFrom<&SingleSwap> for HTLCParams {
    type Error = eyre::Report;

    /// Converts a SingleSwap reference into HTLCParams data format.
    /// It returns a Result containing the HTLCParams instance if successful,
    /// or an error if the conversion fails.
    fn try_from(swap: &SingleSwap) -> Result<Self, Self::Error> {
        let initiator_pubkey = XOnlyPublicKey::from_str(&swap.initiator)
            .map_err(|e| eyre!("Failed to parse initiator public key: {}", e))?;
        let redeemer_pubkey = XOnlyPublicKey::from_str(&swap.redeemer)
            .map_err(|e| eyre!("Failed to parse redeemer public key: {}", e))?;
        let timelock = swap.timelock as u64;

        // Use hex crate from the crates.io dependency
        let secret_hash = hex::decode(&swap.secret_hash)
            .map_err(|e| eyre!("Failed to decode secret hash: {}", e))?;
        let secret_hash: [u8; 32] = secret_hash
            .try_into()
            .map_err(|_| eyre!("Secret hash has incorrect length"))?;

        // Parse amount from string representation
        let amount = swap
            .amount
            .to_string()
            .parse::<u64>()
            .map_err(|_| eyre!("Failed to parse amount"))?;

        Ok(HTLCParams {
            initiator_pubkey,
            redeemer_pubkey,
            amount,
            secret_hash,
            timelock,
        })
    }
}

impl TryFrom<SingleSwap> for HTLCParams {
    type Error = eyre::Report;

    fn try_from(swap: SingleSwap) -> Result<Self, Self::Error> {
        Self::try_from(&swap)
    }
}

/// Represents a pair of signatures from both parties for an Instant refund transaction
#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InstantRefundSignatures {
    /// Signature from the initiator
    pub initiator: String,
    /// Signature from the redeemer
    pub redeemer: String,
}
