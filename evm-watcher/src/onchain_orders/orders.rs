use crate::onchain_orders::{
    multicall::{Multicall, MulticallResult},
    primitives::{OnChainOrder, OnchainRequest},
};
use alloy::{
    hex::FromHex,
    primitives::{Address, Bytes},
    providers::Provider,
};
use alloy_primitives::U256;
use futures::future::try_join_all;
use tars::evm::{
    GardenHTLC::GardenHTLCInstance,
    Multicall3::{Call3Value, Multicall3Instance},
};
use std::{collections::HashMap, sync::Arc};
use tracing::error;

#[async_trait::async_trait]
pub trait OnChainOrders {
    async fn get_orders(
        &self,
        swap_reqs: &[OnchainRequest],
    ) -> eyre::Result<Vec<Option<OnChainOrder>>>;
}

/// Provider for fetching on-chain orders using multicall for efficient batch operations
///
/// This struct encapsulates the logic for interacting with Garden's on-chain HTLC contracts
/// to retrieve order information in batches using the Multicall3 pattern.
pub struct GardenOnChainOrderProvider<T: Provider> {
    /// Multicall instance for batching multiple contract calls
    multicall: Multicall<T>,
}

impl<T: Provider> GardenOnChainOrderProvider<T> {
    /// Creates a new GardenOnChainOrderProvider instance
    ///
    /// # Arguments
    /// * `multicall_addr` - The address of the Multicall3 contract
    /// * `provider` - The provider instance
    ///
    /// # Returns
    /// Result containing the new provider instance or an error
    pub async fn new(multicall_addr: &str, provider: Arc<T>) -> eyre::Result<Self> {
        // Parse the multicall contract address
        let multicall_address = Address::from_hex(multicall_addr)
            .map_err(|e| eyre::eyre!("Invalid multicall address '{}': {}", multicall_addr, e))?;

        // Create the multicall contract instance
        let multicall_contract = Arc::new(Multicall3Instance::new(multicall_address, provider));
        let multicall = Multicall::new(multicall_contract);

        Ok(GardenOnChainOrderProvider { multicall })
    }

    /// Executes the multicall and processes the results
    ///
    /// # Arguments
    /// * `calls` - Slice of Call3Value structs
    /// * `total_requests` - Total number of original requests
    ///
    /// # Returns
    /// Result containing vector of multicall results
    async fn execute_multicall(&self, calls: &[Call3Value]) -> eyre::Result<Vec<MulticallResult>> {
        // Initialize all results as failed
        let default_result = MulticallResult {
            success: false,
            return_data: Bytes::default(),
        };
        // Execute multicall if we have any valid requests
        if !calls.is_empty() {
            let multicall_results = self
                .multicall
                .call(calls)
                .await
                .map_err(|e| eyre::eyre!("Multicall execution failed: {}", e))?;
            Ok(multicall_results)
        } else {
            Ok(vec![default_result; calls.len()])
        }
    }

    /// Processes a single batch of swap requests
    async fn process_batch(
        &self,
        batch: &[OnchainRequest],
    ) -> eyre::Result<Vec<Option<OnChainOrder>>> {
        // Build multicall requests for the batch
        let calls = GardenOnChainOrderProvider::<T>::build_multicall_requests(
            &batch,
            self.multicall.multicall_contract.provider().clone(),
        )?;

        // Execute multicall and process results
        let multicall_results = self.execute_multicall(calls.as_slice()).await?;
        Ok(GardenOnChainOrderProvider::<T>::process_multicall_results(
            multicall_results.as_slice(),
        ))
    }
    /// Builds multicall requests for all valid swap requests
    ///
    /// # Arguments
    /// * `swap_reqs` - Slice of on-chain swap requests
    ///
    /// # Returns
    /// Vector of Call3Value structs
    fn build_multicall_requests(
        swap_reqs: &[OnchainRequest],
        provider: Arc<impl Provider>,
    ) -> eyre::Result<Vec<Call3Value>> {
        let mut calls = Vec::with_capacity(swap_reqs.len());

        // Cache contracts to avoid recreating them for the same address
        let mut contract_cache: HashMap<Address, _> = HashMap::new();

        // Process each swap request
        for request in swap_reqs.iter() {
            // Parse the swap ID
            let swap_id = request.parse_swap_id()?;

            // Get or create contract instance for this address and get the orders call
            let orders_call = contract_cache
                .entry(request.contract)
                .or_insert_with(|| {
                    Arc::new(GardenHTLCInstance::new(request.contract, provider.clone()))
                })
                .orders(swap_id);

            let call_data = orders_call.calldata();
            // Add the call to the batch
            calls.push(Call3Value {
                target: request.contract,
                allowFailure: true,
                value: U256::ZERO,
                callData: call_data.clone(),
            });
        }

        Ok(calls)
    }

    /// Converts multicall results to OnChainOrder objects
    ///
    /// # Arguments
    /// * `multicall_results` - Results from the multicall execution
    ///
    /// # Returns
    /// Vector of optional OnChainOrder objects
    fn process_multicall_results(
        multicall_results: &[MulticallResult],
    ) -> Vec<Option<OnChainOrder>> {
        multicall_results
            .into_iter()
            .map(|result| {
                if result.success {
                    match OnChainOrder::try_from(result.return_data.as_ref()) {
                        Ok(order) => Some(order),
                        Err(e) => {
                            // Log the error but continue processing other results
                            error!("Failed to decode order from return data: {}", e);
                            None
                        }
                    }
                } else {
                    None
                }
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl<T: Provider> OnChainOrders for GardenOnChainOrderProvider<T> {
    /// Retrieves multiple orders from on-chain contracts using multicall
    ///
    /// This method batches multiple contract calls into a single multicall transaction
    ///
    /// # Arguments
    /// * `swap_reqs` - slice of swap requests containing contract addresses and swap IDs
    ///
    /// # Returns
    /// Result containing a vector of optional OnChainOrder objects. Each element
    /// corresponds to the request at the same index - None if the order failed to fetch
    /// or couldn't be decoded.
    /// Processes on-chain orders for a given set of swap requests in batches.
    async fn get_orders(
        &self,
        swap_requests: &[OnchainRequest],
    ) -> eyre::Result<Vec<Option<OnChainOrder>>> {
        // Return empty vector for empty input
        if swap_requests.is_empty() {
            return Ok(Vec::new());
        }

        const BATCH_SIZE: usize = 500;

        // Split requests into batches and process them concurrently
        let batch_futures = swap_requests
            .chunks(BATCH_SIZE)
            .map(|batch| self.process_batch(batch))
            .collect::<Vec<_>>();

        // Execute all batches concurrently and preserve order
        let batch_results = try_join_all(batch_futures).await?;

        // Flatten results while maintaining order
        Ok(batch_results.into_iter().flatten().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::{
        providers::{
            Identity, ProviderBuilder, RootProvider,
            fillers::{
                BlobGasFiller, ChainIdFiller, FillProvider, GasFiller, JoinFill, NonceFiller,
            },
        },
        transports::http::reqwest::Url,
    };
    use sqlx::types::BigDecimal;
    use std::str::FromStr;
    use tracing::info;

    type AlloyProvider = FillProvider<
        JoinFill<
            Identity,
            JoinFill<GasFiller, JoinFill<BlobGasFiller, JoinFill<NonceFiller, ChainIdFiller>>>,
        >,
        RootProvider,
    >;

    const SEPOLIA_WBTC_HTLC: &str = "0x9648B9d01242F537301b98EC0Bf8b6854cDB97E6";
    const SEPOLIA_MULTICALL: &str = "0xcA11bde05977b3631167028862bE2a173976CA11";
    const SEPOLIA_RPC_URL: &str = "https://arbitrum-sepolia-rpc.publicnode.com";

    fn build_multicall_reqs(swap_ids: &[String], contract_address: Address) -> Vec<OnchainRequest> {
        swap_ids
            .iter()
            .map(|swap_id| OnchainRequest {
                swap_id: swap_id.clone(),
                contract: contract_address,
            })
            .collect()
    }

    async fn call_multicall(reqs: &[OnchainRequest]) -> eyre::Result<Vec<Option<OnChainOrder>>> {
        info!("Calling multicall with {} requests", reqs.len());
        let provider = ProviderBuilder::new().connect_http(Url::parse(SEPOLIA_RPC_URL).unwrap());
        let multicall =
            GardenOnChainOrderProvider::new(SEPOLIA_MULTICALL, Arc::new(provider)).await?;
        multicall.get_orders(reqs).await
    }

    fn assert_orders_fulfilled(results: &[OnChainOrder]) {
        for result in results {
            assert!(
                result.fulfilled_at > BigDecimal::from(0),
                "Expected fulfilled order, got: {:?}",
                result
            );
        }
    }

    fn assert_orders_unfulfilled(results: &[OnChainOrder]) {
        for result in results {
            assert!(
                result.fulfilled_at == BigDecimal::from(0),
                "Expected unfulfilled order, got: {:?}",
                result
            );
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_single_redeem_order_fulfilled() {
        _ = tracing_subscriber::fmt::try_init();
        let redeemed_oid =
            String::from("73c0edcae53e567171ebe43648ab7bfa4fa142da73edf24f1c4ccc8869d18943");
        let htlc_addr = Address::from_hex(SEPOLIA_WBTC_HTLC).unwrap();
        let reqs = build_multicall_reqs(&[redeemed_oid], htlc_addr);
        let results = call_multicall(reqs.as_slice()).await.unwrap();

        assert_eq!(results.len(), 1);
        let order = results[0].clone().expect("Expected order to be present");
        assert_orders_fulfilled(&[order]);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_multiple_redeem_orders_fulfilled() {
        _ = tracing_subscriber::fmt::try_init();
        let oids = vec![
            "73c0edcae53e567171ebe43648ab7bfa4fa142da73edf24f1c4ccc8869d18943",
            "7428e0e86120a08ac388dc63de3d70deb25096d9379914de88051f93efc430cc",
            "cd41571a80d7ef6dd3cd57494edb2e3b37026f12d2eefa79a10164c61ccf4ddc",
            "ee86236647d122e967e27e1f0b2c09f65707cd34eac6ba0c5f4dcf4731619168",
            "d26e707dcadae3be2059f2e6b99d2d9c9d26bc684d51c2e1de818addddde7f76",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let htlc_addr = Address::from_hex(SEPOLIA_WBTC_HTLC).unwrap();
        let reqs = build_multicall_reqs(&oids, htlc_addr);
        let results = call_multicall(reqs.as_slice()).await.unwrap();

        assert_eq!(results.len(), 5);
        let orders: Vec<OnChainOrder> = results.into_iter().filter_map(|r| r).collect();
        assert!(!orders.is_empty(), "Expected at least one fulfilled order");
        assert_orders_fulfilled(&orders);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_init_only_orders_unfulfilled() {
        _ = tracing_subscriber::fmt::try_init();
        let swap_ids = vec![
            "0xd50bc12e90c0168a631375c89e2e8a3697f6547799c7ccd26f9210ec487b69c9",
            "0x66736a43d6e9183678e77feeb3b027b769d4b3c2b2b32d7fce1df05eeac9203b",
            "0x8aa5eee5bc7b62a21acf47bca6d29af1bad64b5aa58b67d4e6e0595fc69f9e57",
            "0xee7e4e9b833e5d7fe8c8bc9d7f774e0f0931f431903888abe86c688acc3ee469",
            "0x672237e38a52e283ea307b6a1361b0ca706d8cb4bed7eb8731b9517c74bfccc9",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();

        let htlc_addr = Address::from_hex(SEPOLIA_WBTC_HTLC).unwrap();
        let reqs = build_multicall_reqs(&swap_ids, htlc_addr);
        let results = call_multicall(reqs.as_slice()).await.unwrap();

        assert_eq!(results.len(), 5);
        let orders: Vec<OnChainOrder> = results.into_iter().filter_map(|r| r).collect();
        assert_orders_unfulfilled(&orders);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_invalid_order_uninitiated() {
        _ = tracing_subscriber::fmt::try_init();
        let swap_ids =
            vec!["afb6d977d57696bf6b52deb8787adaf71516ba4c460cc1cb0e3a4d0b16d955fc".to_string()];
        let htlc_addr = Address::from_hex(SEPOLIA_WBTC_HTLC).unwrap();
        let reqs = build_multicall_reqs(&swap_ids, htlc_addr);
        let results = call_multicall(reqs.as_slice()).await.unwrap();

        assert_eq!(results.len(), 1);
        let order = results[0].clone().expect("Expected order to be present");
        assert!(order.is_empty(), "Expected uninitiated order");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_build_multicall_requests() {
        _ = tracing_subscriber::fmt::try_init();
        let provider = ProviderBuilder::new().connect_http(Url::parse(SEPOLIA_RPC_URL).unwrap());
        let swap_ids =
            vec!["afb6d977d57696bf6b52deb8787adaf71516ba4c460cc1cb0e3a4d0b16d955fc".to_string()];
        let onchain_reqs =
            build_multicall_reqs(&swap_ids, Address::from_hex(SEPOLIA_WBTC_HTLC).unwrap());

        let reqs = GardenOnChainOrderProvider::<AlloyProvider>::build_multicall_requests(
            &onchain_reqs,
            Arc::new(provider),
        )
        .expect("Failed to build multicall requests");

        for req in reqs {
            assert_eq!(req.target, Address::from_str(SEPOLIA_WBTC_HTLC).unwrap());
            assert!(!req.callData.is_empty());
            assert!(req.allowFailure);
            assert_eq!(req.value, U256::ZERO);
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn test_process_multicall_results() {
        _ = tracing_subscriber::fmt::try_init();
        let oids = vec![
            "73c0edcae53e567171ebe43648ab7bfa4fa142da73edf24f1c4ccc8869d18943",
            "7428e0e86120a08ac388dc63de3d70deb25096d9379914de88051f93efc430cc",
            "cd41571a80d7ef6dd3cd57494edb2e3b37026f12d2eefa79a10164c61ccf4ddc",
            "ee86236647d122e967e27e1f0b2c09f65707cd34eac6ba0c5f4dcf4731619168",
            "d26e707dcadae3be2059f2e6b99d2d9c9d26bc684d51c2e1de818addddde7f76",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let htlc_addr = Address::from_hex(SEPOLIA_WBTC_HTLC).unwrap();
        let reqs = build_multicall_reqs(&oids, htlc_addr);
        let provider = ProviderBuilder::new().connect_http(Url::parse(SEPOLIA_RPC_URL).unwrap());
        let arc_provider = std::sync::Arc::new(provider.clone());
        let multicall = GardenOnChainOrderProvider::new(SEPOLIA_MULTICALL, arc_provider.clone())
            .await
            .expect("Failed to create multicall");
        let calls = GardenOnChainOrderProvider::<AlloyProvider>::build_multicall_requests(
            &reqs,
            arc_provider.clone(),
        )
        .unwrap();
        let execute_res = multicall.execute_multicall(calls.as_slice()).await.unwrap();

        let test_results = GardenOnChainOrderProvider::<AlloyProvider>::process_multicall_results(
            execute_res.as_slice(),
        );
        let expected_results = vec![
            OnChainOrder {
                initiator: "0x3a7d1A751b69c5D617e35Cc7945813f6795760F8".to_string(),
                redeemer: "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075".to_string(),
                initiated_at: BigDecimal::from(187516060),
                timelock: BigDecimal::from(432000),
                amount: BigDecimal::from(10000),
                fulfilled_at: BigDecimal::from(187516115),
            },
            OnChainOrder {
                initiator: "0x3a7d1A751b69c5D617e35Cc7945813f6795760F8".to_string(),
                redeemer: "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075".to_string(),
                initiated_at: BigDecimal::from(187515827),
                timelock: BigDecimal::from(432000),
                amount: BigDecimal::from(10000),
                fulfilled_at: BigDecimal::from(187515887),
            },
            OnChainOrder {
                initiator: "0x3a7d1A751b69c5D617e35Cc7945813f6795760F8".to_string(),
                redeemer: "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075".to_string(),
                initiated_at: BigDecimal::from(187515756),
                timelock: BigDecimal::from(432000),
                amount: BigDecimal::from(10000),
                fulfilled_at: BigDecimal::from(187515809),
            },
            OnChainOrder {
                initiator: "0x3a7d1A751b69c5D617e35Cc7945813f6795760F8".to_string(),
                redeemer: "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075".to_string(),
                initiated_at: BigDecimal::from(187515676),
                timelock: BigDecimal::from(432000),
                amount: BigDecimal::from(10000),
                fulfilled_at: BigDecimal::from(187515735),
            },
            OnChainOrder {
                initiator: "0x3a7d1A751b69c5D617e35Cc7945813f6795760F8".to_string(),
                redeemer: "0x29f72597ca8a21F9D925AE9527ec5639bAFD5075".to_string(),
                initiated_at: BigDecimal::from(187512360),
                timelock: BigDecimal::from(432000),
                amount: BigDecimal::from(10000),
                fulfilled_at: BigDecimal::from(187512430),
            },
        ];

        for (i, result) in test_results.iter().enumerate() {
            assert_eq!(result.clone().unwrap(), expected_results[i]);
        }
    }
}
