use crate::{htlc::validate::validate_utxos, indexer::primitives::Utxo};
use bitcoin::{
    absolute::LockTime, transaction::Version, Address, Amount, ScriptBuf, Sequence, TapSighashType,
    Transaction, TxIn, TxOut, Witness,
};
use eyre::{bail, eyre, Result};

/// Default transaction version used for HTLC transactions
pub const DEFAULT_TX_VERSION: Version = Version::TWO;

/// Default transaction locktime, set to zero (no timelock)
pub const DEFAULT_TX_LOCKTIME: LockTime = LockTime::ZERO;

/// Bitcoin dust limit (546 satoshis for most output types)
pub const DUST_LIMIT: u64 = 546;

/// Base transaction overhead, which includes the size of the following fixed components:
/// * version (4 bytes)
/// * locktime (4 bytes)
pub const TX_BASE_OVERHEAD: usize = 8;

/// Checks if an output value meets Bitcoin dust limits.
///
/// # Arguments
/// * `value` - Output value in satoshis
///
/// # Returns
/// * `bool` - True if value is above dust limit
fn is_above_dust_limit(value: u64) -> bool {
    value >= DUST_LIMIT
}

/// Sorts UTXOs by txid and vout for deterministic transaction structure.
///
/// This ensures that transaction construction remains consistent across
/// multiple builds with the same inputs.
///
/// # Arguments
/// * `utxos` - Slice of UTXOs to sort
///
/// # Returns
/// * `Vec<Utxo>` - Sorted vector of UTXOs
pub fn sort_utxos(utxos: &[Utxo]) -> Vec<Utxo> {
    let mut sorted_utxos = utxos.to_vec();
    sorted_utxos.sort_by(|a, b| a.txid.cmp(&b.txid).then(a.vout.cmp(&b.vout)));
    sorted_utxos
}

/// Creates unsigned transaction inputs from UTXOs.
///
/// Converts a slice of UTXOs into transaction inputs with empty signatures.
///
/// # Arguments
/// * `utxos` - A slice of UTXOs to convert into inputs.
/// * `witness` - A witness to associate with each input.
/// * `sequence` - A sequence number to associate with each input.
///
/// # Returns
/// * `Vec<TxIn>` - A vector of unsigned transaction inputs.
pub fn create_inputs_from_utxos(
    utxos: &[Utxo],
    witness: &Witness,
    sequence: Sequence,
) -> Vec<TxIn> {
    utxos
        .iter()
        .map(|utxo| TxIn {
            previous_output: utxo.to_outpoint(),
            script_sig: ScriptBuf::new(),
            sequence,
            witness: witness.clone(),
        })
        .collect()
}

/// Creates transaction outputs from output values with fee deduction from the largest value.
///
/// This function creates Bitcoin transaction outputs for a single recipient, where the transaction
/// fee is deducted from the largest output value. This approach ensures that the
/// fee burden falls on the largest output, minimizing the impact on smaller outputs.
///
/// # Arguments
/// * `output_values` - Vector of output values in satoshis
/// * `recipient` - Destination address for all outputs
/// * `fee` - Optional transaction fee in satoshis (defaults to 0 if None)
///
/// # Returns
/// * `Result<Vec<TxOut>>` - Vector of transaction outputs ready for inclusion in a transaction.
pub fn create_outputs(
    output_values: Vec<u64>,
    recipient: &Address,
    fee: Option<u64>,
) -> Result<Vec<TxOut>> {
    let fee = fee.unwrap_or(0);

    // Find index of the largest value
    let max_index = output_values
        .iter()
        .enumerate()
        .max_by_key(|(_, &value)| value)
        .map(|(i, _)| i)
        .ok_or_else(|| eyre!("Output values are empty"))?;

    output_values
        .into_iter()
        .enumerate()
        .map(|(i, value)| {
            let mut output_value = value;
            if i == max_index {
                output_value = output_value
                    .checked_sub(fee)
                    .ok_or_else(|| eyre!("Fee ({}) exceeds output value ({})", fee, value))?;
            }

            if !is_above_dust_limit(output_value) {
                bail!(
                    "Output value {} below dust limit ({})",
                    output_value,
                    DUST_LIMIT
                );
            }

            Ok(TxOut {
                value: Amount::from_sat(output_value),
                script_pubkey: recipient.script_pubkey(),
            })
        })
        .collect()
}

/// Creates transaction outputs representing previous HTLC outputs.
///
/// Generates outputs that match the original HTLC address outputs,
/// used for signature hash computation and witness validation.
///
/// # Arguments
/// * `utxos` - Slice of UTXOs to create outputs for
/// * `address` - The address these outputs were sent to
///
/// # Returns
/// * `Vec<TxOut>` - Vector of transaction outputs
pub fn create_previous_outputs(utxos: &[Utxo], address: &Address) -> Vec<TxOut> {
    utxos
        .iter()
        .map(|utxo| TxOut {
            value: Amount::from_sat(utxo.value),
            script_pubkey: address.script_pubkey(),
        })
        .collect()
}

/// Generates output values array based on the sighash type and UTXOs.
///
/// # Arguments
/// * `utxos` - Slice of UTXOs to process
/// * `sighash_type` - The Taproot sighash type that determines output distribution
///
/// # Returns
/// * `Result<Vec<u64>>` - Vector of output values or an error
///
/// # Errors
/// * When an unsupported sighash type is provided
pub fn get_output_values(utxos: &[Utxo], sighash_type: TapSighashType) -> Result<Vec<u64>> {
    match sighash_type {
        // One output per UTXO
        TapSighashType::SinglePlusAnyoneCanPay => Ok(utxos.iter().map(|utxo| utxo.value).collect()),

        // Single output for total input value
        TapSighashType::All => {
            let total_value: u64 = utxos.iter().map(|u| u.value).sum();
            Ok(vec![total_value])
        }

        _ => Err(eyre!(
            "Unsupported sighash type: {:?}. Only SinglePlusAnyoneCanPay and All are supported.",
            sighash_type
        )),
    }
}

/// Creates a Bitcoin transaction from a set of UTXOs with specified parameters.
///
/// This function constructs a unsigned Bitcoin transaction by combining UTXOs as inputs
/// and creating outputs . The transactbased on the specified sighash typeion can include
/// an optional fee that will be deducted from the maximum utxo value.
///
/// # Arguments
/// * `utxos` - Vector of UTXOs to use as transaction inputs
/// * `recpient` - The recipient address for the transaction outputs
/// * `witness` - The witness to associate with each input
/// * `sighash_type` - The Taproot sighash type that determines output value distribution
/// * `sequence` - The sequence number for all transaction inputs
/// * `fee` - Optional fee amount to deduct from the total input value
///
/// # Returns
/// * `Result<Transaction>` - The constructed Bitcoin transaction or an error
pub fn build_tx(
    utxos: &[Utxo],
    recpient: &Address,
    witness: &Witness,
    sequence: Sequence,
    sighash_type: TapSighashType,
    fee: Option<u64>,
) -> Result<Transaction> {
    validate_utxos(utxos)?;

    // Calculate output values based on sighash type (e.g., single output vs multiple outputs)
    let output_values = get_output_values(utxos, sighash_type)?;

    let outputs = create_outputs(output_values, recpient, fee)?;

    let inputs = create_inputs_from_utxos(&utxos, &witness, sequence);

    // Construct and return the unsigned transaction
    Ok(Transaction {
        version: DEFAULT_TX_VERSION,
        lock_time: DEFAULT_TX_LOCKTIME,
        input: inputs,
        output: outputs,
    })
}

/// Returns the size in bytes needed to encode a length value using Bitcoin's VarInt format.
///
/// This function is adapted from the `bitcoin` crate's internal implementation,
/// specifically from the `encode::VarInt::len` logic in `bitcoin/src/consensus/encode.rs`.
///
/// Reference:
/// https://docs.rs/bitcoin/latest/src/bitcoin/consensus/encode.rs.html
///
/// # Arguments
/// * `length` - The value to be encoded
///
/// # Returns
/// * `usize` - Number of bytes required to encode the length as a VarInt
pub const fn size(length: usize) -> usize {
    match length {
        0..=0xFC => 1,
        0xFD..=0xFFFF => 3,
        0x10000..=0xFFFFFFFF => 5,
        _ => 9,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::get_dummy_utxo;
    use crate::UtxoStatus;
    use bitcoin::hashes::Hash;
    use bitcoin::{Network, Txid, XOnlyPublicKey};
    use eyre::Result;

    const TEST_FEE: u64 = 1000;

    #[test]
    fn test_sort_utxos() {
        let mut unsorted_utxos = vec![get_dummy_utxo(), get_dummy_utxo(), get_dummy_utxo()];
        unsorted_utxos[0].txid = Txid::all_zeros();
        unsorted_utxos[1].txid = Txid::from_slice(&[1u8; 32]).unwrap();
        unsorted_utxos[2].txid = Txid::from_slice(&[2u8; 32]).unwrap();
        let sorted_utxos = sort_utxos(&unsorted_utxos);

        // Verify sorting by txid and then vout
        for i in 1..sorted_utxos.len() {
            let curr = &sorted_utxos[i];
            let prev = &sorted_utxos[i - 1];
            assert!(
                curr.txid > prev.txid || (curr.txid == prev.txid && curr.vout > prev.vout),
                "UTXOs not properly sorted"
            );
        }
    }

    #[test]
    fn test_create_inputs_from_utxos() {
        let utxos = vec![get_dummy_utxo(), get_dummy_utxo(), get_dummy_utxo()];
        let inputs = create_inputs_from_utxos(&utxos, &Witness::new(), Sequence::MAX);

        assert_eq!(
            inputs.len(),
            utxos.len(),
            "Input count should match UTXO count"
        );

        for (input, utxo) in inputs.iter().zip(utxos.iter()) {
            assert_eq!(input.previous_output.txid, utxo.txid);
            assert_eq!(input.previous_output.vout, utxo.vout);
            assert!(input.script_sig.is_empty(), "ScriptSig should be empty");
            assert_eq!(input.sequence, Sequence::MAX);
            assert!(input.witness.is_empty(), "Witness should be empty");
        }
    }

    #[test]
    fn test_create_outputs() -> Result<()> {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let dummy_pubkey = XOnlyPublicKey::from_slice(&[2u8; 32]).unwrap();
        let address = Address::p2tr(&secp, dummy_pubkey, None, Network::Testnet);

        // Test case 1: Multiple output values with fee deducted from largest
        let output_values = vec![100_000, 50_000, 25_000];
        let outputs = create_outputs(output_values, &address, Some(1000))?;

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[0].value.to_sat(), 99_000); // 100k - 1k fee
        assert_eq!(outputs[1].value.to_sat(), 50_000); // No fee deducted
        assert_eq!(outputs[2].value.to_sat(), 25_000); // No fee deducted

        // Verify all outputs go to the same recipient
        for output in &outputs {
            assert_eq!(output.script_pubkey, address.script_pubkey());
        }

        // Test case 2: Single output value with fee
        let single_output = vec![200_000];
        let outputs = create_outputs(single_output, &address, Some(500))?;

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].value.to_sat(), 199_500); // 200k - 500 fee
        assert_eq!(outputs[0].script_pubkey, address.script_pubkey());

        // Test case 3: No fee
        let output_values = vec![100_000, 50_000];
        let outputs = create_outputs(output_values, &address, None)?;

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].value.to_sat(), 100_000); // No fee deducted
        assert_eq!(outputs[1].value.to_sat(), 50_000); // No fee deducted

        // Test case 4: Fee exceeds largest value
        let output_values = vec![100_000, 50_000];
        let result = create_outputs(output_values, &address, Some(150_000));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Fee"));

        // Test case 5: Empty output values
        let result = create_outputs(vec![], &address, Some(1000));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Output values are empty"));

        // Test case 6: Dust limit validation
        let output_values = vec![600]; // Would become dust after fee
        let result = create_outputs(output_values, &address, Some(100));
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("dust limit"));

        Ok(())
    }

    #[test]
    fn test_get_output_values() -> Result<()> {
        let utxos = vec![get_dummy_utxo(), get_dummy_utxo(), get_dummy_utxo()];

        // Test SIGHASH_SINGLE + ANYONECANPAY: should return one output value per UTXO
        let output_values = get_output_values(&utxos, TapSighashType::SinglePlusAnyoneCanPay)?;
        assert_eq!(output_values.len(), utxos.len());

        // Verify the values match the UTXO values
        for (output_value, utxo) in output_values.iter().zip(utxos.iter()) {
            assert_eq!(*output_value, utxo.value);
        }

        // Test SIGHASH_ALL: should return single output value with total of all UTXOs
        let output_values = get_output_values(&utxos, TapSighashType::All)?;
        assert_eq!(output_values.len(), 1);

        let total_utxo_value: u64 = utxos.iter().map(|u| u.value).sum();
        assert_eq!(output_values[0], total_utxo_value);

        // Test unsupported sighash type: should return error
        let result = get_output_values(&utxos, TapSighashType::None);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unsupported sighash type"));

        // Test with empty UTXOs
        let empty_utxos: Vec<Utxo> = vec![];
        let output_values =
            get_output_values(&empty_utxos, TapSighashType::SinglePlusAnyoneCanPay)?;
        assert_eq!(output_values.len(), 0);

        let output_values = get_output_values(&empty_utxos, TapSighashType::All)?;
        assert_eq!(output_values.len(), 1);
        assert_eq!(output_values[0], 0);

        Ok(())
    }

    #[test]
    fn test_create_transaction_from_utxos() -> Result<()> {
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let dummy_pubkey = XOnlyPublicKey::from_slice(&[2u8; 32]).unwrap();
        let recipient = Address::p2tr(&secp, dummy_pubkey, None, Network::Testnet);

        // Test case 1: Normal transaction with fee
        let utxos = vec![get_dummy_utxo(), get_dummy_utxo(), get_dummy_utxo()];
        let tx = build_tx(
            &utxos,
            &recipient,
            &Witness::new(),
            Sequence::MAX,
            TapSighashType::All,
            Some(TEST_FEE),
        )?;

        assert_eq!(tx.input.len(), utxos.len());
        assert_eq!(tx.output.len(), 1); // SIGHASH_ALL creates only one output

        // Verify fee is deducted from the total
        let total_output = tx.output[0].value.to_sat();
        let total_input: u64 = utxos.iter().map(|u| u.value).sum();
        assert_eq!(total_input - total_output, TEST_FEE);

        // Test case 2: Transaction with no fee
        let tx_no_fee = build_tx(
            &utxos,
            &recipient,
            &Witness::new(),
            Sequence::MAX,
            TapSighashType::All,
            None,
        )?;
        let total_output_no_fee: u64 = tx_no_fee.output.iter().map(|o| o.value.to_sat()).sum();
        assert_eq!(total_input, total_output_no_fee);

        // Test case 3: Empty UTXOs
        let empty_utxos: Vec<Utxo> = vec![];
        let result = build_tx(
            &empty_utxos,
            &recipient,
            &Witness::new(),
            Sequence::MAX,
            TapSighashType::All,
            None,
        );
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No UTXOs provided"));

        // Test case 4: Fee exceeds value
        let small_utxos = vec![Utxo {
            txid: Txid::hash(&[1u8; 32]),
            vout: 0,
            value: 1000,
            status: UtxoStatus {
                confirmed: true,
                block_height: Some(100),
            },
        }];
        let result = build_tx(
            &small_utxos,
            &recipient,
            &Witness::new(),
            Sequence::MAX,
            TapSighashType::All,
            Some(TEST_FEE + 1000), // Adding additional fee
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Fee"));

        Ok(())
    }

    #[test]
    fn test_create_previous_outputs() {
        let utxos = vec![get_dummy_utxo(), get_dummy_utxo(), get_dummy_utxo()];
        let secp = bitcoin::secp256k1::Secp256k1::new();
        let dummy_pubkey = XOnlyPublicKey::from_slice(&[2u8; 32]).unwrap();
        let address = Address::p2tr(&secp, dummy_pubkey, None, Network::Regtest);

        let outputs = create_previous_outputs(&utxos, &address);

        assert_eq!(
            outputs.len(),
            utxos.len(),
            "Output count should match UTXO count"
        );

        for (output, utxo) in outputs.iter().zip(utxos.iter()) {
            assert_eq!(output.value, Amount::from_sat(utxo.value));
            assert_eq!(output.script_pubkey, address.script_pubkey());
        }

        // Test with empty UTXOs
        let empty_utxos: Vec<Utxo> = vec![];
        let empty_outputs = create_previous_outputs(&empty_utxos, &address);
        assert_eq!(empty_outputs.len(), 0);

        // Test with single UTXO
        let single_utxo = vec![get_dummy_utxo()];
        let single_outputs = create_previous_outputs(&single_utxo, &address);
        assert_eq!(single_outputs.len(), 1);
        assert_eq!(
            single_outputs[0].value,
            Amount::from_sat(single_utxo[0].value)
        );
        assert_eq!(single_outputs[0].script_pubkey, address.script_pubkey());
    }

    #[test]
    fn test_dust_limit_validation() {
        assert!(!is_above_dust_limit(545)); // Below dust limit
        assert!(is_above_dust_limit(546)); // At dust limit
        assert!(is_above_dust_limit(1000)); // Above dust limit
    }
}
