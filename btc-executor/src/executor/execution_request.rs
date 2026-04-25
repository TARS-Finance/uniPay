use tars::orderbook::primitives::{MatchedOrderVerbose, SingleSwap};
use tars::primitives::HTLCAction;

#[derive(Clone, Debug)]
pub struct OrderExecutionRequest {
    pub cache_key: String,
    pub order_id: String,
    pub action: HTLCAction,
    pub order: MatchedOrderVerbose,
    pub swap: SingleSwap,
}

impl OrderExecutionRequest {
    pub fn new(order: MatchedOrderVerbose, swap: SingleSwap, action: HTLCAction) -> Self {
        let order_id = order.create_order.create_id.clone();
        let cache_key = format!("{order_id}:{}:{action}", swap.swap_id);

        Self {
            cache_key,
            order_id,
            action,
            order,
            swap,
        }
    }
}

#[cfg(test)]
mod tests {
    use tars::orderbook::test_utils::default_matched_order;
    use tars::primitives::HTLCAction;

    use super::OrderExecutionRequest;

    #[test]
    fn cache_key_includes_swap_id() {
        let mut order = default_matched_order();
        order.create_order.create_id = "order-1".to_string();
        let mut swap = order.destination_swap.clone();
        swap.swap_id = "swap-1".to_string();

        let request = OrderExecutionRequest::new(order, swap, HTLCAction::Refund);

        assert_eq!(request.cache_key, "order-1:swap-1:Refund");
    }
}
