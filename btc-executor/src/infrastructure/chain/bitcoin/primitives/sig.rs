//! Signature application for Bitcoin HTLC witness stacks.
//!
//! Adapted from garden-rs batcher `sign.rs`. Replaces the zero-byte
//! placeholder in a witness with an actual Schnorr signature.

use super::error::BitcoinPrimitivesError;
use crate::infrastructure::keys::BitcoinWallet;
use bitcoin::{TapSighashType, Witness};

/// Size of a Schnorr signature placeholder (65 zero bytes: 64 sig + 1 sighash type).
const SCHNORR_PLACEHOLDER_LEN: usize = 65;

/// Replaces the first 64-byte zero placeholder in the given witness with a
/// real Schnorr signature computed from the provided keypair and sighash.
///
/// # Arguments
/// * `witness` - Mutable reference to a witness containing a placeholder
/// * `wallet` - The signing wallet
/// * `sighash` - The 32-byte sighash to sign
/// * `sighash_type` - The sighash type to encode with the signature
///
/// # Returns
/// The mutated witness with the placeholder replaced by a real signature.
///
/// # Errors
/// * `BitcoinPrimitivesError::Sighash` if the sighash bytes are invalid.
/// * `BitcoinPrimitivesError::InvalidParam` if no placeholder is found.
pub fn add_signature_to_witness(
    witness: &mut Witness,
    wallet: &BitcoinWallet,
    sighash: &[u8; 32],
    sighash_type: TapSighashType,
) -> Result<(), BitcoinPrimitivesError> {
    let tap_sig = wallet
        .sign_taproot_script_spend(sighash, sighash_type)
        .map_err(|e| BitcoinPrimitivesError::Signing(e.to_string()))?;
    let sig_bytes = tap_sig.serialize();

    // Find and replace the first placeholder (64 zero bytes) in the witness
    let placeholder = [0u8; SCHNORR_PLACEHOLDER_LEN];
    let mut elements: Vec<Vec<u8>> = (0..witness.len())
        .map(|i| witness.nth(i).unwrap_or_default().to_vec())
        .collect();

    let mut replaced = false;
    for elem in &mut elements {
        if elem.len() == SCHNORR_PLACEHOLDER_LEN && elem.as_slice() == &placeholder[..] {
            *elem = sig_bytes.to_vec();
            replaced = true;
            break;
        }
    }

    if !replaced {
        return Err(BitcoinPrimitivesError::InvalidParam(
            "No signature placeholder found in witness".to_string(),
        ));
    }

    // Rebuild witness from elements
    let mut new_witness = Witness::new();
    for elem in elements {
        new_witness.push(&elem);
    }
    *witness = new_witness;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::keys::BitcoinWallet;

    const TEST_BTC_PRIVKEY_HEX: &str =
        "e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35";

    #[test]
    fn replaces_placeholder_with_signature() {
        let wallet =
            BitcoinWallet::from_private_key(TEST_BTC_PRIVKEY_HEX, bitcoin::Network::Regtest)
                .expect("wallet");
        let sighash = [0xab_u8; 32];

        // Build a witness with a placeholder
        let mut witness = Witness::new();
        witness.push([0u8; 65]); // placeholder (64 sig + 1 sighash type)
        witness.push([0xcc_u8; 32]); // some other data

        assert_eq!(witness.len(), 2);

        add_signature_to_witness(&mut witness, &wallet, &sighash, TapSighashType::All).unwrap();

        // First element should no longer be all zeros
        let first = witness.nth(0).unwrap();
        // TapSighashType::All appends 1 byte => 65 bytes total
        assert_eq!(
            first.len(),
            65,
            "Signature with sighash type should be 65 bytes"
        );
        assert!(
            !first.iter().all(|&b| b == 0),
            "First element must no longer be all zeros after signing"
        );

        // Second element should be unchanged
        let second = witness.nth(1).unwrap();
        assert_eq!(second, &[0xcc_u8; 32]);
    }

    #[test]
    fn errors_when_no_placeholder() {
        let wallet =
            BitcoinWallet::from_private_key(TEST_BTC_PRIVKEY_HEX, bitcoin::Network::Regtest)
                .expect("wallet");
        let sighash = [0xab_u8; 32];

        let mut witness = Witness::new();
        witness.push([0xff_u8; 65]); // not a placeholder (not zeros)

        let result = add_signature_to_witness(&mut witness, &wallet, &sighash, TapSighashType::All);
        assert!(result.is_err(), "Must error when no placeholder found");
    }
}
