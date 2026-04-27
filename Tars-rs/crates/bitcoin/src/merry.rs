use crate::ArcIndexer;
use bitcoin::Address;
use eyre::{bail, Context, Result};
use std::{process::Command, time::Duration};

/// Timeout in milliseconds between transaction confirmation checks
const TX_CONFIRMATION_TIMEOUT_MS: u64 = 500;

/// Maximum number of retries when checking for transaction confirmation
const TX_CONFIRMATION_MAX_RETRIES: u64 = 20;

/// Funds a Bitcoin address using the merry faucet and waits for the transaction to be confirmed
///
/// This function:
/// 1. Calls the merry faucet to send funds to the specified address
/// 2. Polls the indexer until UTXOs appear at the address
///
/// # Arguments
/// * `address` - The Bitcoin address to fund
/// * `indexer` - Bitcoin indexer client to check for UTXOs
///
/// # Returns
/// * `Err` if the faucet command fails or indexer encounters an error
pub async fn fund_btc(address: &Address, indexer: &ArcIndexer) -> Result<()> {
    let output = Command::new("merry")
        .args(["faucet", "--to", &address.to_string()])
        .output()
        .context("Failed to execute merry faucet command")?;

    if !output.status.success() {
        bail!(
            "Failed to fund address: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Capture the initial UTXO count before funding
    let initial_utxos_len = indexer.get_utxos(address).await?.len();

    // Waits for the address to receive UTXOs
    let mut retries = 0;
    while retries < TX_CONFIRMATION_MAX_RETRIES {
        let current_utxos = indexer.get_utxos(address).await?;
        if current_utxos.len() > initial_utxos_len {
            return Ok(());
        }

        tokio::time::sleep(Duration::from_millis(TX_CONFIRMATION_TIMEOUT_MS)).await;
        retries += 1;
    }

    bail!("Failed to confirm transaction for address: {}", address);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{generate_bitcoin_random_keypair, get_test_bitcoin_indexer};
    use bitcoin::{key::Secp256k1, Network};

    #[tokio::test]
    async fn test_fund_btc() -> Result<()> {
        let secp = Secp256k1::new();

        let indexer = get_test_bitcoin_indexer()?;

        let key_pair = generate_bitcoin_random_keypair();

        let pubkey = key_pair.public_key().x_only_public_key().0;

        let address = Address::p2tr(&secp, pubkey, None, Network::Regtest);

        let result = fund_btc(&address, &indexer).await;

        assert!(result.is_ok());

        let utxos = indexer.get_utxos(&address).await?;

        assert!(!utxos.is_empty());

        Ok(())
    }
}
