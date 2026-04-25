# ADR 0001: Backend-First Keeper

## Status

Accepted

## Context

The service needs to automate per-user Initia actions:

- convert one-sided input into a target INIT LP
- lock-delegate the resulting LP position through the VIP lock-staking path

The product scope explicitly excludes:

- pooled user vaults
- cross-chain routing
- frontend-first delivery
- raw private-key custody

The system also needs a safe verification path before any live testnet writes occur.

## Decision

We use a backend-first architecture with:

- `apps/api` for user registration, strategy setup, grant preparation, and status reads
- `apps/keeper` for scheduled execution and reconciliation
- `packages/db` for persistence
- `packages/chain` for Initia authz, feegrant, and execution encoding
- `scripts/mock-fe.ts` as the initial onboarding surrogate instead of a full frontend

The keeper runs one strategy per user. It does not pool balances across users.

For safety, Phase 5 adds `KEEPER_MODE=dry-run|live`. Dry-run mode:

- builds real `MsgExec` payloads
- avoids broadcast
- records executions as `simulated`
- updates synthetic balances so reconciliation and positions can be verified locally

The keeper always uses `vip::lock_staking::single_asset_provide_delegate` so one-sided input can be converted, provided, and locked for Enshrined Liquidity rewards in one tx.

## Consequences

### Positive

- API and keeper can be reviewed independently.
- User onboarding can be exercised before a frontend is built.
- Dry-run mode provides a stable pre-live validation path.
- The keeper logic is testable without chain access.

### Negative

- Dry-run balances are synthetic and not a price-accurate market simulation.
- The database is required even for local verification flows.
- Live mode now depends on simulated event parsing to derive `min_liquidity`; if InitiaDEX or bank event shapes change, the keeper will refuse to broadcast until the parser is updated.
- Reward-mode reconciliation depends on the current VIP bonded-lock query/event shape; if Initia changes those return formats, the parser must be updated before live runs.

## Follow-Up

- Add structured logging and dry-run report storage if operator review needs become heavier.
- Expand the end-to-end tests once a live-mode smoke environment is available.
