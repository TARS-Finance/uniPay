//! Generic iterative fee builder for UTXO chains.
//!
//! Vendored from standard-rs but made fully generic over the chain adaptor
//! (standard-rs hardcodes Zcash types).  The algorithm iteratively adjusts
//! change and cover UTXOs until the transaction's embedded fee matches the
//! target fee. Convergence uses encoded transaction size stability so small
//! witness/signature-length variance does not cause pointless extra iterations.

use super::deps::{CoverUtxoProvider, TxBuilderError, UtxoChainTxAdaptor};

/// Maximum number of fee-adjustment iterations before giving up.
const MAX_ITERATIONS: usize = 16;

/// Tolerance for small encoded-size variance between iterations.
const SIZE_TOLERANCE: usize = 2;

/// Result of a successful fee-builder run.
#[derive(Debug)]
pub struct FeeBuilderResult<T> {
    /// The final transaction with the correct fee embedded.
    pub tx: T,
    /// The change amount included in the transaction (may be 0).
    pub change: u64,
    /// The actual fee paid by the returned transaction, in sats.
    pub fee_paid_sats: u64,
}

/// Iterative action chosen after comparing current fee to target fee.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeeAction {
    Done,
    ReduceChange(u64),
    AddUtxos(u64),
    IncreaseChange(u64),
}

/// Ordered fee adjustments to apply before the next iteration.
#[derive(Debug, Clone, PartialEq, Eq)]
struct FeePlan {
    actions: Vec<FeeAction>,
}

fn plan_fee_actions(fee_delta: i64, change: u64, min_change_value: u64) -> FeePlan {
    let mut actions = Vec::new();

    match fee_delta.cmp(&0) {
        std::cmp::Ordering::Equal => {
            if change < min_change_value {
                actions.push(FeeAction::AddUtxos(min_change_value - change));
            } else {
                actions.push(FeeAction::Done);
            }
        },
        std::cmp::Ordering::Less => {
            let mut needed = fee_delta.unsigned_abs();
            let reducible_change = change.saturating_sub(min_change_value);
            if reducible_change > 0 {
                let reduction = reducible_change.min(needed);
                actions.push(FeeAction::ReduceChange(reduction));
                needed -= reduction;
            }
            if needed > 0 {
                actions.push(FeeAction::AddUtxos(needed));
            }
        },
        std::cmp::Ordering::Greater => {
            let increase = fee_delta as u64;
            actions.push(FeeAction::IncreaseChange(increase));
            let projected_change = change.saturating_add(increase);
            if projected_change < min_change_value {
                actions.push(FeeAction::AddUtxos(min_change_value - projected_change));
            }
        },
    }

    FeePlan { actions }
}

fn fallback_best<T, A>(
    adaptor: &A,
    best_overpay: Option<(Vec<u8>, u64, u64, i64)>,
) -> Result<FeeBuilderResult<T>, TxBuilderError>
where
    A: UtxoChainTxAdaptor<Tx = T>,
{
    let Some((data, change, fee_paid_sats, _)) = best_overpay else {
        return Err(TxBuilderError::MaxIterationsExceeded);
    };

    Ok(FeeBuilderResult {
        tx: adaptor.decode(&data)?,
        change,
        fee_paid_sats,
    })
}

/// Iteratively builds a transaction whose embedded fee matches the target fee.
///
/// The algorithm:
/// 1. Build the transaction with the current cover set and change = 0.
/// 2. Compute `current_fee` and `target_fee`.
/// 3. Compute `delta = current_fee - target_fee`.
///    - `delta > 0`: overpaying -> increase change to absorb surplus.
///    - `delta < 0`: underpaying -> reduce change or add more cover UTXOs.
///    - `delta == 0` (within tolerance): done.
/// 4. If the fee cannot converge within [`MAX_ITERATIONS`], return the best
///    overpaying transaction found so far.
async fn build_with_fee_floor<A>(
    adaptor: &A,
    params: &A::Params,
    cover: &mut A::CoverUtxoProvider,
    initial_change: u64,
    min_change_value: u64,
) -> Result<FeeBuilderResult<A::Tx>, TxBuilderError>
where
    A: UtxoChainTxAdaptor,
    A::Utxo: Clone,
    A::CoverUtxoProvider: CoverUtxoProvider<Utxo = A::Utxo, Tx = A::Tx>,
{
    let mut change = initial_change;
    let mut last_size: Option<usize> = None;
    let mut best_overpay: Option<(Vec<u8>, u64, u64, i64)> = None;
    let mut best_overpay_delta: i64 = i64::MAX;

    for iteration in 0..MAX_ITERATIONS {
        let selected = cover.selected().to_vec();
        let tx = adaptor.build(params, &selected, change)?;

        let current_fee = adaptor.current(params, cover, &tx)?;
        let target_fee = adaptor.target(params, &tx)? as i64;
        let delta = current_fee - target_fee;
        let encoded = adaptor.encode(&tx)?;
        let size = encoded.len();
        tracing::debug!(
            iteration = iteration + 1,
            max_iterations = MAX_ITERATIONS,
            selected_cover_utxo_count = selected.len(),
            change,
            current_fee,
            target_fee,
            delta,
            encoded_size = size,
            last_size = ?last_size,
            min_change_value,
            "bitcoin fee builder iteration",
        );

        if let Some(prev_size) = last_size
            && size > prev_size
            && size - prev_size <= SIZE_TOLERANCE
        {
            tracing::debug!(
                iteration = iteration + 1,
                previous_size = prev_size,
                current_size = size,
                size_tolerance = SIZE_TOLERANCE,
                "bitcoin fee builder accepted tx by size-stability tolerance",
            );
            return Ok(FeeBuilderResult {
                tx,
                change,
                fee_paid_sats: u64::try_from(current_fee).map_err(|_| {
                    TxBuilderError::Client(
                        "fee builder produced negative current fee on converged tx".into(),
                    )
                })?,
            });
        }
        last_size = Some(size);

        if delta > 0 && delta < best_overpay_delta {
            best_overpay_delta = delta;
            best_overpay = Some((
                encoded.clone(),
                change,
                u64::try_from(current_fee).map_err(|_| {
                    TxBuilderError::Client(
                        "fee builder recorded negative fee for best overpay candidate".into(),
                    )
                })?,
                delta,
            ));
            tracing::debug!(
                iteration = iteration + 1,
                change,
                fee_paid_sats = current_fee,
                overpay_delta = delta,
                "bitcoin fee builder recorded best overpay candidate",
            );
        }

        let plan = plan_fee_actions(delta, change, min_change_value);
        tracing::debug!(
            iteration = iteration + 1,
            ?plan,
            "bitcoin fee builder planned fee adjustment actions",
        );

        for action in plan.actions {
            match action {
                FeeAction::Done => {
                    tracing::debug!(
                        iteration = iteration + 1,
                        change,
                        fee_paid_sats = current_fee,
                        "bitcoin fee builder converged exactly",
                    );
                    return Ok(FeeBuilderResult {
                        tx,
                        change,
                        fee_paid_sats: u64::try_from(current_fee).map_err(|_| {
                            TxBuilderError::Client(
                                "fee builder finished with negative current fee".into(),
                            )
                        })?,
                    });
                },
                FeeAction::ReduceChange(amount) => {
                    tracing::debug!(
                        iteration = iteration + 1,
                        reduce_change_by = amount,
                        previous_change = change,
                        new_change = change.saturating_sub(amount),
                        "bitcoin fee builder reducing change",
                    );
                    change = change.saturating_sub(amount);
                },
                FeeAction::AddUtxos(needed) => {
                    let available = cover.available(&tx).await?;
                    tracing::debug!(
                        iteration = iteration + 1,
                        needed,
                        available_candidate_count = available.len(),
                        "bitcoin fee builder requesting additional cover utxos",
                    );
                    let extra = match cover.select(available, needed) {
                        Ok(extra) => extra,
                        Err(TxBuilderError::InsufficientFunds { .. }) if best_overpay.is_some() => {
                            tracing::debug!(
                                iteration = iteration + 1,
                                needed,
                                "bitcoin fee builder fell back to best overpay candidate after insufficient funds",
                            );
                            return fallback_best(adaptor, best_overpay);
                        },
                        Err(err) => return Err(err),
                    };
                    if extra.is_empty() {
                        tracing::debug!(
                            iteration = iteration + 1,
                            needed,
                            "bitcoin fee builder selector returned no extra utxos",
                        );
                        return if best_overpay.is_some() {
                            fallback_best(adaptor, best_overpay)
                        } else {
                            Err(TxBuilderError::InsufficientFunds {
                                needed,
                                available: 0,
                            })
                        };
                    }
                    tracing::debug!(
                        iteration = iteration + 1,
                        added_cover_utxo_count = extra.len(),
                        "bitcoin fee builder added cover utxos",
                    );
                    cover.add(extra);
                },
                FeeAction::IncreaseChange(amount) => {
                    tracing::debug!(
                        iteration = iteration + 1,
                        increase_change_by = amount,
                        previous_change = change,
                        new_change = change.saturating_add(amount),
                        "bitcoin fee builder increasing change",
                    );
                    change = change.saturating_add(amount);
                },
            }
        }
    }

    fallback_best(adaptor, best_overpay)
}

/// Iteratively builds a transaction whose embedded fee matches the target fee.
pub async fn build_with_fee<A>(
    adaptor: &A,
    params: &A::Params,
    cover: &mut A::CoverUtxoProvider,
) -> Result<FeeBuilderResult<A::Tx>, TxBuilderError>
where
    A: UtxoChainTxAdaptor,
    A::Utxo: Clone,
    A::CoverUtxoProvider: CoverUtxoProvider<Utxo = A::Utxo, Tx = A::Tx>,
{
    build_with_fee_floor(adaptor, params, cover, 0, 0).await
}

/// Iteratively builds a transaction that also enforces a minimum change floor.
pub async fn build_with_min_change<A>(
    adaptor: &A,
    params: &A::Params,
    cover: &mut A::CoverUtxoProvider,
    min_change_value: u64,
) -> Result<FeeBuilderResult<A::Tx>, TxBuilderError>
where
    A: UtxoChainTxAdaptor,
    A::Utxo: Clone,
    A::CoverUtxoProvider: CoverUtxoProvider<Utxo = A::Utxo, Tx = A::Tx>,
{
    build_with_fee_floor(adaptor, params, cover, min_change_value, min_change_value).await
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #![allow(clippy::assertions_on_constants)]

    use super::*;
    use crate::infrastructure::chain::bitcoin::tx_builder::deps::UtxoChainTxFeeEstimator;
    use async_trait::async_trait;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    // Verify constants are sensible.
    #[test]
    fn max_iterations_is_positive() {
        assert!(MAX_ITERATIONS > 0);
    }

    #[test]
    fn size_tolerance_is_small() {
        assert!(SIZE_TOLERANCE > 0);
        assert!(SIZE_TOLERANCE <= 10);
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct FakeUtxo {
        value: u64,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct FakeTx {
        cover_total: u64,
        cover_count: usize,
        change: u64,
    }

    #[derive(Clone, Debug)]
    struct FakeParams {
        spend_total: u64,
        target_fee: u64,
    }

    struct FakeProvider {
        selected: Vec<FakeUtxo>,
        available: VecDeque<Vec<FakeUtxo>>,
    }

    impl FakeProvider {
        fn new(selected: Vec<FakeUtxo>, available: Vec<Vec<FakeUtxo>>) -> Self {
            Self {
                selected,
                available: available.into(),
            }
        }
    }

    #[async_trait]
    impl CoverUtxoProvider for FakeProvider {
        type Utxo = FakeUtxo;
        type Tx = FakeTx;

        fn selected(&self) -> &[Self::Utxo] {
            &self.selected
        }

        fn add(&mut self, utxos: Vec<Self::Utxo>) {
            self.selected.extend(utxos);
        }

        async fn available(&self, _tx: &Self::Tx) -> Result<Vec<Self::Utxo>, TxBuilderError> {
            let available = self
                .available
                .front()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|candidate| !self.selected.iter().any(|selected| selected == candidate))
                .collect();
            Ok(available)
        }

        fn select(
            &self,
            utxos: Vec<Self::Utxo>,
            needed: u64,
        ) -> Result<Vec<Self::Utxo>, TxBuilderError> {
            let mut chosen = Vec::new();
            let mut total = 0;
            for utxo in utxos {
                total += utxo.value;
                chosen.push(utxo);
                if total >= needed {
                    return Ok(chosen);
                }
            }

            Err(TxBuilderError::InsufficientFunds {
                needed,
                available: total,
            })
        }
    }

    struct FakeAdaptor;

    impl super::super::deps::UtxoChainTxCodec for FakeAdaptor {
        type Tx = FakeTx;

        fn encode(&self, _tx: &Self::Tx) -> Result<Vec<u8>, TxBuilderError> {
            Ok(vec![])
        }

        fn decode(&self, _data: &[u8]) -> Result<Self::Tx, TxBuilderError> {
            unreachable!("not used")
        }
    }

    impl super::super::deps::UtxoChainTxFeeEstimator for FakeAdaptor {
        type Params = FakeParams;
        type CoverUtxoProvider = FakeProvider;

        fn current(
            &self,
            params: &Self::Params,
            _cover: &Self::CoverUtxoProvider,
            tx: &Self::Tx,
        ) -> Result<i64, TxBuilderError> {
            Ok(tx.cover_total as i64 - params.spend_total as i64 - tx.change as i64)
        }

        fn target(&self, params: &Self::Params, tx: &Self::Tx) -> Result<u64, TxBuilderError> {
            Ok(params.target_fee + tx.cover_count as u64 * 500)
        }
    }

    impl super::super::deps::UtxoChainTxAdaptor for FakeAdaptor {
        type Utxo = FakeUtxo;

        fn build(
            &self,
            _params: &Self::Params,
            cover_utxos: &[Self::Utxo],
            change: u64,
        ) -> Result<Self::Tx, TxBuilderError> {
            Ok(FakeTx {
                cover_total: cover_utxos.iter().map(|utxo| utxo.value).sum(),
                cover_count: cover_utxos.len(),
                change,
            })
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct SizedTx {
        change: u64,
    }

    struct SizedAdaptor;

    struct SizedProvider;

    #[async_trait]
    impl CoverUtxoProvider for SizedProvider {
        type Utxo = FakeUtxo;
        type Tx = SizedTx;

        fn selected(&self) -> &[Self::Utxo] {
            &[]
        }

        fn add(&mut self, _utxos: Vec<Self::Utxo>) {}

        async fn available(&self, _tx: &Self::Tx) -> Result<Vec<Self::Utxo>, TxBuilderError> {
            Ok(vec![])
        }

        fn select(
            &self,
            _utxos: Vec<Self::Utxo>,
            needed: u64,
        ) -> Result<Vec<Self::Utxo>, TxBuilderError> {
            Err(TxBuilderError::InsufficientFunds {
                needed,
                available: 0,
            })
        }
    }

    impl super::super::deps::UtxoChainTxCodec for SizedAdaptor {
        type Tx = SizedTx;

        fn encode(&self, tx: &Self::Tx) -> Result<Vec<u8>, TxBuilderError> {
            Ok(vec![0; 100 + usize::from(tx.change > 0)])
        }

        fn decode(&self, _data: &[u8]) -> Result<Self::Tx, TxBuilderError> {
            unreachable!("not used")
        }
    }

    impl super::super::deps::UtxoChainTxFeeEstimator for SizedAdaptor {
        type Params = ();
        type CoverUtxoProvider = SizedProvider;

        fn current(
            &self,
            _params: &Self::Params,
            _cover: &Self::CoverUtxoProvider,
            _tx: &Self::Tx,
        ) -> Result<i64, TxBuilderError> {
            Ok(10)
        }

        fn target(&self, _params: &Self::Params, _tx: &Self::Tx) -> Result<u64, TxBuilderError> {
            Ok(9)
        }
    }

    impl super::super::deps::UtxoChainTxAdaptor for SizedAdaptor {
        type Utxo = FakeUtxo;

        fn build(
            &self,
            _params: &Self::Params,
            _cover_utxos: &[Self::Utxo],
            change: u64,
        ) -> Result<Self::Tx, TxBuilderError> {
            Ok(SizedTx { change })
        }
    }

    struct MinChangePlanAdaptor;

    impl super::super::deps::UtxoChainTxCodec for MinChangePlanAdaptor {
        type Tx = FakeTx;

        fn encode(&self, _tx: &Self::Tx) -> Result<Vec<u8>, TxBuilderError> {
            Ok(vec![0; 100])
        }

        fn decode(&self, _data: &[u8]) -> Result<Self::Tx, TxBuilderError> {
            unreachable!("not used")
        }
    }

    impl super::super::deps::UtxoChainTxFeeEstimator for MinChangePlanAdaptor {
        type Params = FakeParams;
        type CoverUtxoProvider = FakeProvider;

        fn current(
            &self,
            params: &Self::Params,
            _cover: &Self::CoverUtxoProvider,
            tx: &Self::Tx,
        ) -> Result<i64, TxBuilderError> {
            Ok(tx.cover_total as i64 - params.spend_total as i64 - tx.change as i64)
        }

        fn target(&self, _params: &Self::Params, tx: &Self::Tx) -> Result<u64, TxBuilderError> {
            Ok(if tx.change > 10_000 { 700 } else { 500 })
        }
    }

    impl super::super::deps::UtxoChainTxAdaptor for MinChangePlanAdaptor {
        type Utxo = FakeUtxo;

        fn build(
            &self,
            _params: &Self::Params,
            cover_utxos: &[Self::Utxo],
            change: u64,
        ) -> Result<Self::Tx, TxBuilderError> {
            Ok(FakeTx {
                cover_total: cover_utxos.iter().map(|utxo| utxo.value).sum(),
                cover_count: cover_utxos.len(),
                change,
            })
        }
    }

    struct RecordingMinChangeAdaptor {
        seen_changes: Arc<Mutex<Vec<u64>>>,
    }

    impl super::super::deps::UtxoChainTxCodec for RecordingMinChangeAdaptor {
        type Tx = FakeTx;

        fn encode(&self, _tx: &Self::Tx) -> Result<Vec<u8>, TxBuilderError> {
            Ok(vec![0; 100])
        }

        fn decode(&self, _data: &[u8]) -> Result<Self::Tx, TxBuilderError> {
            unreachable!("not used")
        }
    }

    impl super::super::deps::UtxoChainTxFeeEstimator for RecordingMinChangeAdaptor {
        type Params = FakeParams;
        type CoverUtxoProvider = FakeProvider;

        fn current(
            &self,
            params: &Self::Params,
            _cover: &Self::CoverUtxoProvider,
            tx: &Self::Tx,
        ) -> Result<i64, TxBuilderError> {
            Ok(tx.cover_total as i64 - params.spend_total as i64 - tx.change as i64)
        }

        fn target(&self, params: &Self::Params, tx: &Self::Tx) -> Result<u64, TxBuilderError> {
            Ok(params.target_fee + tx.cover_count as u64 * 500)
        }
    }

    impl super::super::deps::UtxoChainTxAdaptor for RecordingMinChangeAdaptor {
        type Utxo = FakeUtxo;

        fn build(
            &self,
            _params: &Self::Params,
            cover_utxos: &[Self::Utxo],
            change: u64,
        ) -> Result<Self::Tx, TxBuilderError> {
            self.seen_changes.lock().expect("changes lock").push(change);
            Ok(FakeTx {
                cover_total: cover_utxos.iter().map(|utxo| utxo.value).sum(),
                cover_count: cover_utxos.len(),
                change,
            })
        }
    }

    struct FallbackBestAdaptor;

    impl super::super::deps::UtxoChainTxCodec for FallbackBestAdaptor {
        type Tx = FakeTx;

        fn encode(&self, tx: &Self::Tx) -> Result<Vec<u8>, TxBuilderError> {
            let mut data = Vec::with_capacity(24);
            data.extend_from_slice(&tx.cover_total.to_le_bytes());
            data.extend_from_slice(&(tx.cover_count as u64).to_le_bytes());
            data.extend_from_slice(&tx.change.to_le_bytes());
            Ok(data)
        }

        fn decode(&self, data: &[u8]) -> Result<Self::Tx, TxBuilderError> {
            if data.len() != 24 {
                return Err(TxBuilderError::Validation(
                    "fallback fixture received unexpected tx encoding".into(),
                ));
            }

            let cover_total = u64::from_le_bytes(data[0..8].try_into().expect("cover_total"));
            let cover_count =
                u64::from_le_bytes(data[8..16].try_into().expect("cover_count")) as usize;
            let change = u64::from_le_bytes(data[16..24].try_into().expect("change"));

            Ok(FakeTx {
                cover_total,
                cover_count,
                change,
            })
        }
    }

    impl super::super::deps::UtxoChainTxFeeEstimator for FallbackBestAdaptor {
        type Params = FakeParams;
        type CoverUtxoProvider = FakeProvider;

        fn current(
            &self,
            params: &Self::Params,
            _cover: &Self::CoverUtxoProvider,
            tx: &Self::Tx,
        ) -> Result<i64, TxBuilderError> {
            Ok(tx.cover_total as i64 - params.spend_total as i64 - tx.change as i64)
        }

        fn target(&self, _params: &Self::Params, tx: &Self::Tx) -> Result<u64, TxBuilderError> {
            Ok(if tx.change == 0 { 14_000 } else { 17_000 })
        }
    }

    impl super::super::deps::UtxoChainTxAdaptor for FallbackBestAdaptor {
        type Utxo = FakeUtxo;

        fn build(
            &self,
            _params: &Self::Params,
            cover_utxos: &[Self::Utxo],
            change: u64,
        ) -> Result<Self::Tx, TxBuilderError> {
            Ok(FakeTx {
                cover_total: cover_utxos.iter().map(|utxo| utxo.value).sum(),
                cover_count: cover_utxos.len(),
                change,
            })
        }
    }

    #[tokio::test]
    async fn min_change_adds_extra_cover_and_converges() {
        let adaptor = FakeAdaptor;
        let params = FakeParams {
            spend_total: 4_000,
            target_fee: 1_000,
        };
        let mut cover = FakeProvider::new(
            vec![FakeUtxo { value: 14_000 }],
            vec![vec![FakeUtxo { value: 9_000 }]],
        );

        let result = build_with_min_change(&adaptor, &params, &mut cover, 10_000)
            .await
            .expect("min change build");

        assert_eq!(
            cover.selected().len(),
            2,
            "builder should add one more cover input"
        );
        assert!(
            result.change >= 10_000,
            "resulting change must satisfy the minimum change floor"
        );
        let embedded_fee = adaptor
            .current(&params, &cover, &result.tx)
            .expect("current fee");
        let target_fee = adaptor.target(&params, &result.tx).expect("target fee") as i64;
        assert!(
            (embedded_fee - target_fee).abs() <= SIZE_TOLERANCE as i64,
            "fee builder should still converge after adding extra cover"
        );
        assert_eq!(result.fee_paid_sats, embedded_fee as u64);
    }

    #[tokio::test]
    async fn min_change_returns_explicit_error_when_no_extra_cover_exists() {
        let adaptor = FakeAdaptor;
        let params = FakeParams {
            spend_total: 4_000,
            target_fee: 1_000,
        };
        let mut cover = FakeProvider::new(vec![FakeUtxo { value: 14_000 }], vec![vec![]]);

        let err = build_with_min_change(&adaptor, &params, &mut cover, 10_000)
            .await
            .expect_err("expected insufficient funds");

        assert!(
            matches!(err, TxBuilderError::InsufficientFunds { .. }),
            "missing extra cover should fail explicitly, got {err:?}"
        );
    }

    #[tokio::test]
    async fn build_with_fee_accepts_small_size_increase_within_tolerance() {
        let adaptor = SizedAdaptor;
        let mut cover = SizedProvider;

        let result = build_with_fee(&adaptor, &(), &mut cover)
            .await
            .expect("size-tolerant build");

        assert_eq!(
            result.change, 1,
            "builder should accept the new tx once encoded size only increases within tolerance",
        );
    }

    #[tokio::test]
    async fn min_change_reduces_excess_change_before_adding_cover() {
        let adaptor = MinChangePlanAdaptor;
        let params = FakeParams {
            spend_total: 4_000,
            target_fee: 0,
        };
        let mut cover = FakeProvider::new(
            vec![FakeUtxo { value: 15_000 }],
            vec![vec![FakeUtxo { value: 1_000 }]],
        );

        let result = build_with_min_change(&adaptor, &params, &mut cover, 10_000)
            .await
            .expect("min change build");

        assert_eq!(
            cover.selected().len(),
            1,
            "builder should trim change above the floor before selecting another cover UTXO",
        );
        assert_eq!(result.change, 10_300);
    }

    #[tokio::test]
    async fn min_change_never_builds_candidate_below_floor() {
        let seen_changes = Arc::new(Mutex::new(Vec::new()));
        let adaptor = RecordingMinChangeAdaptor {
            seen_changes: Arc::clone(&seen_changes),
        };
        let params = FakeParams {
            spend_total: 4_000,
            target_fee: 1_000,
        };
        let mut cover = FakeProvider::new(
            vec![FakeUtxo { value: 14_000 }],
            vec![vec![FakeUtxo { value: 9_000 }]],
        );

        let result = build_with_min_change(&adaptor, &params, &mut cover, 10_000)
            .await
            .expect("min change build");

        assert!(result.change >= 10_000);
        let seen = seen_changes.lock().expect("changes lock");
        assert!(
            seen.iter().all(|change| *change >= 10_000),
            "every candidate build in the min-change path must keep change at or above the floor, saw {seen:?}",
        );
    }

    #[tokio::test]
    async fn build_with_fee_falls_back_to_best_overpay_when_selection_cannot_cover_shortfall() {
        let adaptor = FallbackBestAdaptor;
        let params = FakeParams {
            spend_total: 0,
            target_fee: 0,
        };
        let mut cover = FakeProvider::new(
            vec![FakeUtxo { value: 15_000 }],
            vec![vec![FakeUtxo { value: 1_000 }]],
        );

        let result = build_with_fee(&adaptor, &params, &mut cover)
            .await
            .expect("best overpay fallback");

        assert_eq!(
            result.change, 0,
            "builder should return the previously recorded best overpaying candidate",
        );
        assert_eq!(result.tx.cover_total, 15_000);
        assert_eq!(result.fee_paid_sats, 15_000);
    }

    #[tokio::test]
    async fn fee_builder_result_reports_actual_fee_paid() {
        let adaptor = FakeAdaptor;
        let params = FakeParams {
            spend_total: 4_000,
            target_fee: 1_000,
        };
        let mut cover = FakeProvider::new(vec![FakeUtxo { value: 15_000 }], vec![]);

        let result = build_with_fee(&adaptor, &params, &mut cover)
            .await
            .expect("fee build");

        let embedded_fee = adaptor
            .current(&params, &cover, &result.tx)
            .expect("current fee");
        assert_eq!(result.fee_paid_sats, embedded_fee as u64);
    }
}
