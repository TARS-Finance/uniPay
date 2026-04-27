use crate::{
    batcher::{
        fee::{adjust_outputs_for_fee, estimate_fee},
        primitives::SpendRequest,
        sign::SACP_SIGHASH_TYPE,
    },
    htlc::tx::{
        create_inputs_from_utxos, create_outputs, get_output_values, DEFAULT_TX_LOCKTIME,
        DEFAULT_TX_VERSION,
    },
    FeeRate, ValidSpendRequests,
};
use bitcoin::{Sequence, Transaction, TxIn, TxOut};
use eyre::{Context, Result};

/// Creates transaction inputs and outputs from spend requests.
///
/// # Arguments
/// * `spend_requests` - A slice of spend requests containing UTXOs and recipient information.
///
/// # Returns
/// * `Ok((Vec<TxIn>, Vec<TxOut>))` if inputs and outputs are successfully created.
/// * `Err` if any error occurs during creation.
fn create_batch_inputs_and_outputs(
    spend_requests: &[SpendRequest],
) -> Result<(Vec<TxIn>, Vec<TxOut>)> {
    // Pre-allocate vectors with calculated capacity for better performance
    let total_utxos: usize = spend_requests.iter().map(|req| req.utxos.len()).sum();

    let mut inputs = Vec::with_capacity(total_utxos);
    let mut outputs = Vec::with_capacity(spend_requests.len());

    for spend_request in spend_requests {
        // Create inputs from UTXOs
        let new_inputs =
            create_inputs_from_utxos(&spend_request.utxos, &spend_request.witness, Sequence::MAX);

        inputs.extend(new_inputs);

        // Create outputs with proper error context
        let output_values = get_output_values(&spend_request.utxos, SACP_SIGHASH_TYPE)
            .with_context(|| {
                format!(
                    "Failed to get output values for spend request '{}'",
                    spend_request.id
                )
            })?;

        let new_outputs = create_outputs(
            output_values,
            &spend_request.recipient,
            None, // Not considering fee for output values here, will be adjusted later.
        )
        .with_context(|| {
            format!(
                "Failed to create outputs for spend request '{}'",
                spend_request.id
            )
        })?;

        outputs.extend(new_outputs);
    }

    Ok((inputs, outputs))
}

/// Builds a batch transaction from spend requests, adjusting the fee among all outputs equally.
///
/// This function performs the following steps:
/// 1. Validates input parameters (non-empty requests, positive fee rate)
/// 2. Verifies that each spend request contains UTXOs
/// 3. Creates transaction inputs and outputs from spend requests
/// 4. Estimates and adjusts transaction fee across outputs
/// 5. Constructs the final transaction
///
/// # Arguments
/// * `spend_requests` - A slice of spend requests containing UTXOs and recipient information.
///                     Must not be empty and each request must contain at least one UTXO.
/// * `fee_rate` - The fee rate in satoshis per vbyte. Must be positive.
///
/// # Returns
/// * `Ok(Transaction)` if the batch transaction is successfully created.
/// * `Err` if validation fails, spend requests are invalid, or fee estimation fails.
pub fn build_batch_tx(
    spend_requests: &ValidSpendRequests,
    fee_rate: FeeRate,
) -> Result<Transaction> {
    let spend_requests = spend_requests.as_ref();

    // Create inputs and outputs
    let (inputs, mut outputs) = create_batch_inputs_and_outputs(spend_requests)?;

    // Estimate and adjust fee
    let fee = estimate_fee(&inputs, &outputs, fee_rate);
    adjust_outputs_for_fee(&mut outputs, fee)
        .with_context(|| "Failed to adjust outputs for transaction fee")?;

    // Construct the final transaction
    let tx = Transaction {
        version: DEFAULT_TX_VERSION,
        lock_time: DEFAULT_TX_LOCKTIME,
        input: inputs,
        output: outputs,
    };

    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        batcher::sign::sign_batch_tx,
        test_utils::{get_test_bitcoin_indexer, get_test_spend_request, TEST_FEE_RATE},
    };

    #[tokio::test]
    async fn test_build_batch_tx() -> Result<()> {
        const REQUEST_COUNT: usize = 5;

        // Create spend requests more cleanly
        let spend_requests: Result<Vec<_>> =
            futures::future::try_join_all((0..REQUEST_COUNT).map(|_| get_test_spend_request()))
                .await;
        let indexer = get_test_bitcoin_indexer()?;

        let spend_requests = ValidSpendRequests::validate(spend_requests?, &indexer).await?;

        let mut tx = build_batch_tx(&spend_requests, FeeRate::new(TEST_FEE_RATE)?)?;

        // Add actual assertions to verify transaction structure
        assert_eq!(
            tx.input.len(),
            spend_requests
                .as_ref()
                .iter()
                .map(|r| r.utxos.len())
                .sum::<usize>(),
            "Transaction input count should match total UTXO count"
        );
        assert_eq!(
            tx.output.len(),
            spend_requests.as_ref().len(),
            "Transaction output count should match spend request count"
        );
        assert!(
            tx.output.iter().all(|out| out.value.to_sat() > 0),
            "All outputs should have positive values"
        );

        sign_batch_tx(&mut tx, &spend_requests)?;
        indexer.submit_tx(&tx).await?;

        println!("Transaction ID: {}", tx.compute_txid());

        Ok(())
    }

    #[tokio::test]
    async fn test_build_batch_tx_single_request() -> Result<()> {
        let spend_request = get_test_spend_request().await?;
        let spend_requests = vec![spend_request.clone()];
        let indexer = get_test_bitcoin_indexer()?;
        let spend_requests = ValidSpendRequests::validate(spend_requests, &indexer).await?;

        let tx = build_batch_tx(&spend_requests, FeeRate::new(TEST_FEE_RATE)?)?;

        assert_eq!(tx.input.len(), spend_request.utxos.len());
        assert_eq!(tx.output.len(), 1);
        assert!(tx.output[0].value.to_sat() > 0);

        Ok(())
    }

    #[test]
    fn test_fee_rate_validation() {
        // Valid fee rates
        assert!(FeeRate::new(1.0).is_ok());
        assert!(FeeRate::new(0.1).is_ok());
        assert!(FeeRate::new(100.0).is_ok());

        // Invalid fee rates
        assert!(FeeRate::new(0.0).is_err());
        assert!(FeeRate::new(-1.0).is_err());
        assert!(FeeRate::new(-0.1).is_err());
    }

    #[test]
    fn test_fee_rate_value_access() {
        let fee_rate = FeeRate::new(2.5).unwrap();
        assert_eq!(fee_rate.value(), 2.5);
    }

    #[tokio::test]
    async fn test_create_batch_inputs_and_outputs() -> Result<()> {
        let spend_request1 = get_test_spend_request().await?;
        let spend_request2 = get_test_spend_request().await?;
        let spend_requests = vec![spend_request1.clone(), spend_request2.clone()];

        let (inputs, outputs) = create_batch_inputs_and_outputs(&spend_requests)?;

        let expected_input_count = spend_request1.utxos.len() + spend_request2.utxos.len();
        assert_eq!(inputs.len(), expected_input_count);
        assert_eq!(outputs.len(), 2); // One output per spend request

        Ok(())
    }
}
