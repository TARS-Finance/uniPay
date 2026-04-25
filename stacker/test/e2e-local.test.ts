import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import {
  ExecutionsRepository,
  GrantsRepository,
  openDatabase,
  PositionsRepository,
  StrategiesRepository,
  UsersRepository
} from "../packages/db/src/index.js";
import { migrate } from "drizzle-orm/node-postgres/migrator";
import { createDryRunKeeperChainClient } from "../packages/chain/src/index.js";
import { createApp } from "../apps/api/src/app.js";
import { createKeeperRunner } from "../apps/keeper/src/runner/keeper-runner.js";
import { StrategyLocks } from "../apps/keeper/src/runner/locks.js";

describe("local end-to-end flow", () => {
  let app: Awaited<ReturnType<typeof createApp>>;

  beforeAll(async () => {
    const { client, db } = openDatabase();

    await client.connect();
    try {
      await migrate(db, {
        migrationsFolder: "./packages/db/drizzle/migrations"
      });
    } finally {
      await client.end();
    }

    app = await createApp({
      config: {
        keeperAddress: "init1replacekeeperaddress",
        lockStakingModuleAddress: "0xlock",
        lockStakingModuleName: "lock_staking",
        lockupSeconds: "86400"
      },
      grantVerifier: {
        verify: async () => ({
          moveGrantActive: true,
          feegrantActive: true
        })
      }
    });
    await app.ready();
  });

  beforeEach(async () => {
    await app.db.execute(`
      truncate table executions, positions, grants, strategies, users
      restart identity cascade;
    `);
  });

  afterAll(async () => {
    await app?.close();
  });

  it("creates a strategy, confirms grants, runs a dry-run tick, and persists a simulated execution", async () => {
    const registerResponse = await app.inject({
      method: "POST",
      url: "/users/register",
      payload: {
        initiaAddress: "init1e2euser"
      }
    });
    const registerBody = registerResponse.json<{ userId: string }>();

    const strategyResponse = await app.inject({
      method: "POST",
      url: "/strategies",
      payload: {
        userId: registerBody.userId,
        inputDenom: "usdc",
        targetPoolId: "pool-1",
        validatorAddress: "initvaloper1validator",
        minBalanceAmount: "100",
        maxAmountPerRun: "250",
        maxSlippageBps: 100,
        cooldownSeconds: 300
      }
    });
    const strategyBody = strategyResponse.json<{ strategyId: string }>();

    await app.inject({
      method: "POST",
      url: "/grants/prepare",
      payload: {
        userId: registerBody.userId,
        strategyId: strategyBody.strategyId
      }
    });

    await app.inject({
      method: "POST",
      url: "/grants/confirm",
      payload: {
        userId: registerBody.userId,
        strategyId: strategyBody.strategyId
      }
    });

    const { client, db } = openDatabase();
    await client.connect();

    try {
      const runner = createKeeperRunner({
        now: () => new Date("2026-04-23T12:00:00.000Z"),
        usersRepository: new UsersRepository(db),
        strategiesRepository: new StrategiesRepository(db),
        grantsRepository: new GrantsRepository(db),
        executionsRepository: new ExecutionsRepository(db),
        positionsRepository: new PositionsRepository(db),
        chain: createDryRunKeeperChainClient({
          keeperAddress: "init1replacekeeperaddress",
          lpDenom: "ulp",
          startingBalances: {
            "init1e2euser:usdc": "500",
            "init1e2euser:ulp": "0",
            "init1e2euser:initvaloper1validator:ulp": "0",
            "init1e2euser:initvaloper1validator:pool-1:bonded-locked": "0"
          }
        }),
        locks: new StrategyLocks(),
        lpDenom: "ulp",
        lockStakingModuleAddress: "0xlock",
        lockStakingModuleName: "lock_staking",
        lockupSeconds: "86400"
      });

      await runner.runTick();
    } finally {
      await client.end();
    }

    const executionsResponse = await app.inject({
      method: "GET",
      url: `/strategies/${strategyBody.strategyId}/executions`
    });

    expect(executionsResponse.statusCode).toBe(200);
    expect(executionsResponse.json()).toEqual({
      executions: [
        expect.objectContaining({
          status: "simulated",
          provideTxHash: "dry-run-provide-delegate-1",
          delegateTxHash: "dry-run-provide-delegate-1"
        })
      ]
    });

    const positionsResponse = await app.inject({
      method: "GET",
      url: `/positions/${registerBody.userId}`
    });

    expect(positionsResponse.statusCode).toBe(200);
    expect(positionsResponse.json()).toEqual({
      positions: [
        expect.objectContaining({
          strategyId: strategyBody.strategyId,
          lastInputBalance: "250",
          lastLpBalance: "0",
          lastDelegatedLpBalance: "250",
          delegatedLpKind: "bonded-locked"
        })
      ]
    });
  });
});
