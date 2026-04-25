use crate::swaps::{SwapEvent, SwapEventType};
use tars::orderbook::primitives::SingleSwap;
use std::collections::HashMap;

/// Validates a list of swap events against pending swaps
pub fn validate_events<'a>(swaps: &[&SingleSwap], events: &'a [SwapEvent]) -> Vec<&'a SwapEvent> {
    if swaps.is_empty() || events.is_empty() {
        return vec![];
    }

    let swap_lookup: HashMap<&str, &SingleSwap> = swaps
        .iter()
        .map(|s| (s.swap_id.as_str(), *s))
        .collect::<HashMap<_, _>>();

    events
        .into_iter()
        .filter_map(|event| {
            let swap = swap_lookup.get(event.swap_id.as_str())?;
            validate_event(event, swap)
                .map_err(|e| tracing::error!("Failed to validate event: {}", e))
                .ok()?;
            Some(event)
        })
        .collect()
}
fn validate_event(event: &SwapEvent, swap: &SingleSwap) -> eyre::Result<()> {
    let event_timelock = event
        .order
        .timelock
        .to_string()
        .parse::<i32>()
        .map_err(|_| validation_error(event, swap))?;

    if !event.order.redeemer.eq_ignore_ascii_case(&swap.redeemer)
        || event_timelock != swap.timelock
        || !event.order.asset_address.eq_ignore_ascii_case(&swap.asset)
        || !event.order.chain.eq_ignore_ascii_case(&swap.chain)
    {
        return Err(validation_error(event, swap));
    }
    Ok(())
}

/// Formats a validation error for a single swap event
fn validation_error(event: &SwapEvent, swap: &SingleSwap) -> eyre::Report {
    let (event_type, fields) = match event.event_type {
        SwapEventType::Initiate => (
            "initiation",
            format!(
                "expected (redeemer: '{}', timelock: {}, asset: '{}', chain: '{}'), got (redeemer: '{}', timelock: {}, asset: '{}', chain: '{}')",
                swap.redeemer,
                swap.timelock,
                swap.asset,
                swap.chain,
                event.order.redeemer,
                event.order.timelock,
                event.order.asset_address,
                event.order.chain
            ),
        ),
        SwapEventType::Redeem(_) => (
            "redeem",
            format!(
                "expected (redeemer: '{}', asset: '{}', chain: '{}'), got (redeemer: '{}', asset: '{}', chain: '{}')",
                swap.redeemer,
                swap.asset,
                swap.chain,
                event.order.redeemer,
                event.order.asset_address,
                event.order.chain
            ),
        ),
        SwapEventType::Refund => (
            "refund",
            format!(
                "expected (asset: '{}', chain: '{}'), got (asset: '{}', chain: '{}')",
                swap.asset, swap.chain, event.order.asset_address, event.order.chain
            ),
        ),
    };
    eyre::eyre!(
        "Invalid {} event for swap '{}': {}",
        event_type,
        event.swap_id.to_string(),
        fields
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swaps::{EventTxInfo, HTLCOrder};
    use tars::orderbook::test_utils::default_matched_order;
    use sqlx::types::{BigDecimal, chrono};

    #[tokio::test]
    async fn test_validate_events() {
        let test_swap = default_matched_order().source_swap;
        let test_swap_event = SwapEvent::new(
            SwapEventType::Initiate,
            test_swap.swap_id.clone().into(),
            EventTxInfo::new(
                "transaction_hash".to_string(),
                BigDecimal::from(1),
                chrono::Utc::now(),
            ),
            HTLCOrder::new(
                test_swap.redeemer.clone(),
                test_swap.timelock.clone().into(),
                test_swap.amount.clone(),
                test_swap.asset.to_string(),
                test_swap.chain.to_string(),
            ),
        );

        // test with valid data
        let events = [test_swap_event.clone()];
        let validated_events = validate_events(&[&&test_swap], &events);
        assert_eq!(validated_events.len(), 1);
        assert_eq!(
            validated_events[0].swap_id,
            test_swap.swap_id.clone().into()
        );
        assert_eq!(validated_events[0].event_type, SwapEventType::Initiate);
        assert_eq!(&validated_events[0].tx_info, &test_swap_event.tx_info);
        assert_eq!(&validated_events[0].order, &test_swap_event.order);

        // test invalid inits
        // 1 : redeemer
        // 2 :timelock
        // 3 : asset
        // 4 : chain

        let mut invalid_redeemer_event = test_swap_event.clone();
        invalid_redeemer_event.order.redeemer = "invalid_redeemer".to_string();
        let events = [invalid_redeemer_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_redeemer_event, &test_swap).is_err());

        let mut invalid_timelock_event = test_swap_event.clone();
        invalid_timelock_event.order.timelock = BigDecimal::from(2);
        let events = [invalid_timelock_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_timelock_event, &test_swap).is_err());

        let mut invalid_asset_event = test_swap_event.clone();
        invalid_asset_event.order.asset_address = "invalid_asset".to_string();
        let events = [invalid_asset_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_asset_event, &test_swap).is_err());

        let mut invalid_chain_event = test_swap_event.clone();
        invalid_chain_event.order.chain = "invalid_chain".to_string();
        let events = [invalid_chain_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_chain_event, &test_swap).is_err());

        // test invalid redeems
        // 1 : redeemer
        // 2 : asset
        // 3 : chain

        let mut invalid_redeemer_event = test_swap_event.clone();
        invalid_redeemer_event.order.redeemer = "invalid_redeemer".to_string();
        let events = [invalid_redeemer_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_redeemer_event, &test_swap).is_err());

        let mut invalid_asset_event = test_swap_event.clone();
        invalid_asset_event.order.asset_address = "invalid_asset".to_string();
        let events = [invalid_asset_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_asset_event, &test_swap).is_err());

        // test invalid refunds
        let mut invalid_refund_event = test_swap_event.clone();
        invalid_refund_event.order.asset_address = "invalid_asset".to_string();
        let events = [invalid_refund_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_refund_event, &test_swap).is_err());

        let mut invalid_chain_event = test_swap_event.clone();
        invalid_chain_event.order.chain = "invalid_chain".to_string();
        let events = [invalid_chain_event.clone()];
        let validated_events = validate_events(&[&test_swap], &events);
        assert_eq!(validated_events.len(), 0);
        assert!(validate_event(&invalid_chain_event, &test_swap).is_err());

        // test mulitple events are are handled properly if one event is invalid
        let mut invalid_redeemer_event = test_swap_event.clone();
        invalid_redeemer_event.order.redeemer = "invalid_redeemer".to_string().into();
        invalid_redeemer_event.swap_id = "invalid_event_swap_id".to_string().into();
        let mut invalid_redeemer_swaps = test_swap.clone();
        invalid_redeemer_swaps.swap_id = "invalid_event_swap_id".to_string();

        let valid_event = test_swap_event.clone();
        let valid_swap = &test_swap.clone();
        let events = [invalid_redeemer_event.clone(), valid_event.clone()];
        let validated_events = validate_events(&[&invalid_redeemer_swaps, valid_swap], &events);
        assert_eq!(validated_events.len(), 1);
        assert_eq!(validated_events[0].event_type, SwapEventType::Initiate);
        assert_eq!(&validated_events[0].tx_info, &valid_event.tx_info);
        assert_eq!(&validated_events[0].order, &valid_event.order);
    }
}
