use crate::{
    batcher::sign::SignaturePlaceholder,
    htlc::tx::{size, TX_BASE_OVERHEAD},
};
use bitcoin::{blockdata::weight::WITNESS_SCALE_FACTOR, TxIn, TxOut, Witness};

/// The overhead for witness marker and flag in a transaction.
pub const WITNESS_MARKER_FLAG_SIZE: usize = 2;

/// Estimates the size of the witness data for a given witness, including varint encoding.
///
/// This function calculates the size of the witness, taking into account the size of each element
/// in the witness. It includes the size of each signature placeholder and the varint encoding
/// required for the witness data.
///
/// # Arguments
/// * `witness` - The witness data that needs to be sized.
///
/// # Returns
/// * `usize` - The total estimated size of the witness, including varint encoding.
pub fn estimate_witness_size(witness: &Witness) -> usize {
    witness
        .iter()
        .map(|element| {
            let element_size = SignaturePlaceholder::from_bytes(element)
                .map(|ph| ph.size())
                .unwrap_or_else(|| element.len());

            element_size + size(element_size)
        })
        .sum::<usize>()
        + size(witness.len())
}

/// Estimates the virtual size (vsize) of a transaction based on the inputs and outputs.
///
/// This function calculates the vsize of a Bitcoin transaction, It considers the base overhead of the
/// transaction, the size of the inputs and outputs, as well as the witness data size.
///
/// # Arguments
/// * `inputs` - A slice of transaction inputs.
/// * `outputs` - A slice of transaction outputs.
///
/// # Returns
/// * `u64` - The estimated vsize of the transaction.
pub fn estimate_vsize(inputs: &[TxIn], outputs: &[TxOut]) -> u64 {
    let mut base_tx_size = TX_BASE_OVERHEAD + size(inputs.len()) + size(outputs.len());

    // Calculate the base size of the inputs and the witness size
    let mut input_base_size = 0;
    let mut witness_size = 0;
    for input in inputs {
        input_base_size += input.base_size();
        witness_size += estimate_witness_size(&input.witness);
    }

    // Calculate the base size of the outputs
    let output_base_size: usize = outputs.iter().map(|output| output.size()).sum();

    base_tx_size += input_base_size + output_base_size;

    // Calculate the total size of the transaction
    let total_tx_size = base_tx_size + WITNESS_MARKER_FLAG_SIZE + witness_size;
    let tx_weight = (base_tx_size * (WITNESS_SCALE_FACTOR - 1)) + total_tx_size;

    ((tx_weight + WITNESS_SCALE_FACTOR - 1) / WITNESS_SCALE_FACTOR) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        batcher::sign::{add_signature_request, SACP_SIGHASH_TYPE},
        htlc::tx::{create_inputs_from_utxos, create_outputs, get_output_values},
        test_utils::{generate_bitcoin_random_keypair, get_dummy_sighash, get_dummy_utxo},
        Utxo,
    };
    use bitcoin::{
        absolute::LockTime,
        key::{Keypair, Secp256k1},
        transaction::{Transaction, Version},
        Address, Network, Sequence, Witness,
    };
    use eyre::{eyre, Result};
    use std::str::FromStr;

    fn generate_witness_with_placeholder() -> Witness {
        let witness = vec![b"add_signature_segwit_v1"];
        Witness::from_slice(&witness)
    }

    fn generate_witness_without_placeholder() -> Witness {
        let witness = vec![b"some_data"];
        Witness::from_slice(&witness)
    }

    #[test]
    fn test_estimate_witness_size() -> Result<()> {
        // Case 1: Witness with signature placeholder
        let witness_with_placeholder = generate_witness_with_placeholder();
        let size_with_placeholder = estimate_witness_size(&witness_with_placeholder);
        // Schnorr signature (65 bytes - 64 bytes signature + 1 byte sighash type) + 1 byte varint encoding + 1 byte witness length.
        assert_eq!(
            size_with_placeholder, 67,
            "Witness size mismatch for witness with signature placeholder: expected 66, got {}",
            size_with_placeholder
        );

        // Case 2: Witness without signature placeholder
        let witness_without_placeholder = generate_witness_without_placeholder();
        let size_without_placeholder = estimate_witness_size(&witness_without_placeholder);
        // 9 bytes of data ('some_data') + 1 byte varint encoding + 1 byte witness length.
        assert_eq!(
            size_without_placeholder, 11,
            "Witness size mismatch for witness without signature placeholder: expected 10, got {}",
            size_without_placeholder
        );

        // Case 3: Mixed witness with signature placeholder and without
        let mut mixed_witness = generate_witness_with_placeholder();
        mixed_witness.push(b"some_data".to_vec());
        let mixed_size = estimate_witness_size(&mixed_witness);
        // 65 bytes of data ('add_signature_segwit_v1') + 1 byte varint encoding + 9 bytes of data ('some_data') + 1 byte varint encoding + 1 byte witness length.
        assert_eq!(
            mixed_size, 77,
            "Witness size mismatch for mixed witness with signature placeholder and without: expected 76, got {}",
            mixed_size
        );

        // Case 4: Witness with no data at all
        let empty_witness: Witness = Witness::new();
        let empty_size = estimate_witness_size(&empty_witness);
        // 0 bytes of data + 1 byte varint encoding + 1 byte witness length.
        assert_eq!(
            empty_size, 1,
            "Witness size mismatch for empty witness: expected 1, got {}",
            empty_size
        );

        Ok(())
    }

    /// Run the test for the transaction vsize estimation.
    fn run_vsize_test(
        utxos: &[Utxo],
        witness: &Witness,
        keypair: &Keypair,
        sighash: &[u8; 32],
        recipient: &Address,
        description: &str,
    ) -> Result<()> {
        let secp = Secp256k1::new();

        let mut inputs = create_inputs_from_utxos(utxos, witness, Sequence::MAX);
        let outputs = create_outputs(
            get_output_values(utxos, SACP_SIGHASH_TYPE)?,
            recipient,
            None,
        )?;

        let estimated_vsize = estimate_vsize(&inputs, &outputs);

        for input in inputs.iter_mut() {
            add_signature_request(&secp, input, keypair, &sighash, SACP_SIGHASH_TYPE)?;
        }

        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: inputs,
            output: outputs,
        };

        let actual_vsize = tx.vsize() as u64;

        if actual_vsize != estimated_vsize {
            return Err(eyre!(
                "Vsize mismatch for {}: actual {} vs estimated {}",
                description,
                actual_vsize,
                estimated_vsize
            ));
        }

        Ok(())
    }

    #[test]
    fn test_calculate_tx_vsize() -> Result<()> {
        let utxos = vec![get_dummy_utxo(), get_dummy_utxo(), get_dummy_utxo()];

        let keypair = generate_bitcoin_random_keypair();
        let sighash = get_dummy_sighash();

        let witness = Witness::from(vec![
            vec![0u8; 72],
            vec![0u8; 33],
            b"add_signature_segwit_v1".to_vec(),
        ]);

        // P2WPKH Recipient
        let recipient = Address::from_str("bc1qg3srsy8ganjj6maj99te77n2qhff53rk3hafe5")?
            .require_network(Network::Bitcoin)?;

        assert!(run_vsize_test(
            &utxos,
            &witness,
            &keypair,
            &sighash,
            &recipient,
            "P2WPKH Recipient",
        )
        .is_ok());

        // P2TR Recipient
        let p2tr_recipient =
            Address::from_str("bc1p2x0nxmvt684j3g68lkdj0d7uae7rvpqvvxplwxjxs5ln97w4shkstkeur2")?
                .require_network(Network::Bitcoin)?;

        assert!(run_vsize_test(
            &utxos,
            &witness,
            &keypair,
            &sighash,
            &p2tr_recipient,
            "P2TR Recipient",
        )
        .is_ok());

        // P2PKH Recipient
        let p2pkh_recipient = Address::from_str("1LADvWHa4ge5PcgoJ596Ah7DBMwNq1quo9")?
            .require_network(Network::Bitcoin)?;

        assert!(run_vsize_test(
            &utxos,
            &witness,
            &keypair,
            &sighash,
            &p2pkh_recipient,
            "P2PKH Recipient",
        )
        .is_ok());

        // P2SH Recipient
        let p2sh_recipient = Address::from_str("3Ake67Ruw3hhPuFWxuirB97MWrjns5uqcH")?
            .require_network(Network::Bitcoin)?;

        assert!(run_vsize_test(
            &utxos,
            &witness,
            &keypair,
            &sighash,
            &p2sh_recipient,
            "P2SH Recipient",
        )
        .is_ok());

        Ok(())
    }
}
