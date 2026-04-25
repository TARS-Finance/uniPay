# Rollup USDC Bridge for Auto-Earn — Design

**Date:** 2026-04-25
**Status:** Spec — pending implementation plan
**Hackathon:** INITIATE Season 1 (submission deadline 2026-04-26)

## Summary

Launch a fresh Initia EVM rollup (`minievm`) that accepts L1 testnet `uusdc` as a canonical bridged token (first L1→rollup deposit of `uusdc` auto-spawns the ERC20 wrapper on the rollup). After HTLC swaps complete on the rollup and merchant USDC accumulates, the merchant clicks "Bridge to L1 & Earn" in the existing `initia-fe` dashboard, which uses InterwovenKit's Interwoven Bridge widget to issue a canonical OPInit withdrawal. Once `uusdc` lands at the merchant's `init1...` address on L1, the existing `stacker` backend (configured with the same merchant key) auto-stakes via single-asset LP provide into the USDC/INIT pool followed by `lock_staking` delegation. The dashboard then polls stacker's existing `/merchants/:id/balance` endpoint to render staked principal, accrued yield, and APY.

This satisfies the hackathon's "Initia native feature" requirement (Interwoven Bridge) while reusing existing components (`HTLC.sol`, `initia-fe`, `stacker`) without modifying their internals.

## Goals

1. Fresh `minievm` rollup configured for short OPInit finalization (~5 min testnet) so demo recording is feasible.
2. Bridged USDC ERC20 on the rollup serves as the underlying for redeployed `HTLC.sol`.
3. Merchant cashout flow: rollup ERC20 USDC → user-clicked Interwoven Bridge → L1 `uusdc` → auto-staked by `stacker` → visible in earn dashboard.
4. Zero code changes to `stacker` (config-only integration).
5. Single-merchant v1.

## Non-Goals

- Multi-merchant routing in stacker.
- Withdraw-from-stake flow (merchant pulling LP back to rollup).
- Reverse-direction (L1 → rollup) deposits as a primary user flow — assumed handled by HTLC settlement upstream.
- Liquidity-fronted (instant) withdrawal route. Canonical OPInit only.
- Production hardening; testnet hackathon demo only.

## Architecture

Five units; three exist already.

### Existing units (no internal changes)

- **`Tars/initia/HTLC.sol`** — redeployed on the new rollup with the spawned bridged USDC ERC20 as the `_token` constructor arg. No Solidity changes; only deploy script updates.
- **`Tars/initia-fe/`** — adds a new "Earn" view; reuses InterwovenKit (`@initia/interwovenkit-react@^2.8.0`) provider already wired in.
- **`Tars/stacker/`** — black-box dependency. Already implements single-asset LP provide + `lock_staking` delegate + `GET /merchants/:id/balance` HTTP endpoint per its existing plan (`Tars/stacker/docs/superpowers/plans/2026-04-23-stacker-backend-keeper.md`). Configured via env to point at the merchant's L1 address + private key.

### New unit

**`Tars/initia-rollup/`** — new top-level folder under `Tars/`. Contains:

- `weave/launch.config.json` — rollup launch config (EVM VM, testnet L1, `output_finalization_period: 5m`, `output_submission_interval: 1m`). Schema from the `initia-appchain-dev` skill's `references/weave-config-schema.md`. There is no genesis-level "bridged denoms" field; `uusdc` becomes bridgeable automatically once the OPInit bridge is created during launch.
- `scripts/launch-rollup.sh` — runs `weave rollup launch` with the config; captures chain id + endpoints.
- `scripts/post-launch-discover.ts` — queries `minitiad query evm denom-erc20 uusdc`, writes the resulting ERC20 address into `initia-fe/.env` (`VITE_USDC_ERC20`) and into the HTLC redeploy script.
- `scripts/redeploy-htlc.ts` — deploys `HTLC.sol` from `Tars/initia/contracts/initia/` against the new rollup with the spawned ERC20.
- `scripts/demo-flow.ts` — end-to-end smoke for the demo video.
- `.env.example`, `README.md`, `docs/superpowers/specs/`, `docs/superpowers/plans/`.

### Frontend additions to `initia-fe/`

- `src/lib/chain.ts` — `rollupChain` custom chain object (with `bech32_prefix: 'init'`, `network_type: 'testnet'`, full `apis`/`fees`/`staking`/`native_assets` per skill rules), `INITIA_TESTNET_CHAIN_ID = 'initiation-2'`.
- `src/lib/usdc.ts` — `useRollupUsdcBalance(address)` — wraps `wagmi.useReadContract` against the bridged ERC20.
- `src/lib/stacker.ts` — `useMerchantPosition()` — polls `${VITE_STACKER_API_URL}/merchants/:id/balance`, returns typed shape.
- `src/components/EarnPanel.tsx` — UI: rollup USDC balance row, "Bridge to L1 & Earn" button, in-flight bridge state, L1 staked rows.
- `src/main.tsx` — `<InterwovenKitProvider {...TESTNET} customChain={rollupChain} customChains={[rollupChain]}>` (both required per skill rule).
- `src/App.tsx` — wire new "Earn" tab.
- `.env` — `VITE_ROLLUP_CHAIN_ID`, `VITE_ROLLUP_RPC_URL`, `VITE_ROLLUP_JSON_RPC_URL`, `VITE_USDC_ERC20`, `VITE_STACKER_API_URL`.

### Address & token model

- **Single ECDSA key** → derives `init1...` (L1 / Cosmos) and `0x...` (rollup / EVM). InterwovenKit handles both.
- **Merchant L1 address = stacker's `KEEPER_ADDRESS`**; same key in `KEEPER_PRIVATE_KEY`.
- **Decimals:** `uusdc` is 6 decimals on L1; bridged ERC20 inherits 6 decimals on rollup. FE uses `formatUnits(x, 6)` everywhere — never `formatEther`.
- **Native gas on rollup** (denom set at launch): merchant must hold a small balance of native to cover the bridge tx.

## Data Flow

```
┌─────────────────────────────────────────────────────────────────┐
│ rollup (minievm)                            L1 (initiation-2)   │
│                                                                 │
│  HTLC.sol ──redeem──▶ merchant 0x...        merchant init1...   │
│                          │                        ▲             │
│                          │ holds bridged          │             │
│                          │ USDC ERC20             │ uusdc       │
│                          ▼                        │ minted      │
│                  ┌─────────────────┐              │             │
│                  │ initia-fe Earn  │              │             │
│                  │   dashboard     │              │             │
│                  └───────┬─────────┘              │             │
│                          │ click "Bridge & Earn"  │             │
│                          ▼                        │             │
│                  openBridge widget                │             │
│                  (InterwovenKit)                  │             │
│                          │                        │             │
│                          ▼                        │             │
│           MsgInitiateTokenWithdrawal ────┐        │             │
│                                          │        │             │
└──────────────────────────────────────────┼────────┼─────────────┘
                                           │        │
                            OPInit challenge window │
                              (~10–30 min testnet)  │
                                           │        │
                                           └────────┘
                                                │
                                                ▼
                                  stacker keeper polls L1 balance
                                                │
                                                │ sees uusdc > 0
                                                ▼
                                  provide-single-asset-liquidity
                                  (USDC into USDC/INIT pool)
                                                │
                                                ▼
                                  lock_staking delegate (LP)
                                                │
                                                ▼
                                  position written to stacker DB
                                                │
                                  ┌─────────────┘
                                  ▼
              FE polls GET /merchants/:id/balance
              renders staked principal + yield + APY
```

**Numbered steps:**

1. HTLC swap completes → merchant has bridged USDC ERC20 on rollup (out-of-scope upstream).
2. Merchant opens `initia-fe` Earn tab; InterwovenKit auto-connects.
3. FE reads ERC20 balance via rollup JSON-RPC.
4. Merchant clicks "Bridge to L1 & Earn" → `openBridge({ srcChainId: ROLLUP_CHAIN_ID, srcDenom: 'uusdc' })`.
5. User signs `MsgInitiateTokenWithdrawal` in InterwovenKit wallet.
6. FE shows pending state with rollup tx hash + countdown; polls L1 balance.
7. After OPInit challenge window (~10–30 min testnet), `uusdc` minted to merchant `init1...`.
8. Stacker keeper tick sees balance > 0 → `provide-single-asset-liquidity` → `lock_staking delegate`.
9. Position written to stacker DB.
10. FE polls `GET /merchants/:id/balance` → renders staked principal, yield, APY.

**Pending-state UX:** Between steps 5 and 7, the dashboard shows "Bridging — finalizing on L1 (~X min)" with the rollup tx hash. Once L1 mint is detected, row flips to "staking…" then "earning."

## Module Boundaries

- `Tars/initia-rollup/` does not import any other Tars project's code; it only writes `.env` files into `initia-fe/` and reads `HTLC.sol` from `initia/contracts/initia/`.
- `initia-fe/` does not import stacker code; only calls its HTTP API.
- `stacker/` is unchanged; configured via env.
- HTLC contract changes are zero — only deploy parameters change.

## Error Handling

| Failure | Detection | UX response |
|---|---|---|
| Withdrawal in flight (signed on rollup, not yet finalized on L1) | Polling: rollup withdrawal tx in receipts but L1 `uusdc` balance unchanged | "Bridging — finalizing on L1 (~10–30 min)" with rollup tx hash + countdown |
| Withdrawal finalized but stacker hasn't staked yet | L1 `uusdc` > 0 but stacker `/balance` shows no new principal | "Funds on L1 — auto-stake queued" (resolves on next stacker tick) |
| Stacker API unreachable | Fetch error / timeout | "Earn dashboard offline — check stacker service" + retry button |
| Stacker in dry-run / paused | Stacker response shape includes a mode flag (verify against API) | "Auto-stake paused — funds idle on L1" |
| Insufficient rollup native gas for bridge tx | `viem` balance check on native denom before showing button | Disable button + tooltip "Need ~0.01 GAS for bridge tx" |
| Zero rollup USDC balance | `balanceOf` = 0 | Hide bridge button; show empty-state "No USDC to bridge yet" |
| User on wrong chain when clicking | Handled by InterwovenKit `openBridge` widget | Widget prompts chain switch — no custom code |

Out-of-scope failures (acknowledge but do not build): multi-merchant routing, withdraw-from-stake flow, OPInit challenge failures, reverse-direction deposits.

## Testing

**Unit (FE, vitest):**

- `useRollupUsdcBalance` against viem mocked client → returns expected balance for given address.
- `useMerchantPosition` against stubbed `fetch` → renders correct staked/yield fields.
- `EarnPanel` integration test: balance > 0 → bridge button enabled; balance = 0 → empty state.
- `BridgeButton` click → asserts `openBridge` called with `{ srcChainId: ROLLUP_CHAIN_ID, srcDenom: 'uusdc' }`.

**Manual integration (testnet, scripts in `Tars/initia-rollup/scripts/`):**

1. `launch-rollup.sh` — runs `weave rollup launch` with the config; captures chain id + endpoints.
2. Verify bridged denom: `minitiad query evm denom-erc20 uusdc` returns non-empty address.
3. **L1 → rollup deposit test:** deposit 1 USDC on L1, wait, assert ERC20 balance on rollup.
4. **Rollup → L1 withdrawal test:** submit withdrawal, poll L1 balance, assert finalization within window.
5. **Stacker auto-stake test:** with stacker pointed at the merchant key, send 1 USDC to the merchant `init1...`, wait one keeper tick, assert `/balance` returns staked > 0.

**E2E smoke (the demo recording):**

Single script `scripts/demo-flow.ts`: bridges merchant's existing rollup balance → waits for finalization → confirms stacker stakes → prints final dashboard payload. This is the demo video's core sequence.

**Out of test scope:** stacker internals, HTLC redemption (HTLC is "done"), the rollup's own consensus.

## Risks & Assumptions

- **Stacker API contract.** Spec assumes `GET /merchants/:id/balance` is implemented per stacker's plan. Verify against the running service before integration; if not yet built, either land it in stacker first or stub it temporarily for FE work.
- **OPInit testnet finalization window.** Demo video timing must accommodate ~10–30 min latency. If the video must be shorter, capture pre/post-bridge separately.
- **Bridged ERC20 spawn.** ERC20 wrapper for `uusdc` is auto-created on the rollup's first L1→rollup deposit of `uusdc`. The plan therefore includes a one-shot "seed deposit" step that mints a small amount of bridged USDC and captures the ERC20 address via `minitiad query evm denom-erc20 uusdc`.
- **Pool ID stability.** Stacker's `TARGET_POOL_ID` must point to the live USDC/INIT testnet pool; verify before configuring.
- **Native gas symbol on rollup.** Set at launch (e.g., `GAS` or `umin`); FE warning copy must match the chosen symbol.

## Open Questions

None blocking. Ready for implementation planning.
