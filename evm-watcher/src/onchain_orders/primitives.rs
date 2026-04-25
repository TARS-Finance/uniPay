use alloy::primitives::{Address, FixedBytes, U256};
use alloy_sol_types::SolValue;
use tars::{orderbook::primitives::SingleSwap, utils::ToBytes};
use sqlx::types::BigDecimal;
use std::{ops::Deref, str::FromStr};

/// Represents a request to retrieve an on-chain order
#[derive(Debug, Clone)]
pub struct OnchainRequest {
    pub swap_id: String,
    pub contract: Address,
}

impl OnchainRequest {
    /// Parses the swap ID and returns it as a FixedBytes<32>
    pub fn parse_swap_id(&self) -> eyre::Result<FixedBytes<32>> {
        self.swap_id.hex_to_fixed_bytes().map_err(|e| {
            eyre::eyre!(
                "Failed to convert swap_id '{}' to bytes: {}",
                self.swap_id,
                e
            )
        })
    }
}

/// Converts a SingleSwap to an OnchainRequest
impl TryFrom<&SingleSwap> for OnchainRequest {
    type Error = eyre::Error;

    fn try_from(swap: &SingleSwap) -> Result<Self, Self::Error> {
        let contract = Address::from_str(&swap.asset)
            .map_err(|_| eyre::eyre!("Invalid contract address: {}", swap.asset))?;
        Ok(OnchainRequest {
            swap_id: swap.swap_id.deref().to_string(),
            contract,
        })
    }
}

/// Represents an on-chain order
#[derive(Debug, Clone, PartialEq)]
pub struct OnChainOrder {
    pub initiator: String,
    pub redeemer: String,
    pub initiated_at: BigDecimal,
    pub timelock: BigDecimal,
    pub amount: BigDecimal,
    pub fulfilled_at: BigDecimal,
}
impl OnChainOrder {
    pub fn is_empty(&self) -> bool {
        self.amount == BigDecimal::from(0)
            && self.initiator == Address::ZERO.to_string()
            && self.redeemer == Address::ZERO.to_string()
            && self.timelock == BigDecimal::from(0)
    }
}

impl TryFrom<&[u8]> for OnChainOrder {
    fn try_from(data: &[u8]) -> eyre::Result<Self> {
        decode_order(data)
    }

    type Error = eyre::Error;
}

fn decode_order(data: &[u8]) -> eyre::Result<OnChainOrder> {
    type OrderTuple = (Address, Address, U256, U256, U256, U256);

    let (initiator, redeemer, initiated_at, timelock, amount, fulfilled_at) =
        OrderTuple::abi_decode(data).map_err(|e| eyre::eyre!("Failed to decode order: {}", e))?;

    let to_big_decimal = |v: U256, name| {
        let v_str = v.to_string();
        let v_big_decimal = BigDecimal::from_str(&v_str)
            .map_err(|e| eyre::eyre!("Failed to convert {}: {}", name, e))?;
        eyre::Ok(v_big_decimal)
    };

    Ok(OnChainOrder {
        initiator: initiator.to_string(),
        redeemer: redeemer.to_string(),
        initiated_at: to_big_decimal(initiated_at, "initiated_at")?,
        timelock: to_big_decimal(timelock, "timelock")?,
        amount: to_big_decimal(amount, "amount")?,
        fulfilled_at: to_big_decimal(fulfilled_at, "fulfilled_at")?,
    })
}
