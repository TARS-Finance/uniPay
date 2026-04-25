use crate::onchain_orders::primitives::OnChainOrder;
use tars::orderbook::primitives::SingleSwap;
use sqlx::types::BigDecimal;

/// Represents the status of an order.
#[derive(Debug, Clone, PartialEq)]
pub enum OrderStatus {
    /// The order has not been initiated.
    NotInitiated,
    /// The order has been initiated but not fulfilled.
    Initiated,
    /// The order has been fulfilled (redeemed or refunded).
    Fulfilled,
}

/// Converts a reference to `SingleSwap` into an `OrderStatus`.
impl From<&SingleSwap> for OrderStatus {
    fn from(swap: &SingleSwap) -> Self {
        let zero = BigDecimal::from(0);

        match &swap.initiate_block_number {
            // If initiate_block_number is None or zero, it's not initiated.
            None => OrderStatus::NotInitiated,
            Some(num) if num == &zero => OrderStatus::NotInitiated,
            _ => {
                // Check if the swap has been redeemed or refunded.
                let redeemed = swap
                    .redeem_block_number
                    .as_ref()
                    .map_or(false, |n| n > &zero);
                let refunded = swap
                    .refund_block_number
                    .as_ref()
                    .map_or(false, |n| n > &zero);

                // If redeemed or refunded, it's fulfilled; otherwise, initiated.
                if redeemed || refunded {
                    OrderStatus::Fulfilled
                } else {
                    OrderStatus::Initiated
                }
            }
        }
    }
}

/// Converts a reference to `OnChainOrder` into an `OrderStatus`.
impl From<&OnChainOrder> for OrderStatus {
    fn from(on_chain_order: &OnChainOrder) -> Self {
        // If the order is empty, it's not initiated.
        if on_chain_order.is_empty() {
            OrderStatus::NotInitiated
        // If fulfilled_at is greater than zero, it's fulfilled.
        } else if on_chain_order.fulfilled_at > BigDecimal::from(0) {
            OrderStatus::Fulfilled
        // Otherwise, it's initiated.
        } else {
            OrderStatus::Initiated
        }
    }
}

/// Block range for event querying
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BlockRange {
    pub from_block: u64,
    pub to_block: u64,
}

impl BlockRange {
    pub fn new(from_block: u64, to_block: u64) -> Self {
        Self {
            from_block,
            to_block,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.from_block == 0 || self.to_block == 0
    }
}

#[cfg(test)]
mod tests {
    use alloy::primitives::Address;
    use tars::orderbook::test_utils::default_matched_order;
    use sqlx::types::BigDecimal;

    use crate::{onchain_orders::primitives::OnChainOrder, primitives::OrderStatus};

    /// Tests conversion from `SingleSwap` to `OrderStatus`.
    #[test]
    fn test_order_status_from_single_swap() {
        let mut test_swap = default_matched_order().source_swap;
        assert_eq!(OrderStatus::from(&test_swap), OrderStatus::NotInitiated);

        test_swap.initiate_block_number = Some(BigDecimal::from(1000));
        assert_eq!(OrderStatus::from(&test_swap), OrderStatus::Initiated);

        test_swap.redeem_block_number = Some(BigDecimal::from(1500));
        assert_eq!(OrderStatus::from(&test_swap), OrderStatus::Fulfilled);
        test_swap.redeem_block_number = None;

        test_swap.refund_block_number = Some(BigDecimal::from(1600));
        assert_eq!(OrderStatus::from(&test_swap), OrderStatus::Fulfilled);
    }

    /// Tests conversion from `OnChainOrder` to `OrderStatus`.
    #[test]
    fn test_order_status_from_onchain_order() {
        let empty_order = OnChainOrder {
            initiator: Address::ZERO.to_string(),
            redeemer: Address::ZERO.to_string(),
            initiated_at: BigDecimal::from(0),
            timelock: BigDecimal::from(0),
            amount: BigDecimal::from(0),
            fulfilled_at: BigDecimal::from(0),
        };
        assert_eq!(OrderStatus::from(&empty_order), OrderStatus::NotInitiated);

        let fulfilled_order = OnChainOrder {
            initiator: Address::ZERO.to_string(),
            redeemer: Address::ZERO.to_string(),
            initiated_at: BigDecimal::from(1),
            timelock: BigDecimal::from(1),
            amount: BigDecimal::from(1),
            fulfilled_at: BigDecimal::from(1),
        };

        assert_eq!(OrderStatus::from(&fulfilled_order), OrderStatus::Fulfilled);

        let initiated_order = OnChainOrder {
            initiator: Address::ZERO.to_string(),
            redeemer: Address::ZERO.to_string(),
            initiated_at: BigDecimal::from(1),
            timelock: BigDecimal::from(1),
            amount: BigDecimal::from(1),
            fulfilled_at: BigDecimal::from(0),
        };
        assert_eq!(OrderStatus::from(&initiated_order), OrderStatus::Initiated);
    }
}
