/*
#[cfg(test)]
pub mod test_utils {
    use alloy::{
        hex::FromHex,
        primitives::Address,
        providers::{Provider, ext::AnvilApi},
        signers::local::PrivateKeySigner,
    };
    use tars::{
        evm::{
            ERC20::ERC20Instance,
            ERC20Contract,
            GardenHTLCv3::GardenHTLCv3Instance,
            GardenHTLCv3Contract, Multicall3Contract,
            htlc::GardenHTLC,
            primitives::AlloyProvider,
            test_utils::{self, Network, fund, multicall_contract, order_id},
        },
        orderbook::primitives::EVMSwap,
        primitives::HTLCVersion,
    };
    use std::sync::Arc;
    use tracing::info;

    pub const MAX_BLOCK_SPAN: u64 = 1000;
    pub const CONTRACT_ADDR: &str = "0x9E545E3C0baAB3E08CdfD552C960A1050f373042";
    pub const V3_CONTRACT_ADDR: &str = "0x6d49021ebF8172F4B51A52a621C7Fc94BD8364cF";
    pub const TEST_RPC_URL: &str = "http://localhost:8545";

    type V3Contracts = (
        GardenHTLC,
        GardenHTLCv3Instance<AlloyProvider>,
        ERC20Instance<AlloyProvider>,
        PrivateKeySigner,
    );

    pub async fn htlc_contract_v3(
        provider: AlloyProvider,
    ) -> (GardenHTLCv3Contract, ERC20Contract, Multicall3Contract) {
        let htlc = GardenHTLCv3Contract::new(
            Address::from_hex(V3_CONTRACT_ADDR).unwrap(),
            provider.clone(),
        );

        let token_address = htlc
            .token()
            .call()
            .await
            .expect("Failed to get token address");

        info!("Token address: {:?}", token_address);

        let erc20 = ERC20Contract::new(token_address, provider.clone());
        let multicall = multicall_contract(provider, Network::Ethereum);

        (htlc, erc20, multicall)
    }

    async fn setup_contracts_common() -> (PrivateKeySigner, AlloyProvider) {
        let signer = PrivateKeySigner::random();
        fund(signer.address().to_string());
        let wallet = alloy::network::EthereumWallet::from(signer.clone());
        let provider = test_utils::ethereum_provider(Some(wallet));
        (signer, provider)
    }

    pub async fn setup_contracts() -> V3Contracts {
        let (signer, provider) = setup_contracts_common().await;
        let (htlc_contract, erc20_contract, multicall3) = htlc_contract_v3(provider.clone()).await;
        (
            GardenHTLC::new(Arc::new(multicall3), signer.clone()).unwrap(),
            htlc_contract,
            erc20_contract,
            signer,
        )
    }

    // Helper function to create and initiate a swap with common logic
    async fn create_initiated_swap_common(
        chain_htlc: &GardenHTLC,
        provider: &AlloyProvider,
        htlc_contract_address: &Address,
        initiator: &PrivateKeySigner,
        version: HTLCVersion,
    ) -> (EVMSwap, String) {
        let chain_id = provider.get_chain_id().await.unwrap();
        let (swap, _) = test_utils::new_swap(
            initiator.address(),
            chain_id,
            *htlc_contract_address,
            version.clone(),
        );

        info!("swap: {:?}", swap);
        info!("htlc contract address: {:?}", htlc_contract_address);

        let tx_id = chain_htlc
            .initiate(&swap, htlc_contract_address)
            .await
            .unwrap();
        info!("Initiated swap transaction ID: {:?}", tx_id);

        provider.anvil_mine(Some(1), Some(1)).await.unwrap();

        let oid = order_id(
            version,
            chain_id,
            &swap.secret_hash,
            &swap.initiator,
            &swap.redeemer,
            &swap.amount,
            &swap.timelock,
            htlc_contract_address,
        );
        info!("Order ID: {:?}", oid);

        (swap, oid.to_string())
    }

    pub async fn create_initiated_swap(
        chain_htlc: &GardenHTLC,
        provider: &AlloyProvider,
        htlc_contract: &GardenHTLCv3Instance<AlloyProvider>,
        initiator: &PrivateKeySigner,
    ) -> (EVMSwap, String) {
        create_initiated_swap_common(
            chain_htlc,
            provider,
            htlc_contract.address(),
            initiator,
            HTLCVersion::V3,
        )
        .await
    }

    // Helper function to create and redeem a swap with common logic
    async fn create_redeemed_swap_common(
        chain_htlc: &GardenHTLC,
        provider: &AlloyProvider,
        htlc_contract_address: &Address,
        initiator: &PrivateKeySigner,
        version: HTLCVersion,
    ) -> (EVMSwap, String) {
        let chain_id = provider.get_chain_id().await.unwrap();
        let (swap, secret) = test_utils::new_swap(
            initiator.address(),
            chain_id,
            *htlc_contract_address,
            version.clone(),
        );

        chain_htlc
            .initiate(&swap, htlc_contract_address)
            .await
            .unwrap();

        provider.anvil_mine(Some(1), Some(1)).await.unwrap();

        chain_htlc
            .redeem(&swap, &secret, htlc_contract_address)
            .await
            .unwrap();

        provider.anvil_mine(Some(1), Some(1)).await.unwrap();

        let oid = order_id(
            version,
            chain_id,
            &swap.secret_hash,
            &swap.initiator,
            &swap.redeemer,
            &swap.amount,
            &swap.timelock,
            htlc_contract_address,
        );

        (swap, oid.to_string())
    }

    pub async fn create_redeemed_swap(
        chain_htlc: &GardenHTLC,
        provider: &AlloyProvider,
        htlc_contract: &GardenHTLCv3Instance<AlloyProvider>,
        initiator: &PrivateKeySigner,
    ) -> (EVMSwap, String) {
        create_redeemed_swap_common(
            chain_htlc,
            provider,
            htlc_contract.address(),
            initiator,
            HTLCVersion::V3,
        )
        .await
    }

    // Helper function to create and refund a swap with common logic
    async fn create_refunded_swap_common(
        chain_htlc: &GardenHTLC,
        provider: &AlloyProvider,
        htlc_contract_address: &Address,
        initiator: &PrivateKeySigner,
        version: HTLCVersion,
    ) -> (EVMSwap, String) {
        let chain_id = provider.get_chain_id().await.unwrap();
        let (swap, _) = test_utils::new_swap(
            initiator.address(),
            chain_id,
            *htlc_contract_address,
            version.clone(),
        );

        chain_htlc
            .initiate(&swap, htlc_contract_address)
            .await
            .unwrap();

        provider.anvil_mine(Some(1), Some(1)).await.unwrap();

        let timelock: u64 = swap.timelock.to();
        // Fast forward time to make refund possible
        provider
            .anvil_mine(Some(timelock + 1), Some(1))
            .await
            .unwrap();

        chain_htlc
            .refund(&swap, htlc_contract_address)
            .await
            .unwrap();

        provider.anvil_mine(Some(1), Some(1)).await.unwrap();

        let oid = order_id(
            version.clone(),
            chain_id,
            &swap.secret_hash,
            &swap.initiator,
            &swap.redeemer,
            &swap.amount,
            &swap.timelock,
            htlc_contract_address,
        );

        (swap, oid.to_string())
    }

    pub async fn create_refunded_swap(
        chain_htlc: &GardenHTLC,
        provider: &AlloyProvider,
        htlc_contract: &GardenHTLCv3Instance<AlloyProvider>,
        initiator: &PrivateKeySigner,
    ) -> (EVMSwap, String) {
        create_refunded_swap_common(
            chain_htlc,
            provider,
            htlc_contract.address(),
            initiator,
            HTLCVersion::V3,
        )
        .await
    }
}
 */
