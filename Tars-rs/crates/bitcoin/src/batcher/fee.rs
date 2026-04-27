use crate::{batcher::size::estimate_vsize, htlc::tx::DUST_LIMIT, FeeRate};
use bitcoin::{Amount, TxIn, TxOut};
use eyre::{eyre, Ok, Result};

/// Estimates the transaction fee based on the inputs, outputs, and fee rate.
///
/// # Arguments
/// * `inputs` - A slice of transaction inputs that are part of the transaction.
/// * `outputs` - A slice of transaction outputs that the transaction will include.
/// * `fee_rate` - The fee rate (satoshis per byte) used to calculate the fee.
///
/// # Returns
/// * The estimated fee for the transaction as a `u64`.
pub fn estimate_fee(inputs: &[TxIn], outputs: &[TxOut], fee_rate: FeeRate) -> u64 {
    let vsize = estimate_vsize(inputs, outputs);
    let fee = (vsize as f64 * fee_rate.value()).ceil() as u64;

    fee
}

/// Distributes the provided fee evenly across the transaction outputs, with any remainder allocated to
/// the output with the highest value.
///
/// # Arguments
/// * `outputs` - A mutable reference to a vector of transaction outputs to adjust for the fee.
/// * `fee` - The total fee to distribute across the outputs.
///
/// # Returns
/// * `Ok(())` if the fee adjustment is successful.
/// * `Err` if any output would become dust after the fee adjustment or if the outputs list is empty.
pub fn adjust_outputs_for_fee(outputs: &mut Vec<TxOut>, fee: u64) -> Result<()> {
    if outputs.is_empty() {
        return Err(eyre!("Cannot adjust fee amount for empty outputs list"));
    }

    let n = outputs.len();
    let fee_per_output = fee / n as u64;
    let fee_remainder = fee % n as u64;

    // Find the index of the output with the maximum value before fee deduction
    let max_index = outputs
        .iter()
        .enumerate()
        .max_by_key(|(_, o)| o.value.to_sat())
        .map(|(i, _)| i)
        .ok_or_else(|| eyre!("Cannot adjust fee amount for empty outputs list"))?;

    for (i, output) in outputs.iter_mut().enumerate() {
        let mut fee_deduction = fee_per_output;
        if i == max_index {
            fee_deduction += fee_remainder;
        }

        let value = output.value.to_sat();
        if value < fee_deduction + DUST_LIMIT {
            return Err(eyre!(
                "Output value at index {} would become dust after fee deduction",
                i
            ));
        }

        output.value = Amount::from_sat(value - fee_deduction);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        batcher::sign::SACP_SIGHASH_TYPE,
        htlc::tx::{create_inputs_from_utxos, create_outputs, get_output_values},
        test_utils::get_test_spend_request,
    };
    use bitcoin::{ScriptBuf, Sequence};

    #[tokio::test]
    async fn test_estimate_fee_for_spend_requests() -> Result<()> {
        let spend_requests = vec![get_test_spend_request().await?];

        let fee_rate = 1.0;
        let mut inputs = Vec::new();
        let mut outputs = Vec::new();

        for spend_request in spend_requests.iter() {
            inputs.extend(create_inputs_from_utxos(
                &spend_request.utxos,
                &spend_request.witness,
                Sequence::MAX,
            ));
            outputs.extend(create_outputs(
                get_output_values(&spend_request.utxos, SACP_SIGHASH_TYPE)?,
                &spend_request.recipient,
                None,
            )?);
        }

        let estimated_fee = estimate_fee(&inputs, &outputs, FeeRate::new(fee_rate)?);

        let vsize = estimate_vsize(&inputs, &outputs);
        let actual_fee = (fee_rate * vsize as f64).ceil() as u64;
        assert_eq!(estimated_fee, actual_fee);
        Ok(())
    }

    #[test]
    fn test_distribute_fee_across_outputs_success() {
        let mut outputs = vec![
            TxOut {
                value: bitcoin::Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: bitcoin::Amount::from_sat(15_000),
                script_pubkey: ScriptBuf::new(),
            },
        ];

        adjust_outputs_for_fee(&mut outputs, 2_000).unwrap();

        let mut total_output_value = 0;
        let distributed_fee = [9000, 14000];

        for (i, output) in outputs.iter().enumerate() {
            total_output_value += output.value.to_sat();
            assert!(output.value.to_sat() == distributed_fee[i]);
        }

        assert!(
            total_output_value == 23_000,
            "Total output value: {}",
            total_output_value
        );
    }

    #[test]
    fn test_distribute_fee_across_outputs_max_output() {
        let mut outputs = vec![
            TxOut {
                value: bitcoin::Amount::from_sat(9_000),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: bitcoin::Amount::from_sat(10_000),
                script_pubkey: ScriptBuf::new(),
            },
        ];

        let max_output_index = 1;

        adjust_outputs_for_fee(&mut outputs, 555).unwrap();

        for (i, output) in outputs.iter().enumerate() {
            if i == max_output_index {
                // For the output with the maximum value (index 1), we expect the fee remainder to be added
                // The fee per output is 555 / 2 = 277 (integer division)
                // The remainder (555 % 2 = 1) is removed from the largest output (which is the one with value 10,000)

                // The expected value for the largest output should be its original value minus the fee per output
                // plus the remainder. So, 10,000 - 277 - 1 = 9,722.
                assert!(output.value.to_sat() == 9722);
            } else {
                assert!(output.value.to_sat() == 8723);
            }
        }
    }

    #[test]
    fn test_distribute_fee_across_outputs_dust_error() {
        let mut outputs = vec![
            TxOut {
                value: bitcoin::Amount::from_sat(DUST_LIMIT + 1),
                script_pubkey: ScriptBuf::new(),
            },
            TxOut {
                value: bitcoin::Amount::from_sat(DUST_LIMIT + 1),
                script_pubkey: ScriptBuf::new(),
            },
        ];

        let fee = (2 * DUST_LIMIT) + 10;
        let result = adjust_outputs_for_fee(&mut outputs, fee);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("dust after fee deduction"));
    }
}
