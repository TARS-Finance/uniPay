# Stacker Backend Keeper Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a backend-first, same-chain Initia keeper service that lets each user authorize a server-side keeper to individually provide USDC/iUSDC liquidity into a target INIT pool and then delegate the resulting LP position.

**Architecture:** Use a TypeScript monorepo with separate `api` and `keeper` apps, plus shared `chain`, `db`, and `shared` packages. Keep the frontend out of scope for now except for a mock script that simulates wallet-driven onboarding and strategy creation while the backend, grants, keeper loop, and reconciliation flows are built and verified.

**Tech Stack:** TypeScript, Node.js 22, pnpm workspaces, Fastify, Zod, PostgreSQL, Drizzle ORM, `@initia/initia.js`, Vitest, tsx, Pino

---

## File Structure

Planned workspace layout:

- Create: `package.json`
- Create: `pnpm-workspace.yaml`
- Create: `tsconfig.base.json`
- Create: `vitest.workspace.ts`
- Create: `docker-compose.yml`
- Create: `apps/api/package.json`
- Create: `apps/api/src/app.ts`
- Create: `apps/api/src/server.ts`
- Create: `apps/api/src/config.ts`
- Create: `apps/api/src/routes/users.ts`
- Create: `apps/api/src/routes/strategies.ts`
- Create: `apps/api/src/routes/grants.ts`
- Create: `apps/api/src/routes/positions.ts`
- Create: `apps/api/src/routes/executions.ts`
- Create: `apps/api/src/services/*.ts`
- Create: `apps/api/test/*.test.ts`
- Create: `apps/keeper/package.json`
- Create: `apps/keeper/src/main.ts`
- Create: `apps/keeper/src/config.ts`
- Create: `apps/keeper/src/runner/keeper-runner.ts`
- Create: `apps/keeper/src/runner/locks.ts`
- Create: `apps/keeper/src/runner/retry-policy.ts`
- Create: `apps/keeper/src/jobs/run-tick.ts`
- Create: `apps/keeper/test/*.test.ts`
- Create: `packages/shared/package.json`
- Create: `packages/shared/src/types/*.ts`
- Create: `packages/shared/src/errors/*.ts`
- Create: `packages/shared/src/logging/*.ts`
- Create: `packages/shared/src/config/*.ts`
- Create: `packages/shared/test/*.test.ts`
- Create: `packages/db/package.json`
- Create: `packages/db/drizzle/schema.ts`
- Create: `packages/db/drizzle/migrations/*`
- Create: `packages/db/src/client.ts`
- Create: `packages/db/src/repositories/*.ts`
- Create: `packages/db/test/*.test.ts`
- Create: `packages/chain/package.json`
- Create: `packages/chain/src/client/rest-client.ts`
- Create: `packages/chain/src/authz/build-move-grant.ts`
- Create: `packages/chain/src/authz/build-stake-grant.ts`
- Create: `packages/chain/src/authz/build-feegrant.ts`
- Create: `packages/chain/src/authz/encode-msg-exec.ts`
- Create: `packages/chain/src/dex/provide-single-asset-liquidity.ts`
- Create: `packages/chain/src/staking/delegate-lp.ts`
- Create: `packages/chain/src/query/*.ts`
- Create: `packages/chain/src/reconcile/*.ts`
- Create: `packages/chain/test/*.test.ts`
- Create: `scripts/mock-fe.ts`
- Create: `scripts/run-local-checks.sh`
- Create: `docs/adr/0001-backend-first-keeper.md`
- Create: `docs/runbooks/local-dev.md`
- Create: `docs/runbooks/keeper-operations.md`

Phase boundaries below are intentionally reviewable and testable. No phase should start until the previous phase has passed its review gate and test gate.

## Chunk 1: Foundation

### Phase 0: Repo Bootstrap And Local Tooling

**Purpose:** Stand up a minimal monorepo, local database, test runner, and package boundaries before any business logic exists.

**Review gate:** Repo shape, package boundaries, stack choice, and local developer workflow are approved.

**Test gate:** Fresh install, typecheck, lint, and an initial smoke test all pass locally.

### Task 1: Bootstrap The Workspace

**Files:**
- Create: `package.json`
- Create: `pnpm-workspace.yaml`
- Create: `tsconfig.base.json`
- Create: `vitest.workspace.ts`
- Create: `.editorconfig`
- Create: `docker-compose.yml`
- Create: `scripts/run-local-checks.sh`

- [ ] **Step 1: Create root workspace manifests**

Define:
- workspace packages: `apps/*`, `packages/*`
- root scripts: `build`, `typecheck`, `lint`, `test`, `check`
- Node version requirement

- [ ] **Step 2: Create a failing smoke test for workspace wiring**

Create a minimal test in `packages/shared/test/workspace-smoke.test.ts` that imports one exported constant from `packages/shared/src/index.ts`.

Run: `pnpm test -- --runInBand`
Expected: FAIL because the package entrypoint does not exist yet

- [ ] **Step 3: Add the minimal shared package entrypoint**

Create `packages/shared/src/index.ts` exporting a constant like `STACKER_APP_NAME = "stacker"`.

- [ ] **Step 4: Add local tooling scripts**

`scripts/run-local-checks.sh` should run:

```bash
pnpm lint
pnpm typecheck
pnpm test
```

- [ ] **Step 5: Verify the foundation**

Run:

```bash
pnpm install
pnpm test -- --runInBand
pnpm typecheck
```

Expected:
- workspace installs cleanly
- smoke test passes
- no TypeScript config errors

### Task 2: Add Local Database And Config Conventions

**Files:**
- Create: `packages/shared/src/config/env.ts`
- Create: `packages/shared/src/config/public-types.ts`
- Create: `.env.example`
- Create: `docs/runbooks/local-dev.md`

- [ ] **Step 1: Define typed environment shape**

Create Zod schemas for:
- `DATABASE_URL`
- `KEEPER_PRIVATE_KEY` placeholder
- `INITIA_LCD_URL`
- `INITIA_RPC_URL`
- `KEEPER_ADDRESS`
- `TARGET_POOL_ID`
- `DEX_MODULE_ADDRESS`
- `DEX_MODULE_NAME`

- [ ] **Step 2: Write failing config tests**

Create `packages/shared/test/env.test.ts` covering:
- valid env parses
- missing required env fails
- malformed URL fails

Run: `pnpm test packages/shared/test/env.test.ts -- --runInBand`
Expected: FAIL because parser does not exist yet

- [ ] **Step 3: Implement config parsing**

Create `env.ts` that exports parsed configuration and throws descriptive errors.

- [ ] **Step 4: Add local dev runbook**

Document:
- how to start Postgres with `docker-compose up -d`
- how to copy `.env.example` to `.env`
- how to run API and keeper apps

- [ ] **Step 5: Verify config behavior**

Run:

```bash
pnpm test packages/shared/test/env.test.ts -- --runInBand
```

Expected: PASS

## Chunk 2: Persistence And Domain

### Phase 1: Database Schema, Domain Types, And Repositories

**Purpose:** Create persistent models and repository boundaries before any chain logic or HTTP routes exist.

**Review gate:** Schema covers user lifecycle, grants, strategies, executions, and positions without hidden coupling.

**Test gate:** Migrations apply cleanly on a local database and repository tests pass.

### Task 3: Implement The Database Schema

**Files:**
- Create: `packages/db/drizzle/schema.ts`
- Create: `packages/db/src/client.ts`
- Create: `packages/db/src/repositories/users-repository.ts`
- Create: `packages/db/src/repositories/strategies-repository.ts`
- Create: `packages/db/src/repositories/grants-repository.ts`
- Create: `packages/db/src/repositories/executions-repository.ts`
- Create: `packages/db/src/repositories/positions-repository.ts`
- Create: `packages/db/test/schema.test.ts`

- [ ] **Step 1: Write failing schema tests**

Test for presence of tables and critical unique constraints:
- `users.initia_address` unique
- one strategy belongs to one user
- grants tied to one user

Run: `pnpm test packages/db/test/schema.test.ts -- --runInBand`
Expected: FAIL because schema does not exist yet

- [ ] **Step 2: Create the schema**

Tables:
- `users`
- `strategies`
- `grants`
- `executions`
- `positions`

Required enum states:
- strategy status: `draft`, `grant_pending`, `active`, `executing`, `partial_lp`, `paused`, `expired`, `error`
- execution status: `queued`, `providing`, `delegating`, `success`, `failed`, `retryable`

- [ ] **Step 3: Generate and apply the first migration**

Run:

```bash
pnpm db:generate
pnpm db:migrate
```

Expected: migration applies to local Postgres cleanly

- [ ] **Step 4: Add repository primitives**

Each repository should handle one aggregate only. No chain logic here.

- [ ] **Step 5: Verify persistence**

Run:

```bash
pnpm test packages/db/test/schema.test.ts -- --runInBand
```

Expected: PASS

### Task 4: Add Shared Domain Types

**Files:**
- Create: `packages/shared/src/types/strategy.ts`
- Create: `packages/shared/src/types/grants.ts`
- Create: `packages/shared/src/types/execution.ts`
- Create: `packages/shared/src/types/position.ts`
- Create: `packages/shared/test/domain-types.test.ts`

- [ ] **Step 1: Write failing tests for domain normalization**

Cover:
- strategy status transitions are represented consistently
- input denom union is only `usdc | iusdc`
- execution status payload contains optional hashes only when relevant

- [ ] **Step 2: Implement domain types and guards**

Use Zod or literal unions for:
- `InputDenom`
- `StrategyStatus`
- `ExecutionStatus`

- [ ] **Step 3: Verify domain types**

Run:

```bash
pnpm test packages/shared/test/domain-types.test.ts -- --runInBand
```

Expected: PASS

## Chunk 3: Chain Adapter And Grant Builder

### Phase 2: Authz, Feegrant, And Chain Execution Building Blocks

**Purpose:** Build grant payload generation and execution message encoding before any HTTP or scheduler flow depends on them.

**Review gate:** Permission scope is narrow and maps exactly to the approved backend design.

**Test gate:** Snapshot/unit tests prove the generated messages and grants match the expected Initia types and scopes.

### Task 5: Build Grant Payload Generators

**Files:**
- Create: `packages/chain/src/authz/build-move-grant.ts`
- Create: `packages/chain/src/authz/build-stake-grant.ts`
- Create: `packages/chain/src/authz/build-feegrant.ts`
- Create: `packages/chain/test/build-grants.test.ts`

- [ ] **Step 1: Write failing grant snapshot tests**

Cover:
- Move authz includes one module address, one module name, and explicit function names
- Stake authz includes delegate-only type, validator allowlist, and max token cap
- Feegrant includes allowed message restrictions and a bounded allowance

Run: `pnpm test packages/chain/test/build-grants.test.ts -- --runInBand`
Expected: FAIL because builders do not exist yet

- [ ] **Step 2: Implement Move authz builder**

Output should use Initia JS `ExecuteAuthorization` items for:
- `module_address`
- `module_name`
- `function_names`

- [ ] **Step 3: Implement Stake authz builder**

Output should use Initia JS `StakeAuthorization` with:
- `authorization_type = DELEGATE`
- one validator allowlist
- max token cap

- [ ] **Step 4: Implement feegrant builder**

Start with `AllowedMsgAllowance` restricted to:
- `/cosmos.authz.v1beta1.MsgExec`

The underlying allowance should be small and time bounded.

- [ ] **Step 5: Verify grant builders**

Run:

```bash
pnpm test packages/chain/test/build-grants.test.ts -- --runInBand
```

Expected: PASS with stable snapshots

### Task 6: Build Execution Message Encoders

**Files:**
- Create: `packages/chain/src/authz/encode-msg-exec.ts`
- Create: `packages/chain/src/dex/provide-single-asset-liquidity.ts`
- Create: `packages/chain/src/staking/delegate-lp.ts`
- Create: `packages/chain/test/encode-executions.test.ts`

- [ ] **Step 1: Write failing execution tests**

Cover:
- DEX execution wraps a Move message inside `MsgExec`
- delegation wraps an `initia.mstaking.v1.MsgDelegate` inside `MsgExec`
- signer/grantee wiring matches the keeper model

- [ ] **Step 2: Implement DEX execution encoder**

This phase should only support:
- `single_asset_provide_liquidity_script`

Do not add explicit `swap_script` yet.

- [ ] **Step 3: Implement LP delegation encoder**

Use `MsgDelegate` from `initia.mstaking.v1`.

- [ ] **Step 4: Verify execution encoding**

Run:

```bash
pnpm test packages/chain/test/encode-executions.test.ts -- --runInBand
```

Expected: PASS

## Chunk 4: API And Mock Frontend Script

### Phase 3: HTTP Surface And Script-Driven Onboarding

**Purpose:** Expose backend capabilities through a small API and validate them end-to-end with a mock script instead of building UI.

**Review gate:** Request/response contracts are stable enough for later frontend integration.

**Test gate:** API integration tests and the mock script pass against a local database and mocked chain adapter.

### Task 7: Implement API Endpoints

**Files:**
- Create: `apps/api/src/app.ts`
- Create: `apps/api/src/server.ts`
- Create: `apps/api/src/routes/users.ts`
- Create: `apps/api/src/routes/strategies.ts`
- Create: `apps/api/src/routes/grants.ts`
- Create: `apps/api/src/routes/positions.ts`
- Create: `apps/api/src/routes/executions.ts`
- Create: `apps/api/src/services/*.ts`
- Create: `apps/api/test/api.test.ts`

- [ ] **Step 1: Write failing API tests**

Cover:
- register user
- create strategy
- prepare grants
- confirm grants
- read strategy status
- read execution history

- [ ] **Step 2: Implement `POST /users/register`**

Input:

```json
{ "initiaAddress": "init1..." }
```

- [ ] **Step 3: Implement `POST /strategies`**

Persist:
- input denom
- pool id
- validator
- thresholds

- [ ] **Step 4: Implement `POST /grants/prepare` and `POST /grants/confirm`**

`prepare` must return the exact keeper/grant payload summary the script or frontend needs to present.

- [ ] **Step 5: Implement read endpoints**

Read-only endpoints:
- strategy status
- positions
- executions

- [ ] **Step 6: Verify API**

Run:

```bash
pnpm test apps/api/test/api.test.ts -- --runInBand
```

Expected: PASS

### Task 8: Implement The Mock FE Script

**Files:**
- Create: `scripts/mock-fe.ts`
- Create: `scripts/mock-fe.example.json`
- Create: `apps/api/test/mock-fe-flow.test.ts`

- [ ] **Step 1: Write a failing script flow test**

Flow:
- register a user
- create a strategy
- request grant payloads
- simulate confirmation
- fetch strategy status

- [ ] **Step 2: Implement `scripts/mock-fe.ts`**

Accept:
- API base URL
- wallet address
- strategy config JSON

Print:
- created user id
- strategy id
- keeper address
- grant payload summary
- resulting strategy status

- [ ] **Step 3: Verify the mock flow**

Run:

```bash
pnpm tsx scripts/mock-fe.ts --config ./scripts/mock-fe.example.json
```

Expected: script completes without manual UI and prints the created strategy lifecycle

## Chunk 5: Keeper And Reconciliation

### Phase 4: Scheduled Execution, Locking, Retries, And State Sync

**Purpose:** Turn stored strategies and grants into deterministic per-user keeper actions.

**Review gate:** The tick algorithm, locking, and retry semantics are approved, especially for partial success cases.

**Test gate:** Keeper unit tests and integration tests prove no double execution, correct cooldown behavior, and correct partial retry behavior.

### Task 9: Implement The Keeper Tick Loop

**Files:**
- Create: `apps/keeper/src/main.ts`
- Create: `apps/keeper/src/runner/keeper-runner.ts`
- Create: `apps/keeper/src/runner/locks.ts`
- Create: `apps/keeper/src/runner/retry-policy.ts`
- Create: `apps/keeper/src/jobs/run-tick.ts`
- Create: `apps/keeper/test/keeper-runner.test.ts`

- [ ] **Step 1: Write failing keeper tests**

Cover:
- skip when below threshold
- skip when paused
- skip when grant expired
- lock prevents concurrent execution
- cooldown prevents re-run

- [ ] **Step 2: Implement per-strategy locking**

One strategy must never have two in-flight executions.

- [ ] **Step 3: Implement the selection algorithm**

Eligibility:
- active
- not paused
- grants valid
- cooldown elapsed
- no active lock

- [ ] **Step 4: Implement the main runner**

Execution flow:
- query input balance
- if eligible, create execution record
- call DEX step
- call delegate step
- update status

- [ ] **Step 5: Verify keeper behavior**

Run:

```bash
pnpm test apps/keeper/test/keeper-runner.test.ts -- --runInBand
```

Expected: PASS

### Task 10: Implement Partial Failure Recovery And Reconciliation

**Files:**
- Create: `packages/chain/src/reconcile/reconcile-provide.ts`
- Create: `packages/chain/src/reconcile/reconcile-delegate.ts`
- Create: `packages/chain/src/query/get-input-balance.ts`
- Create: `packages/chain/src/query/get-lp-balance.ts`
- Create: `packages/chain/src/query/get-delegated-lp-balance.ts`
- Create: `apps/keeper/test/reconciliation.test.ts`

- [ ] **Step 1: Write failing reconciliation tests**

Cover:
- provide succeeds, delegate fails -> next tick retries delegate only
- tx hash exists but confirmation is delayed -> do not double-send
- position sync updates balances after success

- [ ] **Step 2: Implement DEX result reconciliation**

Persist `provide_tx_hash` and parse LP delta before any delegation retry logic runs.

- [ ] **Step 3: Implement delegation reconciliation**

Persist `delegate_tx_hash` and sync delegated LP balance.

- [ ] **Step 4: Verify partial failure handling**

Run:

```bash
pnpm test apps/keeper/test/reconciliation.test.ts -- --runInBand
```

Expected: PASS

## Chunk 6: Hardening And Testnet Proving

### Phase 5: Dry-Run Mode, Operations, And Reviewable Testnet Proof

**Purpose:** Add enough observability and dry-run safety to verify the service on testnet without risking repeated or opaque writes.

**Review gate:** Operators can inspect every planned action before live execution.

**Test gate:** Local full-stack checks pass and one documented dry-run scenario succeeds against testnet or a stable mock environment.

### Task 11: Add Dry-Run Execution Mode

**Files:**
- Create: `packages/chain/src/client/dry-run-client.ts`
- Modify: `apps/keeper/src/config.ts`
- Modify: `apps/keeper/src/runner/keeper-runner.ts`
- Create: `apps/keeper/test/dry-run.test.ts`

- [ ] **Step 1: Write failing dry-run tests**

Cover:
- keeper selects eligible users
- messages are built
- no tx broadcast occurs
- execution is recorded as simulated

- [ ] **Step 2: Implement dry-run mode**

Guard real writes behind a config flag:
- `KEEPER_MODE=dry-run|live`

- [ ] **Step 3: Verify dry-run**

Run:

```bash
KEEPER_MODE=dry-run pnpm test apps/keeper/test/dry-run.test.ts -- --runInBand
```

Expected: PASS

### Task 12: Add Operational Docs And Final Validation Flow

**Files:**
- Create: `docs/runbooks/keeper-operations.md`
- Create: `docs/adr/0001-backend-first-keeper.md`
- Create: `apps/api/test/e2e-local.test.ts`

- [ ] **Step 1: Document operational runbooks**

Include:
- local startup
- dry-run execution
- grant debugging
- partial failure handling
- safe rollout checklist

- [ ] **Step 2: Write a local end-to-end test**

Flow:
- create user
- create strategy
- confirm grants
- run dry-run tick
- verify execution record

- [ ] **Step 3: Run final validation**

Run:

```bash
docker-compose up -d
pnpm install
pnpm check
pnpm test apps/api/test/e2e-local.test.ts -- --runInBand
```

Expected:
- local stack boots
- all checks pass
- one simulated strategy run is visible in persisted state

## Phase Exit Criteria

Implementation may proceed phase-by-phase only if each phase exits with:

- review sign-off on architecture and scope for that phase
- all tests for that phase passing
- no unresolved TODOs that would undermine the next phase
- docs updated if operator behavior or setup changed

## Suggested Phase Review Sequence

1. Review and approve Phase 0 before any package-level implementation.
2. Review and approve Phase 1 after schema and domain tests pass.
3. Review and approve Phase 2 after grant and execution builders are snapshot-tested.
4. Review and approve Phase 3 after the mock FE script drives the API locally.
5. Review and approve Phase 4 after partial-failure recovery is proven.
6. Review and approve Phase 5 only after dry-run validation is complete.

## Notes For Implementation

- Keep the frontend out of scope until the mock script-driven flow is stable.
- Do not add cross-chain routing, vaults, or pooled user balances in this plan.
- Do not add explicit DEX swap flows until `single_asset_provide_liquidity_script` has been proven insufficient.
- Treat grant scope as a security boundary, not an implementation detail.
- Prefer dry-run and mocked-chain tests before any live testnet write path.

Plan complete and saved to `docs/superpowers/plans/2026-04-23-stacker-backend-keeper.md`. Ready to execute?
