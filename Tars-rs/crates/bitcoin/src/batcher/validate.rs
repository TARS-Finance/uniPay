use crate::{batcher::primitives::SpendRequest, htlc::tx::DUST_LIMIT, ArcIndexer, Utxo};
use bitcoin::TxIn;
use eyre::{bail, Result};
use std::collections::{HashMap, HashSet};
use tracing::warn;

/// Validates a list of spend requests by checking UTXO uniqueness, dust limits, and script consistency.
///
/// This function performs multiple checks to ensure that the spend requests are valid:
/// 1. Ensures that the witness data is not empty.
/// 2. Validates that UTXOs are unique across the spend requests.
/// 3. Checks that UTXOs meet the dust limit criteria.
/// 4. Verifies that each UTXO’s value matches the expected value in the transaction.
/// 5. Ensures that each UTXO's script public key matches the expected script.
/// 6. Ensures that each UTXO has not already been spent.
/// 7. Fetches and validates transactions and outspends from the indexer to ensure consistency.
///
/// # Arguments
/// * `spend_requests` - A slice of `SpendRequest` objects that need to be validated.
/// * `indexer` - A reference to the `ArcIndexer` used to fetch transaction and outspend data.
///
/// # Returns
/// * `(Vec<SpendRequest>, Vec<SpendRequest>)` returns a tuple of (valid_spend_requests, invalid_spend_requests).
pub async fn validate_spend_requests(
    spend_requests: &[SpendRequest],
    indexer: &ArcIndexer,
) -> (Vec<SpendRequest>, Vec<SpendRequest>) {
    let mut valid_spend_requests = Vec::with_capacity(spend_requests.len());
    let mut invalid_spend_requests = Vec::with_capacity(spend_requests.len());

    // Validate that spend requests are not empty.
    if spend_requests.is_empty() {
        return (vec![], vec![]);
    }

    let mut tx_cache = HashMap::new();
    let mut outspends_cache = HashMap::new();
    let mut utxo_set = HashSet::new();

    for spend_request in spend_requests.iter() {
        let expected_script_pubkey = spend_request.htlc_address.script_pubkey();

        let mut is_valid = true;

        // Validate witness length, since it's a quick check
        if spend_request.witness.is_empty() {
            warn!("Spend request : {} has empty witness", spend_request.id);
            is_valid = false;
        } else {
            // Validate UTXOs only if witness is not empty
            for utxo in spend_request.utxos.iter() {
                let utxo_id = utxo.to_string();

                // Validate utxos are unique
                if !utxo_set.insert(utxo_id.clone()) {
                    warn!(
                        "Duplicate UTXO found in spend request : {} {}",
                        spend_request.id, utxo_id
                    );
                    is_valid = false;
                    break;
                }

                // Check dust limit
                if utxo.value <= DUST_LIMIT {
                    warn!(
                        "UTXO value below dust limit in spend request : {} {} (value: {})",
                        spend_request.id, utxo_id, utxo.value
                    );
                    is_valid = false;
                    break;
                }

                // Fetch transaction if not cached
                if !tx_cache.contains_key(&utxo.txid) {
                    let Ok(tx) = indexer.get_tx_hex(&utxo.txid.to_string()).await else {
                        warn!(
                            "Failed to find transaction for UTXO {} in spend request : {}",
                            utxo_id, spend_request.id
                        );
                        is_valid = false;
                        break;
                    };
                    tx_cache.insert(utxo.txid, tx);
                }

                let Some(tx) = tx_cache.get(&utxo.txid) else {
                    warn!(
                        "Failed to find transaction for UTXO {} in spend request : {}",
                        utxo_id, spend_request.id
                    );
                    is_valid = false;
                    break;
                };

                let Some(output) = tx.output.get(utxo.vout as usize) else {
                    warn!(
                        "UTXO {} not found in spend request : {}",
                        utxo_id, spend_request.id
                    );
                    is_valid = false;
                    break;
                };

                // Validate output value and utxo value
                if output.value.to_sat() != utxo.value {
                    warn!(
                        "UTXO {} value mismatch in spend request : {} : expected {}, got {}",
                        utxo_id,
                        spend_request.id,
                        output.value.to_sat(),
                        utxo.value
                    );
                    is_valid = false;
                    break;
                }

                // Validate utxo's script pub key
                if output.script_pubkey != expected_script_pubkey {
                    warn!(
                        "UTXO {} script pub key mismatch in spend request : {}",
                        utxo_id, spend_request.id
                    );
                    is_valid = false;
                    break;
                }

                if !outspends_cache.contains_key(&utxo.txid) {
                    let Ok(outspends) = indexer.get_tx_outspends(&utxo.txid.to_string()).await
                    else {
                        warn!(
                            "Failed to find outspends for UTXO {} in spend request : {}",
                            utxo_id, spend_request.id
                        );
                        is_valid = false;
                        break;
                    };
                    outspends_cache.insert(utxo.txid, outspends);
                }

                let Some(outspends) = outspends_cache.get(&utxo.txid) else {
                    warn!(
                        "UTXO {} outspends are missing in spend request : {}",
                        utxo_id, spend_request.id
                    );
                    is_valid = false;
                    break;
                };

                // Validate outspend is not spent
                let Some(outspend) = outspends.get(utxo.vout as usize) else {
                    warn!(
                        "UTXO {} outspend is missing in spend request : {}",
                        utxo_id, spend_request.id
                    );
                    is_valid = false;
                    break;
                };

                if outspend.spent {
                    warn!(
                        "UTXO {} is already spent in spend request : {}",
                        utxo_id, spend_request.id
                    );
                    is_valid = false;
                    break;
                }
            }
        }

        match is_valid {
            true => valid_spend_requests.push(spend_request.clone()),
            false => invalid_spend_requests.push(spend_request.clone()),
        }
    }

    (valid_spend_requests, invalid_spend_requests)
}

/// Validates that the UTXOs and inputs match for a transaction.
///
/// This function ensures that the number of UTXOs matches the number of inputs, and that the UTXO
/// IDs match the corresponding input previous output TXIDs. This helps ensure consistency between
/// the transaction's inputs and the UTXOs being spent.
///
/// # Arguments
/// * `utxos` - A slice of `Utxo` objects representing the unspent transaction outputs.
/// * `inputs` - A slice of `TxIn` objects representing the inputs of the transaction.
///
/// # Returns
/// * `Ok(())` if the UTXOs and inputs match.
/// * `Err` if the number of UTXOs and inputs do not match, or if there is a mismatch between
///         the UTXO TXIDs and the input previous output TXIDs.
pub fn validate_utxos_and_inputs(utxos: &[Utxo], inputs: &[TxIn]) -> Result<()> {
    if utxos.len() != inputs.len() {
        bail!("Number of UTXOs and inputs do not match");
    }

    for (utxo, input) in utxos.iter().zip(inputs.iter()) {
        let utxo_id = utxo.to_string();
        let prevout_txid = input.previous_output.to_string();

        if utxo_id != prevout_txid {
            bail!("UTXO txid does not match input txid for UTXO {}", utxo_id);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        batcher::primitives::SpendRequest,
        htlc::tx::{DEFAULT_TX_LOCKTIME, DEFAULT_TX_VERSION, DUST_LIMIT},
        test_utils::{
            generate_bitcoin_random_keypair, get_dummy_txin, get_dummy_utxo, TEST_NETWORK,
        },
        MockIndexer, OutSpend, Utxo, UtxoStatus,
    };
    use bitcoin::{key::Secp256k1, Address, Amount, ScriptBuf, Transaction, TxOut, Txid, Witness};
    use eyre::eyre;
    use mockall::predicate::*;
    use std::{str::FromStr, sync::Arc};

    // Helper function to create a mock transaction
    fn create_mock_transaction(value: u64, script_pubkey: &ScriptBuf) -> Transaction {
        Transaction {
            version: DEFAULT_TX_VERSION,
            lock_time: DEFAULT_TX_LOCKTIME,
            input: vec![],
            output: vec![TxOut {
                value: Amount::from_sat(value),
                script_pubkey: script_pubkey.clone(),
            }],
        }
    }

    // Helper function to create a mock outspend response
    fn create_mock_outspend(spent: bool) -> OutSpend {
        OutSpend {
            spent: spent,
            txid: if spent {
                Some("some_spending_txid".to_string())
            } else {
                None
            },
        }
    }

    fn setup_basic_test_data() -> (SpendRequest, ScriptBuf) {
        let secp = Secp256k1::new();
        let keypair = generate_bitcoin_random_keypair();
        let internal_key = keypair.x_only_public_key().0;

        let htlc_internal_key = generate_bitcoin_random_keypair().x_only_public_key().0;

        let recipient = Address::p2tr(&secp, internal_key, None, TEST_NETWORK);
        let htlc_address = Address::p2tr(&secp, htlc_internal_key, None, TEST_NETWORK); // Simplified for test

        let script_pubkey = htlc_address.script_pubkey();

        let utxo = Utxo {
            txid: Txid::from_str(
                "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            )
            .unwrap(),
            vout: 0,
            value: 100000, // Above dust limit
            status: UtxoStatus {
                confirmed: true,
                block_height: Some(100),
            },
        };

        let witness = Witness::from_slice(&[b"test_witness"]);

        let spend_request = SpendRequest {
            htlc_address: htlc_address.clone(),
            id: htlc_address.to_string(),
            keypair,
            recipient,
            script: ScriptBuf::new(), // Simplified for test
            utxos: vec![utxo],
            witness,
        };

        (spend_request, script_pubkey)
    }

    #[tokio::test]
    async fn test_validate_spend_request_success() -> Result<()> {
        let (spend_request, script_pubkey) = setup_basic_test_data();

        let mut mock_indexer = MockIndexer::new();

        let txid_str = spend_request.utxos[0].txid.to_string();
        let mock_tx = create_mock_transaction(100000, &script_pubkey);

        // Mock successful transaction fetch
        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx.clone()));

        // Mock outspends - UTXO is not spent
        let outspend = create_mock_outspend(false);
        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid_str))
            .times(1)
            .returning(move |_| Ok(vec![outspend.clone()]));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) = validate_spend_requests(&[spend_request], &arc_indexer).await;
        assert!(valid.len() == 1);
        assert!(invalid.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_tx_not_found() -> Result<()> {
        let (spend_request, _) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        let txid_str = spend_request.utxos[0].txid.to_string();

        // Mock transaction fetch failure
        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str))
            .times(1)
            .returning(|_| Err(eyre!("Transaction not found")));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == spend_request.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_empty_witness() -> Result<()> {
        let (mut spend_request, _) = setup_basic_test_data();
        let mock_indexer = MockIndexer::new();

        // Set empty witness
        spend_request.witness = Witness::new();

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == spend_request.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_value_mismatch() -> Result<()> {
        let (mut spend_request, script_pubkey) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        let txid_str = spend_request.utxos[0].txid.to_string();

        // Transaction has different value than UTXO claims
        let mock_tx = create_mock_transaction(50000, &script_pubkey);
        spend_request.utxos[0].value = 100000;

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx.clone()));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == spend_request.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_script_pubkey_mismatch() -> Result<()> {
        let (spend_request, _) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        let txid_str = spend_request.utxos[0].txid.to_string();

        // Transaction has different script_pubkey
        let wrong_script_pubkey = spend_request.recipient.script_pubkey();
        let mock_tx = create_mock_transaction(100000, &wrong_script_pubkey);

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx.clone()));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == spend_request.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_utxo_validations() -> Result<()> {
        let (spend_request, script_pubkey) = setup_basic_test_data();

        let mut mock_indexer = MockIndexer::new();

        let txid = spend_request.utxos[0].txid.to_string();
        let mock_tx = create_mock_transaction(100000, &script_pubkey);

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid.clone()))
            .returning(move |_| Ok(mock_tx.clone()));

        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid.clone()))
            .returning(|_| Ok(vec![create_mock_outspend(false)]));

        // Add duplicate UTXO
        let mut duplicate_utxo_spend_requst = spend_request.clone();
        let duplicate_utxo = spend_request.utxos[0].clone();
        duplicate_utxo_spend_requst.utxos.push(duplicate_utxo);

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[duplicate_utxo_spend_requst.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == duplicate_utxo_spend_requst.id);

        // Set UTXO value below dust limit
        let mut below_dust_limit_utxo_spend_request = spend_request.clone();
        below_dust_limit_utxo_spend_request.utxos[0].value = DUST_LIMIT - 1;

        let (valid, invalid) =
            validate_spend_requests(&[below_dust_limit_utxo_spend_request.clone()], &arc_indexer)
                .await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == below_dust_limit_utxo_spend_request.id);

        // Set invalid vout (transaction only has output at index 0)
        let mut invalid_vout_spend_request = spend_request.clone();
        invalid_vout_spend_request.utxos[0].vout = 1;

        let (valid, invalid) =
            validate_spend_requests(&[invalid_vout_spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == invalid_vout_spend_request.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_already_spent() -> Result<()> {
        let (spend_request, script_pubkey) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        let txid_str = spend_request.utxos[0].txid.to_string();
        let mock_tx = create_mock_transaction(100000, &script_pubkey);

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx.clone()));

        // Mock outspends - UTXO is already spent
        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid_str))
            .times(1)
            .returning(|_| Ok(vec![create_mock_outspend(true)]));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);
        assert!(invalid[0].id == spend_request.id);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_outspend_missing() -> Result<()> {
        let (spend_request, script_pubkey) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        let txid_str = spend_request.utxos[0].txid.to_string();
        let mock_tx = create_mock_transaction(100000, &script_pubkey);

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx.clone()));

        // Mock outspends - empty array (outspend missing)
        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid_str))
            .times(1)
            .returning(|_| Ok(vec![]));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) =
            validate_spend_requests(&[spend_request.clone()], &arc_indexer).await;
        assert!(valid.is_empty());
        assert!(invalid.len() == 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_multiple_utxos_success() -> Result<()> {
        let (mut spend_request, script_pubkey) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        // Add second UTXO
        let second_utxo = Utxo {
            txid: Txid::from_str(
                "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890",
            )
            .unwrap(),
            vout: 0,
            value: 200000,
            status: UtxoStatus {
                confirmed: true,
                block_height: Some(100),
            },
        };
        spend_request.utxos.push(second_utxo);

        // Mock responses for both transactions
        let txid1_str = spend_request.utxos[0].txid.to_string();
        let txid2_str = spend_request.utxos[1].txid.to_string();

        let mock_tx1 = create_mock_transaction(100000, &script_pubkey);
        let mock_tx2 = create_mock_transaction(200000, &script_pubkey);

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid1_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx1.clone()));

        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid2_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx2.clone()));

        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid1_str))
            .times(1)
            .returning(|_| Ok(vec![create_mock_outspend(false)]));

        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid2_str))
            .times(1)
            .returning(|_| Ok(vec![create_mock_outspend(false)]));

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) = validate_spend_requests(&[spend_request], &arc_indexer).await;
        assert!(valid.len() == 1);
        assert!(invalid.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_spend_request_caching() -> Result<()> {
        let (mut spend_request, script_pubkey) = setup_basic_test_data();
        let mut mock_indexer = MockIndexer::new();

        // Create two UTXOs from the same transaction
        let same_txid = spend_request.utxos[0].txid.clone();
        let second_utxo = Utxo {
            txid: same_txid.clone(),
            vout: 1, // Different output
            value: 150000,
            status: UtxoStatus {
                confirmed: true,
                block_height: Some(100),
            },
        };
        spend_request.utxos.push(second_utxo);

        let txid_str = same_txid.to_string();

        // Create mock transaction with two outputs
        let mut mock_tx = create_mock_transaction(100000, &script_pubkey);
        mock_tx.output.push(TxOut {
            value: Amount::from_sat(150000),
            script_pubkey: script_pubkey.clone(),
        });

        // Mock should be called only once due to caching
        mock_indexer
            .expect_get_tx_hex()
            .with(eq(txid_str.clone()))
            .times(1)
            .returning(move |_| Ok(mock_tx.clone()));

        mock_indexer
            .expect_get_tx_outspends()
            .with(eq(txid_str))
            .times(1)
            .returning(|_| {
                Ok(vec![
                    create_mock_outspend(false),
                    create_mock_outspend(false),
                ])
            });

        let arc_indexer = Arc::new(mock_indexer) as ArcIndexer;
        let (valid, invalid) = validate_spend_requests(&[spend_request], &arc_indexer).await;
        assert!(valid.len() == 1);
        assert!(invalid.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_validate_utxos_and_inputs() -> Result<()> {
        // Step 1: Test where UTXOs and inputs match in number and txid:vout
        let utxos = vec![get_dummy_utxo()];
        let inputs = vec![get_dummy_txin()];

        let result = validate_utxos_and_inputs(&utxos, &inputs);
        assert!(
            result.is_ok(),
            "Expected validation to pass when UTXOs and inputs match"
        );

        // Step 2: Test where the number of UTXOs and inputs do not match
        let utxos_mismatch = vec![get_dummy_utxo()];
        let inputs_mismatch = vec![get_dummy_txin(), get_dummy_txin()];

        let result = validate_utxos_and_inputs(&utxos_mismatch, &inputs_mismatch);
        assert!(
            result.is_err(),
            "Expected error due to mismatch in number of UTXOs and inputs"
        );

        // Step 3: Test where UTXOs and inputs match in number but txid:vout do not match
        let utxos_mismatch_values = vec![get_dummy_utxo()];
        let mut input = get_dummy_txin();
        input.previous_output.vout = 1; // Changing the vout to mismatch
        let inputs_mismatch_values = vec![input];
        let result = validate_utxos_and_inputs(&utxos_mismatch_values, &inputs_mismatch_values);

        assert!(
            result.is_err(),
            "Expected error due to mismatch in txid:vout"
        );

        // Step 4: Test where both UTXOs and inputs are empty
        let empty_utxos: Vec<Utxo> = Vec::new();
        let empty_inputs: Vec<TxIn> = Vec::new();

        let result = validate_utxos_and_inputs(&empty_utxos, &empty_inputs);
        assert!(
            result.is_ok(),
            "Expected validation to pass when both UTXOs and inputs are empty"
        );

        Ok(())
    }
}
