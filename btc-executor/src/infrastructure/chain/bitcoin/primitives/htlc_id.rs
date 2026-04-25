//! HTLC ID computation for Bitcoin.
//!
//! Produces a deterministic on-chain identifier for a Bitcoin HTLC by
//! deriving the Taproot address from the HTLC parameters.

use super::{error::BitcoinPrimitivesError, htlc::HTLCParams};

/// Computes the on-chain HTLC identifier for a Bitcoin HTLC.
///
/// The identifier is the Taproot address string derived from the HTLC
/// parameters and the given network.
pub fn compute_bitcoin_htlc_id(
    params: &HTLCParams,
    network: bitcoin::Network,
) -> Result<String, BitcoinPrimitivesError> {
    let address = super::htlc::get_htlc_address(params, network)?;
    Ok(address.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{Network, XOnlyPublicKey};
    use std::str::FromStr;

    #[test]
    fn htlc_id_is_address_string() {
        let initiator_pubkey = XOnlyPublicKey::from_str(
            "c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66f",
        )
        .unwrap();
        let redeemer_pubkey = XOnlyPublicKey::from_str(
            "1db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fb",
        )
        .unwrap();
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(
            &hex::decode("c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6")
                .unwrap(),
        );

        let params = HTLCParams {
            initiator_pubkey,
            redeemer_pubkey,
            amount: 50_000,
            secret_hash,
            timelock: 144,
        };

        let id = compute_bitcoin_htlc_id(&params, Network::Regtest).unwrap();
        assert!(
            id.starts_with("bcrt1p"),
            "HTLC ID for regtest must be a bcrt1p address, got: {}",
            id
        );
    }

    #[test]
    fn htlc_id_is_deterministic() {
        let initiator_pubkey = XOnlyPublicKey::from_str(
            "c803373989fdde1177323bb21d5df89c12c0207e7f0e38dd6f0287ba6e43a66f",
        )
        .unwrap();
        let redeemer_pubkey = XOnlyPublicKey::from_str(
            "1db36714896afaee20c2cc817d170689870858b5204d3b5a94d217654e94b2fb",
        )
        .unwrap();
        let mut secret_hash = [0u8; 32];
        secret_hash.copy_from_slice(
            &hex::decode("c2da702654a5f5b14d5a969bd489da62282b7fdf12b0e8e13be5f110222b60c6")
                .unwrap(),
        );

        let params = HTLCParams {
            initiator_pubkey,
            redeemer_pubkey,
            amount: 50_000,
            secret_hash,
            timelock: 144,
        };

        let id1 = compute_bitcoin_htlc_id(&params, Network::Regtest).unwrap();
        let id2 = compute_bitcoin_htlc_id(&params, Network::Regtest).unwrap();
        assert_eq!(id1, id2, "Same params must produce the same HTLC ID");
    }
}
