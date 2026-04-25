use crate::{
    onchain_orders::primitives::OnChainOrder,
    swaps::{EventTxInfo, HTLCOrder, OrderSwapId, SwapEvent, SwapEventType},
};
use alloy::{eips::BlockNumberOrTag, providers::Provider};
use alloy_sol_types::SolEventInterface;
use eyre::eyre;
use tars::evm::{
    GardenHTLCv2::GardenHTLCv2Events, GardenHTLCv3::GardenHTLCv3Events, events::EventExt,
};
use sqlx::types::{BigDecimal, chrono};
use std::collections::BTreeSet;
use std::{collections::HashMap, sync::Arc};

/// Prepares store events from HTLC v2 and v3 events for store usage.
pub async fn prepare_store_events(
    chain_name: &str,
    provider: Arc<dyn Provider>,
    v2_events: &[EventExt<GardenHTLCv2Events>],
    v3_events: &[EventExt<GardenHTLCv3Events>],
    onchain_order_map: &HashMap<String, OnChainOrder>,
) -> eyre::Result<Vec<SwapEvent>> {
    if v2_events.is_empty() && v3_events.is_empty() {
        return Ok(vec![]);
    }

    // Collect unique block numbers
    let block_numbers: Vec<u64> = v2_events
        .iter()
        .map(|event| event.block_number.unwrap_or_default())
        .chain(
            v3_events
                .iter()
                .map(|event| event.block_number.unwrap_or_default()),
        )
        .filter(|&block_number| block_number != 0)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();

    // Fetch block times concurrently
    let block_time_map: HashMap<u64, chrono::DateTime<chrono::Utc>> =
        futures::future::try_join_all(block_numbers.iter().map(|&block_number| {
            let provider = provider.clone();
            async move {
                get_block_time(provider, block_number)
                    .await
                    .map(|time| (block_number, time))
            }
        }))
        .await?
        .into_iter()
        .collect();

    let mut swap_events = Vec::with_capacity(v2_events.len() + v3_events.len());

    // Process v2 events
    for event in v2_events {
        if let Some(event_data) = get_v2_event_data(&event.event) {
            if let Some(swap_event) = create_swap_event(
                event_data,
                event,
                onchain_order_map,
                &block_time_map,
                chain_name,
            ) {
                swap_events.push(swap_event);
            }
        }
    }

    // Process v3 events
    for event in v3_events {
        if let Some(event_data) = get_v3_event_data(&event.event) {
            if let Some(swap_event) = create_swap_event(
                event_data,
                event,
                onchain_order_map,
                &block_time_map,
                chain_name,
            ) {
                swap_events.push(swap_event);
            }
        }
    }

    Ok(swap_events)
}

/// Extract order ID and event type from v2 events
fn get_v2_event_data(event: &GardenHTLCv2Events) -> Option<(String, SwapEventType)> {
    match event {
        GardenHTLCv2Events::Initiated(e) => Some((e.orderID.to_string(), SwapEventType::Initiate)),
        GardenHTLCv2Events::Redeemed(e) => {
            let secret = match e.secret.to_string().try_into() {
                Ok(secret) => secret,
                Err(_) => {
                    tracing::error!(
                        "Failed to convert secret to OrderSecret for order {} ",
                        e.orderID.to_string()
                    );
                    return None;
                }
            };
            Some((e.orderID.to_string(), SwapEventType::Redeem(secret)))
        }
        GardenHTLCv2Events::Refunded(e) => Some((e.orderID.to_string(), SwapEventType::Refund)),
        _ => None,
    }
}

/// Extract order ID and event type from v3 events
fn get_v3_event_data(event: &GardenHTLCv3Events) -> Option<(String, SwapEventType)> {
    match event {
        GardenHTLCv3Events::Initiated(e) => Some((e.orderID.to_string(), SwapEventType::Initiate)),
        GardenHTLCv3Events::InitiatedWithDestinationData(e) => {
            Some((e.orderID.to_string(), SwapEventType::Initiate))
        }
        GardenHTLCv3Events::Redeemed(e) => {
            let secret = match e.secret.to_string().try_into() {
                Ok(secret) => secret,
                Err(_) => {
                    tracing::error!(
                        "Failed to convert secret to OrderSecret for order {} ",
                        e.orderID.to_string()
                    );
                    return None;
                }
            };
            Some((e.orderID.to_string(), SwapEventType::Redeem(secret)))
        }
        GardenHTLCv3Events::Refunded(e) => Some((e.orderID.to_string(), SwapEventType::Refund)),
        _ => None,
    }
}

/// Creates a swap event from processed data
fn create_swap_event<T: SolEventInterface>(
    // event_data is (order_id, event_type)
    event_data: (String, SwapEventType),
    event: &EventExt<T>,
    onchain_order_map: &HashMap<String, OnChainOrder>,
    block_time_map: &HashMap<u64, chrono::DateTime<chrono::Utc>>,
    chain_name: &str,
) -> Option<SwapEvent> {
    let (order_id, event_type) = event_data;
    let order_id = OrderSwapId::from(order_id);
    let onchain_order = onchain_order_map.get(&order_id.to_string())?;
    let block_number = event.block_number.unwrap_or_default();
    let block_time = *block_time_map
        .get(&block_number)
        .unwrap_or(&chrono::Utc::now());

    Some(SwapEvent::new(
        event_type,
        order_id.into(),
        EventTxInfo::new(
            event
                .transaction_hash
                .map(|h| h.to_string())
                .unwrap_or_default(),
            BigDecimal::from(block_number),
            block_time,
        ),
        HTLCOrder::new(
            onchain_order.redeemer.clone(),
            onchain_order.timelock.clone(),
            onchain_order.amount.clone(),
            event.address.to_string(),
            chain_name.to_string(),
        ),
    ))
}

/// Gets the block timestamp
async fn get_block_time(
    provider: Arc<dyn Provider>,
    block_number: u64,
) -> eyre::Result<chrono::DateTime<chrono::Utc>> {
    match provider
        .get_block_by_number(BlockNumberOrTag::Number(block_number))
        .await
    {
        Ok(Some(block)) => Ok(
            chrono::DateTime::from_timestamp(block.header.timestamp as i64, 0)
                .ok_or_else(|| eyre!("Invalid timestamp for block {}", block_number))?,
        ),
        Ok(None) => {
            tracing::error!("Block {} not found", block_number);
            return Err(eyre!("Block {} not found", block_number));
        }
        Err(e) => {
            tracing::error!("Failed to get block time for block {}: {}", block_number, e);
            return Err(eyre!(
                "Failed to get block time for block {}: {}",
                block_number,
                e
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swaps::{OrderSecret, OrderSwapId};
    use alloy::{primitives::Address, providers::ProviderBuilder};
    use alloy_primitives::Uint;
    use tars::{
        evm::{GardenHTLCv2, GardenHTLCv3},
        utils::ToBytes,
    };
    use std::str::FromStr;

    const ARB_SEPOLIA_URL: &str = "https://arbitrum-sepolia-rpc.publicnode.com";
    const V3_HTLC_ADDR: &str = "0x9648B9d01242F537301b98EC0Bf8b6854cDB97E6";
    const TEST_SWAP_ID: &str = "09bdaff31fb3c79592618804948a20f7c02c721899f2612e312fe1f9d61140bf";
    const TEST_SECRET_HASH: &str =
        "0xc457a9cc503cc59af8079652e3e84891ea0974cad3cb8214685427aac9d91dd4";
    const TEST_TX_HASH: &str = "0x1ab94d1c1aa1612d9b41eaee89c93e6453b9fbbc26040a7395ad4affed042954";
    const TEST_BLOCK_HASH: &str =
        "0x21626b9b3c50bf44bca0a1c8012955cb09826d6e7a39479c0c99d7e4e6a69980";

    async fn get_provider() -> Arc<dyn Provider> {
        Arc::new(ProviderBuilder::new().connect_http(ARB_SEPOLIA_URL.parse().unwrap()))
    }

    fn setup_onchain_order_map(
        initiator: &str,
        redeemer: &str,
        redeemer_amount: u64,
        block_number: u64,
    ) -> HashMap<String, OnChainOrder> {
        HashMap::from([(
            TEST_SWAP_ID.to_string(),
            OnChainOrder {
                initiator: initiator.to_string(),
                redeemer: redeemer.to_string(),
                initiated_at: BigDecimal::from(block_number - 15),
                timelock: BigDecimal::from(36000),
                amount: BigDecimal::from(redeemer_amount),
                fulfilled_at: BigDecimal::from(block_number),
            },
        )])
    }

    fn setup_event_ext<T: SolEventInterface>(event: T, block_number: u64) -> EventExt<T> {
        EventExt {
            event,
            address: Address::from_str(V3_HTLC_ADDR).unwrap(),
            block_hash: Some(TEST_BLOCK_HASH.hex_to_fixed_bytes().unwrap()),
            block_number: Some(block_number),
            transaction_hash: Some(TEST_TX_HASH.hex_to_fixed_bytes().unwrap()),
            transaction_index: None,
            log_index: None,
            removed: false,
        }
    }

    #[tokio::test]
    async fn test_initiated_event_processing() {
        let onchain_order_map = setup_onchain_order_map(
            "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075",
            "0x6DA99883352D5d3047E753667A62b06A78cD8E1c",
            49700,
            192482538,
        );

        let v3_initiated = GardenHTLCv3Events::Initiated(GardenHTLCv3::Initiated {
            orderID: TEST_SWAP_ID.hex_to_fixed_bytes().unwrap(),
            secretHash: TEST_SECRET_HASH.hex_to_fixed_bytes().unwrap(),
            amount: Uint::from(49700),
        });

        let events = [setup_event_ext(v3_initiated, 192482538)];
        let swap_events = prepare_store_events(
            "arbitrum_sepolia",
            get_provider().await,
            &[],
            &events,
            &onchain_order_map,
        )
        .await
        .unwrap();

        let swap_event = swap_events.first().expect("Expected one swap event");
        assert_eq!(swap_events.len(), 1, "Expected exactly one swap event");
        assert_eq!(
            swap_event.event_type,
            SwapEventType::Initiate,
            "Event type should be Initiate"
        );
        assert_eq!(
            swap_event.swap_id,
            OrderSwapId::from(TEST_SWAP_ID.to_string()),
            "Swap ID mismatch"
        );
        assert_eq!(
            swap_event.tx_info.tx_hash.hex_to_fixed_bytes().unwrap(),
            TEST_TX_HASH.hex_to_fixed_bytes().unwrap(),
            "Transaction hash mismatch"
        );
        assert_eq!(
            swap_event.tx_info.block_number,
            BigDecimal::from(192482538),
            "Block number mismatch"
        );
        assert_eq!(
            swap_event
                .tx_info
                .timestamp
                .format("%b-%d-%Y %I:%M:%S %p %Z")
                .to_string(),
            "Sep-09-2025 12:08:05 PM UTC",
            "Block timestamp mismatch"
        );
    }

    #[tokio::test]
    async fn test_redeem_event_processing() {
        let _ = tracing_subscriber::fmt().try_init();

        let onchain_order_map = setup_onchain_order_map(
            "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075",
            "0x004Cc75ACF4132Fc08cB6a252E767804F303F729",
            48354,
            192473022,
        );

        let v3_redeemed = GardenHTLCv3Events::Redeemed(GardenHTLCv3::Redeemed {
            orderID: TEST_SWAP_ID.hex_to_fixed_bytes().unwrap(),
            secret: TEST_SECRET_HASH.hex_to_bytes().unwrap(),
            secretHash: TEST_SECRET_HASH.hex_to_fixed_bytes().unwrap(),
        });

        let events = [setup_event_ext(v3_redeemed, 192482523)];
        let swap_events = prepare_store_events(
            "arbitrum_sepolia",
            get_provider().await,
            &[],
            &events,
            &onchain_order_map,
        )
        .await
        .unwrap();

        let swap_event = swap_events.first().expect("Expected one swap event");
        assert_eq!(swap_events.len(), 1, "Expected exactly one swap event");
        assert_eq!(
            swap_event.event_type,
            SwapEventType::Redeem(OrderSecret::new(TEST_SECRET_HASH.to_string()).unwrap()),
            "Event type should be Redeem"
        );
        assert_eq!(
            swap_event.swap_id,
            OrderSwapId::from(TEST_SWAP_ID.to_string()),
            "Swap ID mismatch"
        );
        assert_eq!(
            swap_event.tx_info.tx_hash.hex_to_fixed_bytes().unwrap(),
            TEST_TX_HASH.hex_to_fixed_bytes().unwrap(),
            "Transaction hash mismatch"
        );
        assert_eq!(
            swap_event.tx_info.block_number,
            BigDecimal::from(192482523),
            "Block number mismatch"
        );
        assert_eq!(
            swap_event
                .tx_info
                .timestamp
                .format("%b-%d-%Y %I:%M:%S %p %Z")
                .to_string(),
            "Sep-09-2025 12:08:01 PM UTC",
            "Block timestamp mismatch"
        );
        assert_eq!(
            swap_event.order.redeemer, "0x004Cc75ACF4132Fc08cB6a252E767804F303F729",
            "Redeemer address mismatch"
        );
        assert_eq!(
            swap_event.order.amount,
            BigDecimal::from(48354),
            "Amount mismatch"
        );
    }

    #[tokio::test]
    async fn test_event_data() {
        use tars::evm::GardenHTLCv2::GardenHTLCv2Events;
        use tars::evm::GardenHTLCv3::GardenHTLCv3Events;

        let v2_initiated = GardenHTLCv2::Initiated {
            orderID: [0u8; 32].into(),
        };
        let v2_redeemed = GardenHTLCv2::Redeemed {
            orderID: [1u8; 32].into(),
            secret: [2u8; 32].into(),
        };
        let v2_refunded = GardenHTLCv2::Refunded {
            orderID: [3u8; 32].into(),
        };

        use alloy_primitives::Uint;
        let v3_initiated = GardenHTLCv3::Initiated {
            orderID: [4u8; 32].into(),
            secretHash: [5u8; 32].into(),
            amount: Uint::from(1000u64),
        };
        let v3_redeemed = GardenHTLCv3::Redeemed {
            orderID: [6u8; 32].into(),
            secretHash: [5u8; 32].into(),
            secret: [7u8; 32].into(),
        };
        let v3_refunded = GardenHTLCv3::Refunded {
            orderID: [8u8; 32].into(),
        };

        let event_v2_initiated = GardenHTLCv2Events::Initiated(v2_initiated.clone());
        let event_v2_redeemed = GardenHTLCv2Events::Redeemed(v2_redeemed.clone());
        let event_v2_refunded = GardenHTLCv2Events::Refunded(v2_refunded.clone());
        let event_v3_initiated = GardenHTLCv3Events::Initiated(v3_initiated.clone());
        let event_v3_redeemed = GardenHTLCv3Events::Redeemed(v3_redeemed.clone());
        let event_v3_refunded = GardenHTLCv3Events::Refunded(v3_refunded.clone());
        assert_eq!(
            get_v2_event_data(&event_v2_initiated).unwrap(),
            (v2_initiated.orderID.to_string(), SwapEventType::Initiate),
        );
        assert_eq!(
            get_v2_event_data(&event_v2_redeemed).unwrap(),
            (
                v2_redeemed.orderID.to_string(),
                SwapEventType::Redeem(OrderSecret::new(v2_redeemed.secret.to_string()).unwrap())
            )
        );
        assert_eq!(
            get_v2_event_data(&event_v2_refunded).unwrap(),
            (v2_refunded.orderID.to_string(), SwapEventType::Refund)
        );
        assert_eq!(
            get_v3_event_data(&event_v3_initiated).unwrap(),
            (v3_initiated.orderID.to_string(), SwapEventType::Initiate)
        );
        assert_eq!(
            get_v3_event_data(&event_v3_redeemed).unwrap(),
            (
                v3_redeemed.orderID.to_string(),
                SwapEventType::Redeem(OrderSecret::new(v3_redeemed.secret.to_string()).unwrap())
            )
        );
        assert_eq!(
            get_v3_event_data(&event_v3_refunded).unwrap(),
            (v3_refunded.orderID.to_string(), SwapEventType::Refund)
        );
        assert_eq!(
            get_v3_event_data(&event_v3_refunded).unwrap(),
            (v3_refunded.orderID.to_string(), SwapEventType::Refund)
        );
    }

    #[tokio::test]
    async fn test_get_blocktime() {
        let provider = get_provider().await;
        let blocktime = provider
            .get_block_by_number(BlockNumberOrTag::Number(192482538))
            .await
            .unwrap()
            .unwrap()
            .header
            .timestamp;
        let given_blocktime = get_block_time(provider, 192482538).await.unwrap();
        assert_eq!(
            given_blocktime,
            chrono::DateTime::from_timestamp(blocktime as i64, 0).unwrap(),
            "Block timestamp mismatch"
        );
    }
}
