use std::str::FromStr;

use crate::{generate_instant_refund_hash, HTLCParams, InstantRefundSignatures, Utxo};
use alloy::hex;
use bitcoin::{
    key::{Keypair, Secp256k1},
    secp256k1::{Message, SecretKey},
    taproot, Address, Network, TapSighashType,
};
use eyre::{eyre, Result};

/// Creates a keypair from a secret key string.
///
/// # Arguments
///
/// * `secret_key_str` - The secret key string to create a keypair from.
///
/// # Returns
///
/// A `Keypair` instance.
pub fn create_keypair(secret_key_str: &str) -> Result<Keypair> {
    let secp = Secp256k1::new();
    let secret_key = SecretKey::from_str(secret_key_str)
        .map_err(|e| eyre!("Failed to parse secret key : {:#?}", e.to_string()))?;
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    Ok(keypair)
}

/// Generates the instant refund signatures for the given htlc params, utxos, recipient and network.
///
/// # Arguments
///
/// * `initiator_keypair` - The initiator keypair.
/// * `redeemer_keypair` - The redeemer keypair.
/// * `htlc_params` - The htlc params.
/// * `utxos` - The utxos.
/// * `recipient` - The recipient address.
/// * `network` - The network.
/// * `fee` - The fee.
///
/// # Returns
///
/// A `Vec<InstantRefundSignatures>` instance.
pub async fn generate_instant_refund_signatures(
    initiator_keypair: &Keypair,
    redeemer_keypair: &Keypair,
    htlc_params: &HTLCParams,
    utxos: &[Utxo],
    recipient: &Address,
    network: Network,
    fee: Option<u64>,
) -> Result<Vec<InstantRefundSignatures>> {
    let hashes = generate_instant_refund_hash(&htlc_params, &utxos, &recipient, network, fee)?;
    let mut signatures = Vec::with_capacity(hashes.len());
    let secp = Secp256k1::new();

    for hash_bytes in hashes {
        let message = Message::from_digest_slice(&hash_bytes)?;

        let initiator_sig = taproot::Signature {
            signature: secp.sign_schnorr_no_aux_rand(&message, initiator_keypair),
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
        };

        let redeemer_sig = taproot::Signature {
            signature: secp.sign_schnorr_no_aux_rand(&message, redeemer_keypair),
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
        };

        let initiator_sig_hex = hex::encode(initiator_sig.serialize());
        let redeemer_sig_hex = hex::encode(redeemer_sig.serialize());

        let instant_refund_signature = InstantRefundSignatures {
            initiator: initiator_sig_hex,
            redeemer: redeemer_sig_hex,
        };

        signatures.push(instant_refund_signature);
    }

    Ok(signatures)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        build_instant_refund_sacp, fund_btc, get_htlc_address,
        test_utils::{
            generate_bitcoin_random_keypair, get_test_bitcoin_indexer, get_test_htlc_params,
            TEST_FEE, TEST_NETWORK,
        },
    };
    use eyre::{bail, Context, Result};
    use utils::gen_secret;

    #[tokio::test]
    async fn test_build_instant_refund_sacp() -> Result<()> {
        let secp = Secp256k1::new();

        let indexer = get_test_bitcoin_indexer()?;

        let initiator_key_pair = generate_bitcoin_random_keypair();
        let redeemer_key_pair = generate_bitcoin_random_keypair();

        let initiator_pubkey = initiator_key_pair.public_key().x_only_public_key().0;
        let redeemer_pubkey = redeemer_key_pair.public_key().x_only_public_key().0;

        let (_secret, secret_hash) = gen_secret();

        let htlc_params =
            get_test_htlc_params(&initiator_pubkey, &redeemer_pubkey, secret_hash.into());

        let htlc_address = get_htlc_address(&htlc_params, Network::Regtest)
            .context("Failed to generate HTLC address")?;

        fund_btc(&htlc_address, &indexer).await?;

        let utxos = indexer.get_utxos(&htlc_address).await?;

        if utxos.is_empty() {
            bail!("No UTXOs found for the HTLC address");
        }

        let recipient = Address::p2tr(&secp, initiator_pubkey, None, Network::Regtest);

        let signatures = generate_instant_refund_signatures(
            &initiator_key_pair,
            &redeemer_key_pair,
            &htlc_params,
            &utxos,
            &recipient,
            TEST_NETWORK,
            Some(TEST_FEE),
        )
        .await
        .context("Failed to generate instant refund signatures")?;

        let tx =
            build_instant_refund_sacp(&htlc_params, &utxos, signatures, &recipient, Some(1000))
                .await
                .context("Failed to build instant refund transaction")?;

        indexer
            .submit_tx(&tx)
            .await
            .context("Failed to submit transaction to network")?;

        println!("Submitted instant refund tx: {}", tx.compute_txid());

        Ok(())
    }
}
