use alloy::providers::Provider;
use bigdecimal::BigDecimal;
use eyre::Result;
use tars::primitives::HTLCVersion;

const ARBITRUM_SEPOLIA: &str = "arbitrum_sepolia";
const ARBITRUM: &str = "arbitrum";

/// Checks if the chain is an Arbitrum chain
///
/// # Arguments
/// * `chain` - The chain to check
///
/// # Returns
/// * `bool` - True if the chain is an Arbitrum chain, false otherwise
#[inline]
pub fn is_arbitrum_chain(chain: &str) -> bool {
    chain == ARBITRUM || chain == ARBITRUM_SEPOLIA
}

// Block numbers for L1 and L2 chains
///
/// Contains the current block numbers for the configured chain.
///
/// For L2 chains like Arbitrum:
/// - `block_number` is the L2 block number (required for V2 contracts)
/// - `ancestor_block_number` is the L1 block number (required for V1 contracts)
///
/// For L1 chains like Ethereum:
/// - `block_number` is the current L1 block number
/// - `ancestor_block_number` is None
#[derive(Debug, Clone)]
pub struct BlockNumbers {
    block_number: BigDecimal,
    ancestor_block_number: Option<BigDecimal>,
}

impl BlockNumbers {
    pub fn new(block_number: BigDecimal, ancestor_block_number: Option<BigDecimal>) -> Self {
        Self {
            block_number,
            ancestor_block_number,
        }
    }

    /// Retrieves the block number for a specific HTLC version.
    ///
    /// # Arguments
    /// * `version` - The HTLC version
    /// * `is_arbitrum` - Whether the chain is an Arbitrum chain
    ///
    /// # Returns
    /// * `Option<&BigDecimal>` - The block number for the specified version, or None if not available
    pub fn get_block_for_version(
        &self,
        version: &HTLCVersion,
        is_arbitrum: bool,
    ) -> Option<&BigDecimal> {
        match (is_arbitrum, version) {
            // For non-Arbitrum chains, always use the main block number
            (false, _) => Some(&self.block_number),
            // For Arbitrum V1 contracts, use the ancestor block number (L1 block number)
            (true, HTLCVersion::V1) => self.ancestor_block_number.as_ref(),
            // For Arbitrum V2 contracts, use the block number (L2 block number)
            (true, HTLCVersion::V2) => Some(&self.block_number),
            // For Arbitrum V3 contracts, use the block number (L2 block number)
            (true, HTLCVersion::V3) => Some(&self.block_number),
        }
    }
}

/// Retrieves the current block numbers for the specified chain.
///
/// This function fetches the appropriate block numbers based on the chain type.
/// For Arbitrum chains, it retrieves both L1 and L2 block numbers, while for other
/// chains it only fetches the native block number.
///
/// # Arguments
/// * `provider` - A reference to the blockchain provider
/// * `chain_identifier` - A string slice that identifies the chain (e.g., "arbitrum-one", "ethereum")
///
/// # Behavior by Chain Type
///
/// ## For Arbitrum Chains:
/// - `block_number`: The current L2 block number (used for V2 contracts)
/// - `ancestor_block_number`: The corresponding L1 block number (used for V1 contracts)
///
/// ## For Other Chains (e.g., Ethereum):
/// - `block_number`: The current chain's block number (used for all contract versions)
/// - `ancestor_block_number`: `None` (not applicable for L1 chains)
///
/// # Returns
/// - `Some(BlockNumbers)` containing the appropriate block numbers if successful
/// - `None` if there was an error fetching either block number
///
/// # Errors
/// This function will return `None` and log an error if:
/// - It fails to fetch the current block number from the provider
/// - (For Arbitrum chains) It fails to fetch the L1 block number
pub async fn get_block_numbers(
    provider: &impl Provider,
    chain_identifier: &str,
) -> Option<BlockNumbers> {
    // Get the current chain's block number
    let block_number = match provider.get_block_number().await {
        Ok(block) => BigDecimal::from(block),
        Err(e) => {
            tracing::error!(
                chain = %chain_identifier,
                error = %e,
                "Failed to get block number"
            );
            return None;
        }
    };

    // Only get ancestor block number for Arbitrum chains
    let ancestor_block_number = if is_arbitrum_chain(&chain_identifier) {
        match get_l1_block_number(&provider).await {
            Ok(block) => Some(BigDecimal::from(block)),
            Err(e) => {
                tracing::error!(
                    error = %e,
                    "Failed to get L1 block number for Arbitrum chain"
                );
                return None;
            }
        }
    } else {
        None
    };

    Some(BlockNumbers::new(block_number, ancestor_block_number))
}

/// Fetches the L1 block number from an L2 provider.
///
/// This function is specifically designed to work with L2 chains (like Arbitrum) that provide
/// access to the L1 block number through their RPC endpoint. It makes a raw RPC call to
/// `eth_getBlockByNumber` and extracts the `l1BlockNumber` field from the response.
///
/// # Arguments
/// * `provider` - A reference to an AlloyProvider instance configured for the L2 network
///
/// # Returns
/// * `Result<u64>` - The L1 block number as a u64 if successful, or an error if:
///   - The RPC request fails
///   - The response doesn't contain the `l1BlockNumber` field
///   - The `l1BlockNumber` cannot be parsed as a hex string
///
/// # Note
/// This function should only be called when connected to an L2 provider that supports the
/// `l1BlockNumber` field in the block response. It will return an error if used with an L1 provider.
pub async fn get_l1_block_number(provider: &impl Provider) -> Result<u64> {
    let block: serde_json::Value = provider
        .raw_request(
            std::borrow::Cow::from("eth_getBlockByNumber"),
            ("latest", false),
        )
        .await?;

    let l1_block_number_hex = block["l1BlockNumber"]
        .as_str()
        .ok_or_else(|| eyre::eyre!("l1BlockNumber not found in block"))?;

    let l1_block_number = u64::from_str_radix(l1_block_number_hex.trim_start_matches("0x"), 16)
        .map_err(|e| eyre::eyre!("Failed to parse l1BlockNumber: {}", e))?;

    Ok(l1_block_number)
}

#[cfg(test)]
mod tests {
    use crate::executor::block_numbers::get_l1_block_number;
    use alloy::providers::ProviderBuilder;
    use reqwest::Url;

    // Arbitrum RPC URL which supports L1 block number
    const ARBITRUM_RPC_URL: &str = "https://arb1.arbitrum.io/rpc";

    #[tokio::test]
    async fn test_get_l1_block_number() {
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_http(Url::parse(ARBITRUM_RPC_URL).unwrap());

        let response = get_l1_block_number(&provider).await;
        assert!(response.is_ok());
        let block_number = response.unwrap();
        assert!(block_number > 0u64);
    }
}
