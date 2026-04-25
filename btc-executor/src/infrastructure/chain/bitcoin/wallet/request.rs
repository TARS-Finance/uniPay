//! Normalized request model accepted by the Bitcoin wallet runner.
//!
//! The coordinator and chain port translate higher-level intents into
//! [`WalletRequest`] values. The request carries enough information for the
//! batcher to build and sign a transaction later, while `dedupe_key` gives the
//! persistence layer an idempotent identity for enqueue/restore flows.

use bitcoin::taproot::TapLeafHash;
use bitcoin::{Address, OutPoint, ScriptBuf, Sequence, TapSighashType, Witness};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::str::FromStr;

/// Validation failures for [`WalletRequest`] construction.
#[derive(Debug, thiserror::Error)]
pub enum WalletRequestError {
    #[error("dedupe_key must not be empty")]
    EmptyDedupeKey,
    #[error("send amount must be > 0")]
    InvalidSendAmount,
    #[error("spend value must be > 0")]
    InvalidSpendValue,
    #[error("SACP spend must have a recipient")]
    MissingSacpRecipient,
}

/// Request to pay a single address directly from the wallet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendRequest {
    #[serde(
        serialize_with = "serialize_checked_address",
        deserialize_with = "deserialize_checked_address"
    )]
    /// Destination address already validated against the target network.
    pub address: Address,
    /// Amount in sats.
    pub amount: u64,
}

/// Request to spend a specific UTXO with a precomputed witness template.
///
/// This is how HTLC redeems/refunds enter the batcher. The builder treats the
/// input as mandatory and only decides how to cover fees plus whether an
/// optional paired recipient output should be emitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpendRequest {
    /// Mandatory prevout to spend.
    pub outpoint: OutPoint,
    /// Prevout value in sats.
    pub value: u64,
    /// Prevout script pubkey.
    pub script_pubkey: ScriptBuf,
    /// Witness stack with signature placeholder(s) already arranged.
    pub witness_template: Witness,
    /// Tapleaf script committed by the spend.
    pub script: ScriptBuf,
    /// Tapleaf hash used for sighash calculation.
    pub leaf_hash: TapLeafHash,
    /// Sequence chosen for locktime or RBF semantics.
    pub sequence: Sequence,
    /// Sighash type expected by the witness template.
    pub sighash_type: TapSighashType,
    /// Optional paired output. Required for SACP instant-refund spends.
    pub recipient: Option<SendRequest>,
}

/// Discriminated request kinds accepted by the wallet runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WalletRequestKind {
    Send(SendRequest),
    Spend(SpendRequest),
}

/// Idempotent wallet request persisted by the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WalletRequest {
    dedupe_key: String,
    kind: WalletRequestKind,
}

impl WalletRequest {
    /// Build a request from validated parts.
    ///
    /// Assumptions:
    /// - `dedupe_key` uniquely identifies the logical request within a wallet
    ///   scope.
    /// - send amounts and spend values are expressed in sats, not BTC decimals.
    /// - SACP spends provide a recipient because the paired output is part of
    ///   the sighash contract.
    pub fn from_parts(
        dedupe_key: impl Into<String>,
        kind: WalletRequestKind,
    ) -> Result<Self, WalletRequestError> {
        let dedupe_key = validate_dedupe_key(dedupe_key.into())?;

        match kind {
            WalletRequestKind::Send(send) => {
                if send.amount == 0 {
                    return Err(WalletRequestError::InvalidSendAmount);
                }

                Ok(Self {
                    dedupe_key,
                    kind: WalletRequestKind::Send(send),
                })
            },
            WalletRequestKind::Spend(spend) => {
                if spend.value == 0 {
                    return Err(WalletRequestError::InvalidSpendValue);
                }
                if spend.sighash_type == TapSighashType::SinglePlusAnyoneCanPay
                    && spend.recipient.is_none()
                {
                    return Err(WalletRequestError::MissingSacpRecipient);
                }

                Ok(Self {
                    dedupe_key,
                    kind: WalletRequestKind::Spend(spend),
                })
            },
        }
    }

    /// Convenience constructor for a direct send request.
    pub fn send(
        dedupe_key: impl Into<String>,
        address: Address,
        amount: u64,
    ) -> Result<Self, WalletRequestError> {
        Self::from_parts(
            dedupe_key,
            WalletRequestKind::Send(SendRequest { address, amount }),
        )
    }

    /// Convenience constructor for a mandatory-input spend request.
    #[allow(clippy::too_many_arguments)]
    pub fn spend(
        dedupe_key: impl Into<String>,
        outpoint: OutPoint,
        value: u64,
        script_pubkey: ScriptBuf,
        witness_template: Witness,
        script: ScriptBuf,
        leaf_hash: TapLeafHash,
        sequence: Sequence,
        sighash_type: TapSighashType,
        recipient: Option<SendRequest>,
    ) -> Result<Self, WalletRequestError> {
        Self::from_parts(
            dedupe_key,
            WalletRequestKind::Spend(SpendRequest {
                outpoint,
                value,
                script_pubkey,
                witness_template,
                script,
                leaf_hash,
                sequence,
                sighash_type,
                recipient,
            }),
        )
    }

    /// Stable idempotency key used by the persistence layer and submit waiters.
    pub fn dedupe_key(&self) -> &str {
        &self.dedupe_key
    }

    /// Underlying request payload consumed by the builder/runtime.
    pub fn kind(&self) -> &WalletRequestKind {
        &self.kind
    }
}

fn validate_dedupe_key(dedupe_key: String) -> Result<String, WalletRequestError> {
    if dedupe_key.trim().is_empty() {
        return Err(WalletRequestError::EmptyDedupeKey);
    }

    Ok(dedupe_key)
}

fn serialize_checked_address<S>(address: &Address, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&address.to_string())
}

fn deserialize_checked_address<'de, D>(deserializer: D) -> Result<Address, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    Address::from_str(&value)
        .map(|address| address.assume_checked())
        .map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Txid, Witness};

    use super::{WalletRequest, WalletRequestKind};

    fn regtest_address() -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[7u8; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    #[test]
    fn send_request_rejects_zero_amount() {
        let err = WalletRequest::send("k1", regtest_address(), 0).unwrap_err();
        assert!(err.to_string().contains("amount"), "got: {err}");
    }

    #[test]
    fn spend_request_rejects_zero_value() {
        let err = WalletRequest::spend(
            "k2",
            OutPoint {
                txid: Txid::from_byte_array([3u8; 32]),
                vout: 1,
            },
            0,
            ScriptBuf::new(),
            Witness::new(),
            ScriptBuf::new(),
            bitcoin::taproot::TapLeafHash::all_zeros(),
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            TapSighashType::All,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("value"), "got: {err}");
    }

    #[test]
    fn sacp_spend_requires_recipient() {
        let err = WalletRequest::spend(
            "k3",
            OutPoint {
                txid: Txid::from_byte_array([4u8; 32]),
                vout: 2,
            },
            10_000,
            ScriptBuf::new(),
            Witness::new(),
            ScriptBuf::new(),
            bitcoin::taproot::TapLeafHash::all_zeros(),
            Sequence::ENABLE_RBF_NO_LOCKTIME,
            TapSighashType::SinglePlusAnyoneCanPay,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("recipient"), "got: {err}");
    }

    #[test]
    fn dedupe_key_is_preserved_on_valid_send_request() {
        let request = WalletRequest::send("dedupe-key", regtest_address(), 10_000).unwrap();
        assert_eq!(request.dedupe_key(), "dedupe-key");
        assert!(matches!(request.kind(), WalletRequestKind::Send(_)));
    }
}
