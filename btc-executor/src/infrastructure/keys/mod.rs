use bitcoin::key::TapTweak;
use std::fmt;

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("invalid Bitcoin private key: {0}")]
    InvalidBtcKey(String),

    #[error("wallet initialization failed: {0}")]
    WalletInit(String),

    #[error("signing failed: {0}")]
    Signing(String),
}

pub struct BitcoinWallet {
    keypair: bitcoin::secp256k1::Keypair,
    x_only_pubkey: bitcoin::key::XOnlyPublicKey,
    address: bitcoin::Address,
    network: bitcoin::Network,
}

impl BitcoinWallet {
    pub fn from_private_key(hex_key: &str, network: bitcoin::Network) -> Result<Self, KeyError> {
        let key_bytes = hex::decode(hex_key.trim_start_matches("0x"))
            .map_err(|e| KeyError::InvalidBtcKey(format!("hex decode: {e}")))?;
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let secret_key = bitcoin::secp256k1::SecretKey::from_slice(&key_bytes)
            .map_err(|e| KeyError::InvalidBtcKey(format!("invalid secret key: {e}")))?;
        let keypair = bitcoin::secp256k1::Keypair::from_secret_key(&secp, &secret_key);
        let (x_only_pubkey, _parity) = keypair.x_only_public_key();
        let address = bitcoin::Address::p2tr(&secp, x_only_pubkey, None, network);

        Ok(Self {
            keypair,
            x_only_pubkey,
            address,
            network,
        })
    }

    pub fn keypair(&self) -> &bitcoin::secp256k1::Keypair {
        &self.keypair
    }

    pub fn x_only_pubkey(&self) -> &bitcoin::key::XOnlyPublicKey {
        &self.x_only_pubkey
    }

    pub fn address(&self) -> &bitcoin::Address {
        &self.address
    }

    pub fn network(&self) -> bitcoin::Network {
        self.network
    }

    pub fn sign_taproot_script_spend(
        &self,
        msg: &[u8; 32],
        sighash_type: bitcoin::TapSighashType,
    ) -> Result<bitcoin::taproot::Signature, KeyError> {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let message = bitcoin::secp256k1::Message::from_digest(*msg);
        let signature = secp.sign_schnorr_no_aux_rand(&message, &self.keypair);
        Ok(bitcoin::taproot::Signature {
            signature,
            sighash_type,
        })
    }

    pub fn sign_taproot_key_spend(
        &self,
        msg: &[u8; 32],
        sighash_type: bitcoin::TapSighashType,
    ) -> Result<bitcoin::taproot::Signature, KeyError> {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let message = bitcoin::secp256k1::Message::from_digest(*msg);
        let tweaked_keypair = self.keypair.tap_tweak(&secp, None);
        let signature = secp.sign_schnorr_no_aux_rand(&message, tweaked_keypair.as_keypair());
        Ok(bitcoin::taproot::Signature {
            signature,
            sighash_type,
        })
    }

    pub fn sign_schnorr(
        &self,
        msg: &[u8; 32],
    ) -> Result<bitcoin::secp256k1::schnorr::Signature, KeyError> {
        Ok(self
            .sign_taproot_script_spend(msg, bitcoin::TapSighashType::Default)?
            .signature)
    }
}

impl fmt::Debug for BitcoinWallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BitcoinWallet")
            .field("address", &self.address.to_string())
            .field("network", &self.network)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_BTC_PRIVKEY_HEX: &str =
        "e8f32e723decf4051aefac8e2c93c9c5b214313817cdb01a1494b917c8436b35";

    #[test]
    fn btc_wallet_from_valid_hex() {
        let wallet =
            BitcoinWallet::from_private_key(TEST_BTC_PRIVKEY_HEX, bitcoin::Network::Regtest)
                .expect("should create wallet from valid hex key");

        assert!(!wallet.address().to_string().is_empty());
        assert_eq!(wallet.network(), bitcoin::Network::Regtest);
        assert_eq!(wallet.x_only_pubkey().serialize().len(), 32);
    }
}
