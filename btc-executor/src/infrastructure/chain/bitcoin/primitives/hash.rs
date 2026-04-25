//! Sighash generation for Taproot script path spending.
//!
//! Vendored from garden-rs `crates/bitcoin/src/htlc/hash.rs`, adapted to
//! use local error types instead of `eyre`.

use super::error::BitcoinPrimitivesError;
use bitcoin::{
    hashes::Hash,
    sighash::{Prevouts, SighashCache},
    TapLeafHash, TapSighashType, Transaction, TxOut,
};

/// Generates signature hashes for Taproot script path spending.
///
/// This struct is used to generate signature message hashes for each transaction
/// input using the specified sighash type. These hashes are used for creating
/// Schnorr signatures for Taproot script path spending.
#[derive(Clone, Debug)]
pub struct TapScriptSpendSigHashGenerator {
    tx: Transaction,
    leaf_hash: TapLeafHash,
}

impl TapScriptSpendSigHashGenerator {
    /// Create a new `TapScriptSpendSigHashGenerator` instance.
    ///
    /// # Arguments
    /// * `tx` - Transaction to generate hashes for
    /// * `leaf_hash` - Taproot script leaf hash for the spending path
    pub fn new(tx: Transaction, leaf_hash: TapLeafHash) -> Self {
        Self { tx, leaf_hash }
    }

    /// Generate a signature hash for a single input.
    ///
    /// # Arguments
    /// * `input_index` - Index of the input to generate the hash for
    /// * `prevouts` - Previous output information for the input
    /// * `sighash_type` - Sighash type of the input's signature
    ///
    /// # Errors
    /// Returns `BitcoinPrimitivesError::Sighash` if computation fails.
    fn generate(
        &mut self,
        input_index: usize,
        prevouts: &Prevouts<TxOut>,
        sighash_type: TapSighashType,
    ) -> Result<[u8; 32], BitcoinPrimitivesError> {
        let mut sighash_cache = SighashCache::new(&mut self.tx);
        let sighash = sighash_cache
            .taproot_script_spend_signature_hash(
                input_index,
                prevouts,
                self.leaf_hash,
                sighash_type,
            )
            .map_err(|e| {
                BitcoinPrimitivesError::Sighash(format!(
                    "Failed to generate signature hash for input {input_index}: {e}"
                ))
            })?;

        Ok(sighash.to_raw_hash().to_byte_array())
    }

    /// Generate a signature hash for a single input with a single previous output.
    ///
    /// # Arguments
    /// * `input_index` - Index of the input to generate the hash for
    /// * `previous_output` - Previous output information for the input
    /// * `sighash_type` - Sighash type of the input's signature
    ///
    /// # Errors
    /// Returns `BitcoinPrimitivesError::Sighash` if computation fails.
    pub fn with_prevout(
        &mut self,
        input_index: usize,
        previous_output: &TxOut,
        sighash_type: TapSighashType,
    ) -> Result<[u8; 32], BitcoinPrimitivesError> {
        let prevouts = Prevouts::One(input_index, previous_output.clone());
        self.generate(input_index, &prevouts, sighash_type)
    }

    /// Generate signature hashes for all inputs with all previous outputs.
    ///
    /// # Arguments
    /// * `previous_outputs` - Previous output information for each input
    /// * `sighash_type` - Sighash type of the inputs' signatures
    ///
    /// # Errors
    /// Returns `BitcoinPrimitivesError::Sighash` if computation fails
    /// or the number of inputs doesn't match previous outputs.
    pub fn with_all_prevouts(
        &mut self,
        previous_outputs: &[TxOut],
        sighash_type: TapSighashType,
    ) -> Result<Vec<[u8; 32]>, BitcoinPrimitivesError> {
        if self.tx.input.len() != previous_outputs.len() {
            return Err(BitcoinPrimitivesError::Sighash(format!(
                "Number of transaction inputs ({}) does not match number of previous outputs ({})",
                self.tx.input.len(),
                previous_outputs.len()
            )));
        }

        let mut sighashes = Vec::with_capacity(previous_outputs.len());
        let prevouts = Prevouts::All(previous_outputs);

        for input_index in 0..self.tx.input.len() {
            sighashes.push(self.generate(input_index, &prevouts, sighash_type)?);
        }

        Ok(sighashes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{
        absolute::LockTime, transaction::Version, Amount, OutPoint, ScriptBuf, Sequence,
        TapLeafHash, TxIn, Txid, Witness,
    };

    fn dummy_txin() -> TxIn {
        TxIn {
            previous_output: OutPoint {
                txid: Txid::from_byte_array([0u8; 32]),
                vout: 0,
            },
            script_sig: ScriptBuf::new(),
            sequence: Sequence::MAX,
            witness: Witness::new(),
        }
    }

    fn dummy_txout() -> TxOut {
        TxOut {
            value: Amount::from_sat(50_000),
            script_pubkey: ScriptBuf::new(),
        }
    }

    #[test]
    fn sighash_is_32_bytes() {
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_txin(), dummy_txin()],
            output: vec![dummy_txout(), dummy_txout()],
        };

        let leaf_hash = TapLeafHash::all_zeros();
        let mut generator = TapScriptSpendSigHashGenerator::new(tx, leaf_hash);

        let sighash = generator
            .with_prevout(0, &dummy_txout(), TapSighashType::SinglePlusAnyoneCanPay)
            .unwrap();
        assert_eq!(sighash.len(), 32, "Sighash must be 32 bytes");
    }

    #[test]
    fn with_all_prevouts_matches_input_count() {
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_txin(), dummy_txin()],
            output: vec![dummy_txout(), dummy_txout()],
        };

        let leaf_hash = TapLeafHash::all_zeros();
        let mut generator = TapScriptSpendSigHashGenerator::new(tx, leaf_hash);

        let prevouts = vec![dummy_txout(), dummy_txout()];
        let sighashes = generator
            .with_all_prevouts(&prevouts, TapSighashType::All)
            .unwrap();

        assert_eq!(sighashes.len(), 2, "Must have sighash per input");
        for sighash in &sighashes {
            assert_eq!(sighash.len(), 32);
        }
    }

    #[test]
    fn mismatched_prevouts_errors() {
        let tx = Transaction {
            version: Version::TWO,
            lock_time: LockTime::ZERO,
            input: vec![dummy_txin(), dummy_txin()],
            output: vec![dummy_txout(), dummy_txout()],
        };

        let leaf_hash = TapLeafHash::all_zeros();
        let mut generator = TapScriptSpendSigHashGenerator::new(tx, leaf_hash);

        let mismatched = vec![dummy_txout()];
        let result = generator.with_all_prevouts(&mismatched, TapSighashType::All);
        assert!(result.is_err(), "Mismatched input/prevout counts must fail");
    }
}
