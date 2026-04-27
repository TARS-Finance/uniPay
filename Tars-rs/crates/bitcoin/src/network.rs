use bitcoin::{Address, Network};
use eyre::{eyre, Context, Result};
use orderbook::primitives::MatchedOrderVerbose;
use std::str::FromStr;

/// Supported Bitcoin networks
const BITCOIN_MAINNET: &str = "bitcoin";
const BITCOIN_TESTNET: &str = "bitcoin_testnet";
const BITCOIN_REGTEST: &str = "bitcoin_regtest";

/// Get the Bitcoin network for this swap
///
/// # Arguments
/// * `chain` - The chain identifier string: "bitcoin", "bitcoin_testnet", or "bitcoin_regtest"
///
/// # Returns
/// * `Ok(Network)` with the Bitcoin network type
/// * `Err` if the chain is not a valid Bitcoin network
pub fn get_bitcoin_network(chain: &str) -> Result<Network> {
    match chain.to_lowercase().as_str() {
        BITCOIN_MAINNET => Ok(Network::Bitcoin),
        BITCOIN_REGTEST => Ok(Network::Regtest),
        BITCOIN_TESTNET => Ok(Network::Testnet),
        _ => Err(eyre::eyre!(
            "Expected one of the following networks: {}, {}, {}",
            BITCOIN_MAINNET,
            BITCOIN_TESTNET,
            BITCOIN_REGTEST
        )),
    }
}

/// Validates a Bitcoin address for a specific network.
///
/// # Arguments
/// * `addr` - The Bitcoin address to validate
/// * `network` - The Bitcoin network (Mainnet, Testnet, etc.)
///
/// # Returns
/// * `Ok(Address)` with the validated address
/// * `Err` if the address is invalid or doesn't match the network
pub fn validate_btc_address_for_network(addr: &str, network: Network) -> Result<Address> {
    let address = Address::from_str(addr)
        .with_context(|| format!("Invalid Bitcoin address format: {}", addr))?;

    if address.is_valid_for_network(network) {
        Ok(address.assume_checked())
    } else {
        Err(eyre::eyre!(
            "Address {} is not valid for network {:?}",
            addr,
            network
        ))
    }
}

/// Get the Bitcoin recipient address from a matched order
///
/// # Arguments
/// * `order` - The matched order
///
/// # Returns
/// * `Ok(Address)` with the Bitcoin recipient address
pub fn get_bitcoin_recipient_address(order: &MatchedOrderVerbose) -> Result<Address> {
    // Get the Bitcoin network from the order
    let network = get_bitcoin_network(&order.source_swap.chain)?;

    // Get the Bitcoin recipient address from the order
    let recipient_str = order
        .create_order
        .additional_data
        .bitcoin_optional_recipient
        .clone()
        .ok_or(eyre!("Bitcoin optional address is required"))?;

    // Validate the Bitcoin recipient address for the network
    validate_btc_address_for_network(&recipient_str, network)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::Network;
    use eyre::Result;

    #[test]
    fn test_get_bitcoin_network() -> Result<()> {
        let bitcoin = get_bitcoin_network("bitcoin")?;
        let bitcoin_testnet = get_bitcoin_network("bitcoin_testnet")?;
        let bitcoin_regtest = get_bitcoin_network("bitcoin_regtest")?;
        let invalid_network = get_bitcoin_network("ethereum");

        assert!(invalid_network.is_err());
        assert_eq!(bitcoin.to_core_arg(), "main");
        assert_eq!(bitcoin_testnet.to_core_arg(), "test");
        assert_eq!(bitcoin_regtest.to_core_arg(), "regtest");

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_btc_address_for_network() {
        // Valid address for Mainnet
        let addr = "bc1plunh8ysxvzt6emdsuj2uwth79zem0ye78annc0lqacwzjs28e0tsp5tpdv";
        let network = Network::Bitcoin;
        assert!(validate_btc_address_for_network(addr, network).is_ok());

        // Invalid address format
        let invalid_addr = "invalid_address";
        assert!(validate_btc_address_for_network(invalid_addr, network).is_err());

        // Valid address but wrong network
        let testnet_addr = "tb1plunh8ysxvzt6emdsuj2uwth79zem0ye78annc0lqacwzjs28e0tskuawhr";
        let mainnet_network = Network::Bitcoin;
        assert!(validate_btc_address_for_network(testnet_addr, mainnet_network).is_err());

        // Valid address for Testnet
        let network_testnet = Network::Testnet;
        assert!(validate_btc_address_for_network(testnet_addr, network_testnet).is_ok());
    }
}
