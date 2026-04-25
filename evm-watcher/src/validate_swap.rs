use crate::primitives::OrderStatus;
use tars::orderbook::primitives::SingleSwap;
use std::collections::HashMap;

/// Returns a list of swap IDs that need processing based on their on-chain
/// status compared to their pending status in the database.
/// A swap is processable if:
/// - Its on-chain status differs from its pending status, or
/// - It is stuck in the Initiated state for longer than its allowed window, or
/// - It exists in pending swaps but has no on-chain status.
pub fn validate_swaps(
    onchain_statuses: &HashMap<String, OrderStatus>,
    pending_swaps: &HashMap<String, &SingleSwap>,
    current_block: u64,
) -> Vec<String> {
    pending_swaps
        .iter()
        .filter_map(|(swap_id, swap)| {
            let swap_status = OrderStatus::from(*swap);
            match onchain_statuses.get(swap_id) {
                // Status mismatch between onchain and pending
                Some(onchain_status) if onchain_status != &swap_status => Some(swap_id.clone()),
                // Check if initiated swap has exceeded its confirmation window
                Some(OrderStatus::Initiated) => {
                    let init_block = swap
                        .initiate_block_number
                        .as_ref()
                        .and_then(|n| n.to_string().parse::<u64>().ok())
                        .unwrap_or(0);
                    if current_block <= init_block + swap.required_confirmations as u64 {
                        Some(swap_id.clone())
                    } else {
                        None
                    }
                }
                // Swap exists in pending but not in onchain
                None => Some(swap_id.clone()),
                // No action needed for matching statuses within window
                _ => None,
            }
        })
        .collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    use tars::orderbook::primitives::SingleSwap;
    use tars::orderbook::test_utils::default_matched_order;
    use sqlx::types::BigDecimal;
    use std::collections::HashMap;

    // Mock SingleSwap for testing
    #[derive(Clone, PartialEq)]
    struct MockSingleSwap {
        initiate_block_number: Option<BigDecimal>,
        redeem_block_number: Option<BigDecimal>,
        refund_block_number: Option<BigDecimal>,
        required_confirmations: i32,
    }

    fn new_single_swap(mock: MockSingleSwap) -> SingleSwap {
        let mut swap = default_matched_order().source_swap;
        swap.initiate_block_number = mock.initiate_block_number;
        swap.redeem_block_number = mock.redeem_block_number;
        swap.refund_block_number = mock.refund_block_number;
        swap.required_confirmations = mock.required_confirmations;
        swap
    }

    /// Tests for `validate_swaps` covering:
    /// - Case 1: No onchain status, all pending swaps are processable.
    /// - Case 2: Onchain statuses differ from pending, only differing swaps are processable.
    /// - Case 3: Onchain statuses match pending, no swaps are processable unless initiated swaps exceed window.
    /// - Case 4: Initiated swaps exceeding confirmation window are processable.
    #[test]
    fn test_validate_swaps_various_cases() {
        let one_hundred = BigDecimal::from(100);
        let one = BigDecimal::from(1);

        // Case 1: No onchain status
        let mut pending_swaps = HashMap::new();
        let swap1 = MockSingleSwap {
            initiate_block_number: Some(one_hundred.clone()),
            redeem_block_number: None,
            refund_block_number: None,
            required_confirmations: 10,
        };
        let swap2 = MockSingleSwap {
            initiate_block_number: None,
            redeem_block_number: None,
            refund_block_number: None,
            required_confirmations: 10,
        };
        let swap1 = new_single_swap(swap1);
        let swap2 = new_single_swap(swap2);
        pending_swaps.insert("swap1".to_string(), &swap1);
        pending_swaps.insert("swap2".to_string(), &swap2);
        let onchain_statuses = HashMap::new();
        let current_block = 115;

        let result = validate_swaps(&onchain_statuses, &pending_swaps, current_block);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"swap1".to_string()));
        assert!(result.contains(&"swap2".to_string()));

        // Case 2: Different statuses
        let mut onchain_statuses = HashMap::new();
        onchain_statuses.insert("swap1".to_string(), OrderStatus::Fulfilled);
        onchain_statuses.insert("swap2".to_string(), OrderStatus::NotInitiated);

        let result = validate_swaps(&onchain_statuses, &pending_swaps, current_block);
        assert_eq!(result.len(), 1);
        assert!(result.contains(&"swap1".to_string()));

        // Case 3: Same statuses, within confirmation window
        let mut onchain_statuses = HashMap::new();
        onchain_statuses.insert("swap1".to_string(), OrderStatus::Initiated);
        onchain_statuses.insert("swap2".to_string(), OrderStatus::NotInitiated);

        let result = validate_swaps(&onchain_statuses, &pending_swaps, 111);
        assert!(result.is_empty());

        // Case 4: Initiated swap exceeding confirmation window
        let result = validate_swaps(&onchain_statuses, &pending_swaps, 115);
        assert_eq!(result.len(), 0);

        // Case 5: Fulfilled swap (redeemed)
        let swap3 = MockSingleSwap {
            initiate_block_number: Some(one_hundred.clone()),
            redeem_block_number: Some(one.clone()),
            refund_block_number: None,
            required_confirmations: 10,
        };
        let mut pending_swaps = HashMap::new();
        let swap3 = new_single_swap(swap3);
        pending_swaps.insert("swap3".to_string(), &swap3);
        let mut onchain_statuses = HashMap::new();
        onchain_statuses.insert("swap3".to_string(), OrderStatus::Initiated);

        let result = validate_swaps(&onchain_statuses, &pending_swaps, 105);
        assert_eq!(result.len(), 1);
        assert!(result.contains(&"swap3".to_string()));
    }
}
