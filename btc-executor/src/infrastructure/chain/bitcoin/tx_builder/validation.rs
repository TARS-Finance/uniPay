//! Input/output validation for Bitcoin batch transactions.
//!
//! Called before the fee-builder loop to reject obviously invalid requests
//! early, avoiding wasted UTXO queries.

use std::collections::HashSet;

use bitcoin::{OutPoint, TapSighashType};

use super::deps::TxBuilderError;
use super::primitives::{BitcoinTxAdaptorParams, CoverUtxo, DUST_LIMIT};
use crate::infrastructure::chain::bitcoin::wallet::SpendRequest;

/// Validates a single `SpendRequest` (used for both SACP and regular spends).
fn validate_spend(spend: &SpendRequest) -> Result<(), TxBuilderError> {
    if spend.value == 0 {
        return Err(TxBuilderError::Validation("UTXO value must be > 0".into()));
    }
    if spend.sighash_type == TapSighashType::SinglePlusAnyoneCanPay && spend.recipient.is_none() {
        return Err(TxBuilderError::Validation(
            "SACP spend must have a recipient".into(),
        ));
    }

    Ok(())
}

/// Validates SACP spends, regular spends, and send outputs before transaction
/// construction.
///
/// # Rules
/// - At least one operation (SACP spend, regular spend, or send) must be present.
/// - Every spend request must have a non-zero value.
/// - Every send output must be at or above the dust limit (546 sats).
pub fn validate(params: &BitcoinTxAdaptorParams) -> Result<(), TxBuilderError> {
    if params.sacps.is_empty() && params.spends.is_empty() && params.sends.is_empty() {
        return Err(TxBuilderError::Validation(
            "must have at least one operation".into(),
        ));
    }

    // Validate SACP spends
    for spend in &params.sacps {
        validate_spend(spend)?;
    }

    // Validate regular spends
    for spend in &params.spends {
        validate_spend(spend)?;
    }

    for send in &params.sends {
        if send.amount < DUST_LIMIT {
            return Err(TxBuilderError::Validation(format!(
                "output {} below dust {}",
                send.amount, DUST_LIMIT
            )));
        }
    }

    Ok(())
}

pub fn validate_unique_mandatory_inputs(
    params: &BitcoinTxAdaptorParams,
    carried_cover_utxos: &[CoverUtxo],
    lineage_prevout: Option<&CoverUtxo>,
) -> Result<(), TxBuilderError> {
    let mut seen = HashSet::new();

    for outpoint in params
        .sacps
        .iter()
        .map(|spend| spend.outpoint)
        .chain(params.spends.iter().map(|spend| spend.outpoint))
    {
        ensure_unique_outpoint(&mut seen, outpoint, "request input")?;
    }

    for utxo in carried_cover_utxos {
        ensure_unique_outpoint(&mut seen, utxo.outpoint, "carried cover input")?;
    }

    if let Some(lineage_prevout) = lineage_prevout {
        ensure_unique_outpoint(&mut seen, lineage_prevout.outpoint, "lineage prevout")?;
    }

    Ok(())
}

/// Ensure an outpoint only appears once across all mandatory inputs.
fn ensure_unique_outpoint(
    seen: &mut HashSet<OutPoint>,
    outpoint: OutPoint,
    role: &str,
) -> Result<(), TxBuilderError> {
    if !seen.insert(outpoint) {
        return Err(TxBuilderError::Validation(format!(
            "duplicate mandatory input outpoint detected for {role}: {outpoint}"
        )));
    }
    Ok(())
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::chain::bitcoin::wallet::{CoverUtxo, SendRequest};
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Keypair, Secp256k1};
    use bitcoin::taproot::TapLeafHash;
    use bitcoin::{Address, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Txid, Witness};

    fn dummy_sacp_spend() -> SpendRequest {
        let secp = Secp256k1::new();
        let keypair = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = keypair.x_only_public_key();
        let address = Address::p2tr(&secp, xonly, None, Network::Regtest);

        SpendRequest {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([1u8; 32]),
                vout: 0,
            },
            value: 50_000,
            script_pubkey: ScriptBuf::new(),
            witness_template: Witness::new(),
            recipient: Some(SendRequest {
                address: address.clone(),
                amount: 50_000,
            }),
            script: ScriptBuf::new(),
            leaf_hash: TapLeafHash::all_zeros(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            sighash_type: TapSighashType::SinglePlusAnyoneCanPay,
        }
    }

    fn dummy_regular_spend() -> SpendRequest {
        let secp = Secp256k1::new();
        let keypair = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = keypair.x_only_public_key();
        let _address = Address::p2tr(&secp, xonly, None, Network::Regtest);

        SpendRequest {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([2u8; 32]),
                vout: 0,
            },
            value: 50_000,
            script_pubkey: ScriptBuf::new(),
            witness_template: Witness::new(),
            recipient: None, // Regular spend: no paired output
            script: ScriptBuf::new(),
            leaf_hash: TapLeafHash::all_zeros(),
            sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
            sighash_type: TapSighashType::All,
        }
    }

    fn dummy_send() -> SendRequest {
        let secp = Secp256k1::new();
        let keypair = Keypair::new(&secp, &mut bitcoin::secp256k1::rand::thread_rng());
        let (xonly, _) = keypair.x_only_public_key();
        let address = Address::p2tr(&secp, xonly, None, Network::Regtest);
        SendRequest {
            address,
            amount: 10_000,
        }
    }

    fn empty_params() -> BitcoinTxAdaptorParams {
        BitcoinTxAdaptorParams {
            sacps: vec![],
            spends: vec![],
            sends: vec![],
            fee_rate: 5.0,
        }
    }

    #[test]
    fn rejects_empty_transaction() {
        let params = empty_params();
        let result = validate(&params);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("at least one"), "got: {msg}");
    }

    #[test]
    fn rejects_sacp_spend_with_no_utxos() {
        let mut spend = dummy_sacp_spend();
        spend.value = 0;
        let mut params = empty_params();
        params.sacps = vec![spend];
        let result = validate(&params);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("value must be > 0"), "got: {msg}");
    }

    #[test]
    fn rejects_regular_spend_with_no_utxos() {
        let mut spend = dummy_regular_spend();
        spend.value = 0;
        let mut params = empty_params();
        params.spends = vec![spend];
        let result = validate(&params);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("value must be > 0"), "got: {msg}");
    }

    #[test]
    fn rejects_zero_value_utxo_in_sacp() {
        let mut spend = dummy_sacp_spend();
        spend.value = 0;
        let mut params = empty_params();
        params.sacps = vec![spend];
        let result = validate(&params);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("value must be > 0"), "got: {msg}");
    }

    #[test]
    fn rejects_zero_value_utxo_in_regular_spend() {
        let mut spend = dummy_regular_spend();
        spend.value = 0;
        let mut params = empty_params();
        params.spends = vec![spend];
        let result = validate(&params);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("value must be > 0"), "got: {msg}");
    }

    #[test]
    fn rejects_sub_dust_send() {
        let mut send = dummy_send();
        send.amount = DUST_LIMIT - 1;
        let mut params = empty_params();
        params.sends = vec![send];
        let result = validate(&params);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("below dust"), "got: {msg}");
    }

    #[test]
    fn accepts_valid_sacp_only() {
        let mut params = empty_params();
        params.sacps = vec![dummy_sacp_spend()];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn accepts_valid_regular_spend_only() {
        let mut params = empty_params();
        params.spends = vec![dummy_regular_spend()];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn accepts_valid_send_only() {
        let mut params = empty_params();
        params.sends = vec![dummy_send()];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn accepts_valid_sacp_and_send() {
        let mut params = empty_params();
        params.sacps = vec![dummy_sacp_spend()];
        params.sends = vec![dummy_send()];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn accepts_valid_regular_spend_and_send() {
        let mut params = empty_params();
        params.spends = vec![dummy_regular_spend()];
        params.sends = vec![dummy_send()];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn accepts_all_three_operation_types() {
        let mut params = empty_params();
        params.sacps = vec![dummy_sacp_spend()];
        params.spends = vec![dummy_regular_spend()];
        params.sends = vec![dummy_send()];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn accepts_send_at_exact_dust_limit() {
        let mut send = dummy_send();
        send.amount = DUST_LIMIT;
        let mut params = empty_params();
        params.sends = vec![send];
        assert!(validate(&params).is_ok());
    }

    #[test]
    fn rejects_duplicate_outpoint_between_spend_and_lineage_prevout() {
        let spend = dummy_regular_spend();
        let mut params = empty_params();
        params.spends = vec![spend.clone()];

        let result = validate_unique_mandatory_inputs(
            &params,
            &[],
            Some(&CoverUtxo {
                outpoint: spend.outpoint,
                value: 20_000,
                script_pubkey: ScriptBuf::new(),
            }),
        );

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("duplicate mandatory input outpoint"),
            "got: {msg}"
        );
    }

    #[test]
    fn rejects_duplicate_outpoint_between_carried_cover_and_lineage_prevout() {
        let carried = CoverUtxo {
            outpoint: OutPoint {
                txid: Txid::from_byte_array([9u8; 32]),
                vout: 1,
            },
            value: 25_000,
            script_pubkey: ScriptBuf::new(),
        };

        let result = validate_unique_mandatory_inputs(
            &empty_params(),
            std::slice::from_ref(&carried),
            Some(&carried),
        );

        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("duplicate mandatory input outpoint"),
            "got: {msg}"
        );
    }
}
