use crate::{
    primitives::BlockRange,
    stream_event::garden_htlc::{AddressFilter, EventProvider},
};
use futures::StreamExt;
use tars::evm::event_provider;
use tars::evm::events::prelude;
use tars::{
    evm::{GardenHTLCv2::GardenHTLCv2Events, GardenHTLCv3::GardenHTLCv3Events, events::EventExt},
    utils::NonEmptyVec,
};
use prelude::*;
use std::{ops::Deref, sync::Arc};
use tokio::sync::mpsc::{self};
use tokio_stream::wrappers::ReceiverStream;

// event provider for garden htlc v2 and v3
event_provider!(
    garden_htlc,
    (htlc_v2, GardenHTLCv2Events),
    (htlc_v3, GardenHTLCv3Events),
);

/// Streams Garden contract events (v2 and v3) for specified block ranges and address filter.
///
/// # Arguments
/// * `provider` - The event provider for querying logs.
/// * `block_ranges` - Non-empty vector of block ranges to query.
/// * `address_filter` - Filter specifying which contract addresses to include.
///
/// # Returns
/// A `ReceiverStream` yielding `eyre::Result` containing vectors of v2 and v3 events for each block range chunk.
pub async fn stream_garden_events(
    provider: Arc<EventProvider>,
    block_ranges: &NonEmptyVec<BlockRange>,
    address_filter: AddressFilter,
) -> ReceiverStream<
    eyre::Result<(
        Vec<EventExt<GardenHTLCv2Events>>,
        Vec<EventExt<GardenHTLCv3Events>>,
    )>,
> {
    const QUERY_BUFFER_SIZE: usize = 100;

    let (tx, rx) = mpsc::channel(QUERY_BUFFER_SIZE);
    let block_ranges = block_ranges.deref().clone();

    tokio::spawn(async move {
        let mut stream = tokio_stream::iter(block_ranges)
            .map(|range| {
                let filter = address_filter.clone();
                let provider = Arc::clone(&provider);
                async move {
                    provider
                        .query(range.from_block, range.to_block, &filter)
                        .await
                }
            })
            .buffered(QUERY_BUFFER_SIZE);

        while let Some(result) = stream.next().await {
            if let Err(e) = tx.send(result).await {
                tracing::error!("Failed to send result to channel: {}", e);
                break;
            }
        }
    });

    ReceiverStream::new(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::BlockRange;
    use alloy::providers::{Provider, ProviderBuilder};
    use futures::StreamExt;
    use std::{sync::Arc, u64};

    const ARB_SEPOLIA_URL: &str = "https://arbitrum-sepolia.therpc.io";
    const V3_HTLC_ADDR: &str = "0x9648B9d01242F537301b98EC0Bf8b6854cDB97E6";

    fn get_provider() -> impl Provider {
        ProviderBuilder::new().connect_http(ARB_SEPOLIA_URL.parse().unwrap())
    }

    async fn setup_garden_events_provider() -> Arc<EventProvider> {
        let provider = Arc::new(get_provider());
        Arc::new(garden_htlc::EventProvider::new(provider, Some(9000), None))
    }

    fn create_v3_filter() -> garden_htlc::AddressFilter {
        garden_htlc::AddressFilter::new()
            .htlc_v2(vec![])
            .htlc_v3(vec![V3_HTLC_ADDR.parse().unwrap()])
    }

    async fn collect_stream_results(
        mut stream: ReceiverStream<
            eyre::Result<(
                Vec<EventExt<GardenHTLCv2Events>>,
                Vec<EventExt<GardenHTLCv3Events>>,
            )>,
        >,
    ) -> (usize, usize, usize) {
        let mut chunk_count = 0;
        let mut total_v2_events = 0;
        let mut total_v3_events = 0;
        while let Some(result) = stream.next().await {
            chunk_count += 1;
            match result {
                Ok((v2_events, v3_events)) => {
                    total_v2_events += v2_events.len();
                    total_v3_events += v3_events.len();
                }
                Err(e) => {
                    tracing::error!("Error in chunk {}: {}", chunk_count, e);
                }
            }
        }
        (chunk_count, total_v2_events, total_v3_events)
    }

    #[tokio::test]
    async fn stream_logs_single_range_returns_expected_v3_event_count() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = setup_garden_events_provider().await;
        let ranges = vec![BlockRange {
            from_block: 192138510,
            to_block: 192138510,
        }];
        let filter = create_v3_filter();

        let stream =
            stream_garden_events(provider, &NonEmptyVec::new(ranges).unwrap(), filter).await;

        let (_chunks, _v2, v3) = collect_stream_results(stream).await;
        assert_eq!(v3, 1);
    }

    #[tokio::test]
    async fn stream_logs_multiple_ranges_returns_chunks_and_events() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = setup_garden_events_provider().await;
        let ranges = vec![
            BlockRange {
                from_block: 192138510,
                to_block: 192138510,
            },
            BlockRange {
                from_block: 192138916,
                to_block: 192138916,
            },
        ];
        let filter = create_v3_filter();

        let stream =
            stream_garden_events(provider, &NonEmptyVec::new(ranges).unwrap(), filter).await;

        let (chunk_count, _v2, v3) = collect_stream_results(stream).await;
        assert_eq!(chunk_count, 2);
        assert!(v3 > 0);
    }

    #[tokio::test]
    async fn stream_logs_range_with_no_events_returns_empty() {
        let provider = setup_garden_events_provider().await;
        let ranges = vec![BlockRange {
            from_block: 1,
            to_block: 100,
        }];
        let filter = create_v3_filter();

        let stream =
            stream_garden_events(provider, &NonEmptyVec::new(ranges).unwrap(), filter).await;

        let (chunk_count, v2, v3) = collect_stream_results(stream).await;
        assert_eq!(chunk_count, 1);
        assert_eq!(v2, 0);
        assert_eq!(v3, 0);
    }

    #[tokio::test]
    async fn stream_logs_handles_stream_errors_gracefully() {
        let provider = setup_garden_events_provider().await;
        let ranges = vec![BlockRange {
            from_block: u64::MAX - 1000000,
            to_block: u64::MAX - 1000000,
        }];
        let filter = create_v3_filter();

        let mut stream =
            stream_garden_events(provider, &NonEmptyVec::new(ranges).unwrap(), filter).await;

        let mut saw_error = false;
        while let Some(result) = stream.next().await {
            match result {
                Ok((v2_events, v3_events)) => {
                    assert_eq!(v2_events.len(), 0);
                    assert_eq!(v3_events.len(), 0);
                }
                Err(_e) => {
                    saw_error = true;
                }
            }
        }
        assert!(
            saw_error,
            "Expected at least one error for invalid block range"
        );
    }

    #[tokio::test]
    async fn stream_logs_returns_events_in_order() {
        let _ = tracing_subscriber::fmt().try_init();
        let provider = setup_garden_events_provider().await;
        let ranges = vec![
            BlockRange {
                from_block: 192138510,
                to_block: 192138510,
            },
            BlockRange {
                from_block: 192138916,
                to_block: 192138916,
            },
        ];
        let filter = create_v3_filter();

        let mut stream =
            stream_garden_events(provider, &NonEmptyVec::new(ranges).unwrap(), filter).await;
        let mut chunk_count = 0;
        let mut all_v3_events: Vec<EventExt<GardenHTLCv3Events>> = vec![];
        while let Some(result) = stream.next().await {
            chunk_count += 1;
            match result {
                Ok((_, v3_events)) => {
                    all_v3_events.extend(v3_events);
                }
                Err(e) => {
                    tracing::error!("Error in chunk {}: {}", chunk_count, e);
                }
            }
        }

        assert_eq!(chunk_count, 2);
        let older_event = all_v3_events.first().unwrap();
        let newer_event = all_v3_events.last().unwrap();
        match &newer_event.event {
            GardenHTLCv3Events::Initiated(event) => {
                assert_eq!(
                    event.orderID.to_string(),
                    "0xba562e0a36db60c88f93fa0759343394449b2ad054a73d9123a466db71337cbd"
                );
                assert_eq!(
                    event.secretHash.to_string(),
                    "0x648592429acb1b728f6d174d468c0b1b32573b9899032339bea695a4d8553201"
                );
            }
            _ => panic!("Expected Initiated event"),
        }

        match &older_event.event {
            GardenHTLCv3Events::Initiated(event) => {
                assert_eq!(
                    event.orderID.to_string(),
                    "0x8eee9625276b49bfce53b1cac71fd89bafa01eeacb1149d26f2d1876881d0f6c"
                );
                assert_eq!(
                    event.secretHash.to_string(),
                    "0x648592429acb1b728f6d174d468c0b1b32573b9899032339bea695a4d8553202"
                );
            }
            _ => panic!("Expected Initiated event"),
        }
    }
}
