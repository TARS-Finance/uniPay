//! Batch-planning heuristics for the Bitcoin wallet runtime.
//!
//! The planner decides whether pending requests should become a fresh lineage,
//! replace an existing one via RBF, or stay chained to a confirmed change
//! anchor. It does not build transactions itself; it only chooses which groups
//! the runner should attempt based on cost estimates and config limits.
//!
//! Example:
//!
//! ```text
//! pending.free      = [send_a, send_b]
//! pending.anchored  = [refund_c(anchor=X)]
//! live_lineages     = [L1]
//!
//! choices:
//!   refund_c must stay with anchor X       -> Chained(anchor=X, [refund_c])
//!   send_a/send_b may become:
//!     - Fresh([send_a, send_b])
//!     - Rbf(L1, [send_a, send_b])
//!     - Mixed into the anchor-X group if that total cost is cheaper
//!
//! output:
//!   one action per mandatory anchor group
//!   plus at most one placement for all free requests
//! ```

use super::{
    ChainAnchor, LiveLineage, PendingWalletRequest, WalletConfig, WalletRequestKind,
    WalletRuntimeState,
};

/// High-level execution choices emitted by the planner for one runner tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlannedBatchAction {
    Fresh {
        requests: Vec<PendingWalletRequest>,
    },
    Rbf {
        lineage_id: super::LineageId,
        requests: Vec<PendingWalletRequest>,
    },
    Chained {
        existing_lineage_id: Option<super::LineageId>,
        chain_anchor: ChainAnchor,
        requests: Vec<PendingWalletRequest>,
    },
}

/// Cost model used by the planner to compare fresh, RBF, and chained batches.
pub trait PlannerCostEvaluator {
    fn fresh_cost(&self, requests: &[PendingWalletRequest]) -> Option<u64>;

    fn rbf_cost(&self, lineage: &LiveLineage, requests: &[PendingWalletRequest]) -> Option<u64>;

    fn chained_cost(
        &self,
        chain_anchor: &ChainAnchor,
        existing_lineage: Option<&LiveLineage>,
        requests: &[PendingWalletRequest],
    ) -> Option<u64>;
}

#[derive(Clone)]
struct CostedAction {
    action: PlannedBatchAction,
    cost: u64,
}

/// Choose the batch actions that should run for the current wallet state.
///
/// Assumptions:
/// - anchored requests must preserve their `ChainAnchor` grouping;
/// - at most one free-request assignment is added after anchored groups are
///   planned, to keep the runtime deterministic and easy to recover;
/// - `PlannerCostEvaluator` returns `None` for plans that are impossible under
///   current fee/UTXO conditions.
///
/// The planner is consumed by `BitcoinWalletRunner::process_tick`. The runner
/// precomputes all candidate costs, then this function makes the deterministic
/// "which grouping wins?" decision without any further chain I/O.
pub fn plan_wallet_batches<E>(
    runtime: &WalletRuntimeState,
    config: &WalletConfig,
    evaluator: &E,
) -> Vec<PlannedBatchAction>
where
    E: PlannerCostEvaluator,
{
    if runtime.pending.is_empty() {
        return Vec::new();
    }

    // Anchored requests are planned first because they are the least flexible:
    // they must remain tied to a specific confirmed change output.
    let anchored_groups = group_anchored_requests(&runtime.pending.anchored);
    if anchored_groups.is_empty() {
        return plan_free_only(runtime, config, evaluator, &runtime.pending.free)
            .into_iter()
            .map(|choice| choice.action)
            .collect();
    }

    let mut planned = Vec::new();
    let mut group_choices = Vec::new();
    let mut base_total_cost = 0u64;

    for (chain_anchor, requests) in anchored_groups {
        // Each anchor group must be feasible on its own before we try to mix in
        // any free requests. If one mandatory chained group is impossible, the
        // runner should skip planning entirely for this tick.
        let Some(choice) = plan_chained_group(runtime, config, evaluator, &chain_anchor, &requests)
        else {
            return Vec::new();
        };
        base_total_cost += choice.cost;
        group_choices.push((chain_anchor, requests, choice.clone()));
        planned.push(choice.action);
    }

    if runtime.pending.free.is_empty() {
        return planned;
    }

    let mut best_assignment: Option<(usize, CostedAction, u64)> = None;

    // Baseline option: free requests stay separate from the anchored groups.
    if let Some(free_choice) = plan_free_only(runtime, config, evaluator, &runtime.pending.free) {
        best_assignment = Some((
            usize::MAX,
            free_choice.clone(),
            base_total_cost + free_choice.cost,
        ));
    }

    for (index, (chain_anchor, anchored_requests, choice)) in group_choices.iter().enumerate() {
        // Alternative option: fold all free requests into exactly one chained
        // group if doing so is cheaper than keeping them separate.
        let mut mixed_requests = anchored_requests.clone();
        mixed_requests.extend(runtime.pending.free.clone());
        let Some(mixed_choice) =
            plan_chained_group(runtime, config, evaluator, chain_anchor, &mixed_requests)
        else {
            continue;
        };

        let total_cost = base_total_cost - choice.cost + mixed_choice.cost;
        let replace = match &best_assignment {
            Some((_, _, best_cost)) => total_cost < *best_cost,
            None => true,
        };
        if replace {
            best_assignment = Some((index, mixed_choice, total_cost));
        }
    }

    if let Some((index, choice, _)) = best_assignment {
        if index == usize::MAX {
            planned.push(choice.action);
        } else {
            planned[index] = choice.action;
        }
    }

    planned
}

/// Plan work for requests that are not tied to any confirmed chain anchor.
fn plan_free_only<E>(
    runtime: &WalletRuntimeState,
    config: &WalletConfig,
    evaluator: &E,
    requests: &[PendingWalletRequest],
) -> Option<CostedAction>
where
    E: PlannerCostEvaluator,
{
    // Output-count is a hard builder constraint, so reject impossible request
    // sets before comparing fresh/RBF costs.
    if !requests_fit_output_cap(requests, config.max_outputs_per_batch) {
        return None;
    }

    let mut choices = Vec::new();

    // Fresh lineages are only legal while we still have concurrency budget.
    if runtime.live_lineages.len() < config.max_concurrent_lineages
        && let Some(cost) = evaluator.fresh_cost(requests)
    {
        choices.push(CostedAction {
            action: PlannedBatchAction::Fresh {
                requests: requests.to_vec(),
            },
            cost,
        });
    }

    // RBF is intentionally disabled: free requests always take the Fresh path
    // so each batch goes out as a standalone transaction.

    cheapest_choice(choices)
}

/// Plan one anchored request group against its matching confirmed chain anchor.
fn plan_chained_group<E>(
    runtime: &WalletRuntimeState,
    config: &WalletConfig,
    evaluator: &E,
    chain_anchor: &ChainAnchor,
    requests: &[PendingWalletRequest],
) -> Option<CostedAction>
where
    E: PlannerCostEvaluator,
{
    if !requests_fit_output_cap(requests, config.max_outputs_per_batch) {
        return None;
    }

    // If a live lineage already owns this anchor, prefer keeping descendants in
    // that lineage; otherwise the chained action will create a new one.
    let existing_lineage = runtime
        .live_lineages
        .values()
        .filter(|lineage| lineage.chain_anchor.as_ref() == Some(chain_anchor))
        .min_by_key(|lineage| lineage.lineage_id.to_string());

    evaluator
        .chained_cost(chain_anchor, existing_lineage, requests)
        .map(|cost| CostedAction {
            action: PlannedBatchAction::Chained {
                existing_lineage_id: existing_lineage.map(|lineage| lineage.lineage_id),
                chain_anchor: chain_anchor.clone(),
                requests: requests.to_vec(),
            },
            cost,
        })
}

/// Return the lowest-cost feasible action, if any.
fn cheapest_choice(mut choices: Vec<CostedAction>) -> Option<CostedAction> {
    choices.sort_by_key(|choice| choice.cost);
    choices.into_iter().next()
}

/// Group pending requests by exact `ChainAnchor`.
///
/// The runner later turns each group into a chained batch so those requests can
/// spend from the same confirmed change output that made them runnable.
fn group_anchored_requests(
    requests: &[PendingWalletRequest],
) -> Vec<(ChainAnchor, Vec<PendingWalletRequest>)> {
    let mut groups: Vec<(ChainAnchor, Vec<PendingWalletRequest>)> = Vec::new();

    for request in requests {
        let Some(chain_anchor) = request.chain_anchor.clone() else {
            continue;
        };

        // Equality is by full anchor payload, so only requests that refer to
        // the same confirmed change output are allowed to share one chained
        // planning group.
        if let Some((_, grouped)) = groups
            .iter_mut()
            .find(|(existing_anchor, _)| *existing_anchor == chain_anchor)
        {
            grouped.push(request.clone());
        } else {
            groups.push((chain_anchor, vec![request.clone()]));
        }
    }

    groups
}

/// True when the request set fits inside the builder's output cap.
fn requests_fit_output_cap(
    requests: &[PendingWalletRequest],
    max_outputs_per_batch: usize,
) -> bool {
    request_output_count(requests) <= max_outputs_per_batch
}

/// Count how many outputs the request set forces the batch builder to produce.
///
/// Regular spends consume an input but may produce no output, whereas sends and
/// SACP spends both reserve output slots.
fn request_output_count(requests: &[PendingWalletRequest]) -> usize {
    requests
        .iter()
        .map(|pending| match pending.request.kind() {
            WalletRequestKind::Send(_) => 1,
            WalletRequestKind::Spend(spend) => usize::from(spend.recipient.is_some()),
        })
        .sum()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]

    use crate::timestamp::Timestamp;
    use bitcoin::hashes::Hash;
    use bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoin::{Address, Network, OutPoint, ScriptBuf, Sequence, TapSighashType, Txid, Witness};
    use std::collections::HashMap;

    use super::{plan_wallet_batches, PlannedBatchAction, PlannerCostEvaluator};
    use crate::infrastructure::chain::bitcoin::wallet::{
        ChainAnchor, CoverUtxo, LineageId, LiveLineage, LiveLineageRequest, PendingState,
        PendingWalletRequest, WalletConfig, WalletRequest, WalletRuntimeState,
    };

    fn regtest_address(seed: u8) -> Address {
        let secp = Secp256k1::new();
        let secret_key = SecretKey::from_slice(&[seed; 32]).expect("secret key");
        let keypair = bitcoin::key::Keypair::from_secret_key(&secp, &secret_key);
        let (xonly, _) = keypair.x_only_public_key();
        Address::p2tr(&secp, xonly, None, Network::Regtest)
    }

    fn send_request(key: &str, seed: u8, amount: u64) -> PendingWalletRequest {
        PendingWalletRequest {
            request: WalletRequest::send(key, regtest_address(seed), amount).expect("send"),
            chain_anchor: None,
            created_at: Timestamp::default(),
        }
    }

    fn anchor(seed: u8, height: u64) -> ChainAnchor {
        let txid = Txid::from_byte_array([seed; 32]);
        ChainAnchor {
            confirmed_txid: txid,
            change_outpoint: OutPoint { txid, vout: 1 },
            change_value: 25_000,
            change_script_pubkey: ScriptBuf::new(),
            confirmed_height: height,
        }
    }

    fn anchored_request(key: &str, seed: u8, anchored_to: ChainAnchor) -> PendingWalletRequest {
        PendingWalletRequest {
            request: WalletRequest::send(key, regtest_address(seed), 11_000).expect("send"),
            chain_anchor: Some(anchored_to),
            created_at: Timestamp::default(),
        }
    }

    fn regular_spend_request(key: &str, seed: u8) -> PendingWalletRequest {
        PendingWalletRequest {
            request: WalletRequest::spend(
                key,
                OutPoint {
                    txid: Txid::from_byte_array([seed; 32]),
                    vout: 0,
                },
                15_000,
                regtest_address(seed).script_pubkey(),
                Witness::new(),
                ScriptBuf::new(),
                bitcoin::taproot::TapLeafHash::all_zeros(),
                Sequence::ENABLE_RBF_NO_LOCKTIME,
                TapSighashType::All,
                None,
            )
            .expect("regular spend"),
            chain_anchor: None,
            created_at: Timestamp::default(),
        }
    }

    fn sacp_spend_request(key: &str, seed: u8) -> PendingWalletRequest {
        PendingWalletRequest {
            request: WalletRequest::spend(
                key,
                OutPoint {
                    txid: Txid::from_byte_array([seed; 32]),
                    vout: 1,
                },
                15_000,
                regtest_address(seed).script_pubkey(),
                Witness::new(),
                ScriptBuf::new(),
                bitcoin::taproot::TapLeafHash::all_zeros(),
                Sequence::ENABLE_RBF_NO_LOCKTIME,
                TapSighashType::SinglePlusAnyoneCanPay,
                Some(crate::infrastructure::chain::bitcoin::wallet::SendRequest {
                    address: regtest_address(seed + 1),
                    amount: 14_000,
                }),
            )
            .expect("sacp spend"),
            chain_anchor: None,
            created_at: Timestamp::default(),
        }
    }

    fn live_lineage(
        lineage_id: LineageId,
        head_seed: u8,
        chain_anchor: Option<ChainAnchor>,
    ) -> LiveLineage {
        let head_txid = Txid::from_byte_array([head_seed; 32]);
        LiveLineage {
            lineage_id,
            head_txid,
            all_txids: vec![head_txid],
            requests: vec![LiveLineageRequest {
                request: WalletRequest::send("live-1", regtest_address(head_seed), 7_000).unwrap(),
                txid_history: vec![head_txid],
                created_at: Timestamp::default(),
            }],
            cover_utxos: vec![CoverUtxo {
                outpoint: OutPoint {
                    txid: Txid::from_byte_array([head_seed + 1; 32]),
                    vout: 0,
                },
                value: 25_000,
                script_pubkey: ScriptBuf::new(),
            }],
            chain_anchor,
        }
    }

    fn runtime_state(
        free: Vec<PendingWalletRequest>,
        anchored: Vec<PendingWalletRequest>,
        live_lineages: Vec<LiveLineage>,
    ) -> WalletRuntimeState {
        WalletRuntimeState {
            live_lineages: live_lineages
                .into_iter()
                .map(|lineage| (lineage.lineage_id, lineage))
                .collect(),
            pending: PendingState { free, anchored },
            missing_observations: HashMap::new(),
            current_height: 870_000,
        }
    }

    #[derive(Default)]
    struct FakePlannerEvaluator {
        fresh: HashMap<String, u64>,
        rbf: HashMap<(LineageId, String), u64>,
        chained: HashMap<(Option<LineageId>, String), u64>,
    }

    impl FakePlannerEvaluator {
        fn with_fresh(mut self, keys: &[&str], cost: u64) -> Self {
            self.fresh.insert(join_keys(keys), cost);
            self
        }

        fn with_rbf(mut self, lineage_id: LineageId, keys: &[&str], cost: u64) -> Self {
            self.rbf.insert((lineage_id, join_keys(keys)), cost);
            self
        }

        fn with_chained(
            mut self,
            existing_lineage_id: Option<LineageId>,
            keys: &[&str],
            cost: u64,
        ) -> Self {
            self.chained
                .insert((existing_lineage_id, join_keys(keys)), cost);
            self
        }
    }

    impl PlannerCostEvaluator for FakePlannerEvaluator {
        fn fresh_cost(&self, requests: &[PendingWalletRequest]) -> Option<u64> {
            self.fresh.get(&request_keys(requests)).copied()
        }

        fn rbf_cost(
            &self,
            lineage: &LiveLineage,
            requests: &[PendingWalletRequest],
        ) -> Option<u64> {
            self.rbf
                .get(&(lineage.lineage_id, request_keys(requests)))
                .copied()
        }

        fn chained_cost(
            &self,
            _chain_anchor: &ChainAnchor,
            existing_lineage: Option<&LiveLineage>,
            requests: &[PendingWalletRequest],
        ) -> Option<u64> {
            self.chained
                .get(&(
                    existing_lineage.map(|lineage| lineage.lineage_id),
                    request_keys(requests),
                ))
                .copied()
        }
    }

    fn request_keys(requests: &[PendingWalletRequest]) -> String {
        let mut keys = requests
            .iter()
            .map(|request| request.request.dedupe_key().to_string())
            .collect::<Vec<_>>();
        keys.sort();
        keys.join("|")
    }

    fn join_keys(keys: &[&str]) -> String {
        let mut keys = keys.iter().map(|key| key.to_string()).collect::<Vec<_>>();
        keys.sort();
        keys.join("|")
    }

    #[test]
    fn returns_no_actions_when_pending_is_empty() {
        let runtime = runtime_state(Vec::new(), Vec::new(), Vec::new());

        let actions = plan_wallet_batches(
            &runtime,
            &WalletConfig::default(),
            &FakePlannerEvaluator::default(),
        );

        assert!(actions.is_empty());
    }

    #[test]
    fn free_pending_always_uses_fresh_even_when_rbf_would_be_cheaper() {
        let lineage_id = LineageId::new();
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 12_000)],
            Vec::new(),
            vec![live_lineage(lineage_id, 9, None)],
        );
        let evaluator = FakePlannerEvaluator::default()
            .with_fresh(&["free-1"], 180)
            .with_rbf(lineage_id, &["free-1"], 90);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Fresh {
                requests: vec![send_request("free-1", 1, 12_000)],
            }]
        );
    }

    #[test]
    fn free_pending_prefers_fresh_when_it_is_cheapest() {
        let first_lineage = LineageId::new();
        let second_lineage = LineageId::new();
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 12_000)],
            Vec::new(),
            vec![
                live_lineage(first_lineage, 9, None),
                live_lineage(second_lineage, 10, None),
            ],
        );
        let evaluator = FakePlannerEvaluator::default()
            .with_fresh(&["free-1"], 80)
            .with_rbf(first_lineage, &["free-1"], 120)
            .with_rbf(second_lineage, &["free-1"], 100);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Fresh {
                requests: vec![send_request("free-1", 1, 12_000)],
            }]
        );
    }

    #[test]
    fn free_pending_yields_no_action_when_fresh_is_blocked_by_concurrency_cap() {
        let lineage_id = LineageId::new();
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 12_000)],
            Vec::new(),
            vec![live_lineage(lineage_id, 9, None)],
        );
        let mut config = WalletConfig::default();
        config.max_concurrent_lineages = 1;
        let evaluator = FakePlannerEvaluator::default()
            .with_fresh(&["free-1"], 50)
            .with_rbf(lineage_id, &["free-1"], 90);

        let actions = plan_wallet_batches(&runtime, &config, &evaluator);

        // RBF is disabled, and the concurrency cap blocks Fresh — so the planner
        // emits nothing for free requests this tick.
        assert!(actions.is_empty());
    }

    #[test]
    fn anchored_pending_requires_chained_handling() {
        let chain_anchor = anchor(7, 870_000);
        let runtime = runtime_state(
            Vec::new(),
            vec![anchored_request("anchored-1", 2, chain_anchor.clone())],
            Vec::new(),
        );
        let evaluator = FakePlannerEvaluator::default().with_chained(None, &["anchored-1"], 100);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: None,
                chain_anchor,
                requests: vec![anchored_request("anchored-1", 2, anchor(7, 870_000))],
            }]
        );
    }

    #[test]
    fn anchored_group_reuses_existing_matching_lineage_id() {
        let chain_anchor = anchor(20, 870_000);
        let lineage_id = LineageId::new();
        let runtime = runtime_state(
            Vec::new(),
            vec![anchored_request("anchored-1", 2, chain_anchor.clone())],
            vec![live_lineage(lineage_id, 3, Some(chain_anchor.clone()))],
        );
        let evaluator =
            FakePlannerEvaluator::default().with_chained(Some(lineage_id), &["anchored-1"], 55);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: Some(lineage_id),
                chain_anchor,
                requests: vec![anchored_request("anchored-1", 2, anchor(20, 870_000))],
            }]
        );
    }

    #[test]
    fn distinct_chain_anchor_groups_produce_separate_actions() {
        let first_anchor = anchor(3, 870_000);
        let second_anchor = anchor(4, 870_000);
        let runtime = runtime_state(
            Vec::new(),
            vec![
                anchored_request("a-1", 5, first_anchor.clone()),
                anchored_request("b-1", 6, second_anchor.clone()),
            ],
            Vec::new(),
        );
        let evaluator = FakePlannerEvaluator::default()
            .with_chained(None, &["a-1"], 40)
            .with_chained(None, &["b-1"], 50);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(actions.len(), 2);
        assert!(actions.contains(&PlannedBatchAction::Chained {
            existing_lineage_id: None,
            chain_anchor: first_anchor.clone(),
            requests: vec![anchored_request("a-1", 5, first_anchor)],
        }));
        assert!(actions.contains(&PlannedBatchAction::Chained {
            existing_lineage_id: None,
            chain_anchor: second_anchor.clone(),
            requests: vec![anchored_request("b-1", 6, second_anchor)],
        }));
    }

    #[test]
    fn planner_returns_empty_when_anchored_group_has_no_feasible_chained_plan() {
        let chain_anchor = anchor(30, 870_000);
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 10_000)],
            vec![anchored_request("anchored-1", 2, chain_anchor)],
            Vec::new(),
        );

        let actions = plan_wallet_batches(
            &runtime,
            &WalletConfig::default(),
            &FakePlannerEvaluator::default(),
        );

        assert!(actions.is_empty());
    }

    #[test]
    fn max_outputs_per_batch_blocks_infeasible_merges() {
        let runtime = runtime_state(
            vec![
                send_request("free-1", 1, 10_000),
                send_request("free-2", 2, 11_000),
            ],
            Vec::new(),
            Vec::new(),
        );
        let mut config = WalletConfig::default();
        config.max_outputs_per_batch = 1;
        let evaluator = FakePlannerEvaluator::default().with_fresh(&["free-1", "free-2"], 100);

        let actions = plan_wallet_batches(&runtime, &config, &evaluator);

        assert!(actions.is_empty());
    }

    #[test]
    fn output_cap_counts_sacp_outputs_but_not_regular_spend_inputs() {
        let runtime = runtime_state(
            vec![
                regular_spend_request("spend-1", 1),
                sacp_spend_request("sacp-1", 2),
            ],
            Vec::new(),
            Vec::new(),
        );
        let mut config = WalletConfig::default();
        config.max_outputs_per_batch = 1;
        let evaluator = FakePlannerEvaluator::default().with_fresh(&["spend-1", "sacp-1"], 90);

        let actions = plan_wallet_batches(&runtime, &config, &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Fresh {
                requests: vec![
                    regular_spend_request("spend-1", 1),
                    sacp_spend_request("sacp-1", 2),
                ],
            }]
        );
    }

    #[test]
    fn max_concurrent_lineages_blocks_fresh_but_not_mandatory_chained() {
        let free_runtime = runtime_state(
            vec![send_request("free-1", 1, 10_000)],
            Vec::new(),
            Vec::new(),
        );
        let chain_anchor = anchor(5, 870_000);
        let anchored_runtime = runtime_state(
            Vec::new(),
            vec![anchored_request("anchored-1", 3, chain_anchor.clone())],
            Vec::new(),
        );
        let mut config = WalletConfig::default();
        config.max_concurrent_lineages = 0;
        let evaluator = FakePlannerEvaluator::default()
            .with_fresh(&["free-1"], 20)
            .with_chained(None, &["anchored-1"], 30);

        assert!(plan_wallet_batches(&free_runtime, &config, &evaluator).is_empty());
        assert_eq!(
            plan_wallet_batches(&anchored_runtime, &config, &evaluator),
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: None,
                chain_anchor,
                requests: vec![anchored_request("anchored-1", 3, anchor(5, 870_000))],
            }]
        );
    }

    #[test]
    fn anchored_actions_are_preserved_when_free_requests_have_no_feasible_assignment() {
        let chain_anchor = anchor(40, 870_000);
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 10_000)],
            vec![anchored_request("anchored-1", 2, chain_anchor.clone())],
            Vec::new(),
        );
        let evaluator = FakePlannerEvaluator::default().with_chained(None, &["anchored-1"], 100);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: None,
                chain_anchor,
                requests: vec![anchored_request("anchored-1", 2, anchor(40, 870_000))],
            }]
        );
    }

    #[test]
    fn free_requests_can_be_mixed_into_a_chained_action_when_cheaper() {
        let chain_anchor = anchor(8, 870_000);
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 10_000)],
            vec![anchored_request("anchored-1", 2, chain_anchor.clone())],
            Vec::new(),
        );
        let evaluator = FakePlannerEvaluator::default()
            .with_chained(None, &["anchored-1"], 100)
            .with_chained(None, &["anchored-1", "free-1"], 130)
            .with_fresh(&["free-1"], 60);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: None,
                chain_anchor,
                requests: vec![
                    anchored_request("anchored-1", 2, anchor(8, 870_000)),
                    send_request("free-1", 1, 10_000),
                ],
            }]
        );
    }

    #[test]
    fn free_requests_can_be_mixed_into_existing_chained_lineage_when_cheaper() {
        let chain_anchor = anchor(9, 870_000);
        let lineage_id = LineageId::new();
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 10_000)],
            vec![anchored_request("anchored-1", 2, chain_anchor.clone())],
            vec![live_lineage(lineage_id, 3, Some(chain_anchor.clone()))],
        );
        let evaluator = FakePlannerEvaluator::default()
            .with_chained(Some(lineage_id), &["anchored-1"], 100)
            .with_chained(Some(lineage_id), &["anchored-1", "free-1"], 130)
            .with_fresh(&["free-1"], 60);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![PlannedBatchAction::Chained {
                existing_lineage_id: Some(lineage_id),
                chain_anchor,
                requests: vec![
                    anchored_request("anchored-1", 2, anchor(9, 870_000)),
                    send_request("free-1", 1, 10_000),
                ],
            }]
        );
    }

    #[test]
    fn free_requests_remain_separate_when_mixing_is_more_expensive() {
        let chain_anchor = anchor(50, 870_000);
        let runtime = runtime_state(
            vec![send_request("free-1", 1, 10_000)],
            vec![anchored_request("anchored-1", 2, chain_anchor.clone())],
            Vec::new(),
        );
        let evaluator = FakePlannerEvaluator::default()
            .with_chained(None, &["anchored-1"], 100)
            .with_chained(None, &["anchored-1", "free-1"], 180)
            .with_fresh(&["free-1"], 40);

        let actions = plan_wallet_batches(&runtime, &WalletConfig::default(), &evaluator);

        assert_eq!(
            actions,
            vec![
                PlannedBatchAction::Chained {
                    existing_lineage_id: None,
                    chain_anchor,
                    requests: vec![anchored_request("anchored-1", 2, anchor(50, 870_000))],
                },
                PlannedBatchAction::Fresh {
                    requests: vec![send_request("free-1", 1, 10_000)],
                },
            ]
        );
    }
}
