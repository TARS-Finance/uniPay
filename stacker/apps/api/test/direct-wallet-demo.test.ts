import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { executions, positions } from "@stacker/db";
import { createApp } from "../src/app.js";

describe("direct-wallet demo api", () => {
  let app: Awaited<ReturnType<typeof createApp>>;

  beforeAll(async () => {
    app = await createApp({
      config: {
        executionMode: "direct",
        lockStakingModuleAddress:
          "0x81c3ea419d2fd3a27971021d9dd3cc708def05e5d6a09d39b2f1f9ba18312264",
        lockStakingModuleName: "lock_staking",
        lockupSeconds: "10",
        merchantDemoApyBps: 2480,
      },
      grantVerifier: {
        verify: async () => ({
          moveGrantActive: true,
          feegrantActive: true,
        }),
      },
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

  it("accepts uusdc strategies and exposes the merchant balance shape used by the frontend", async () => {
    const merchantAddress = "init18f735agmd8zav9lrtnregkqn7eu4wc8cnanpql";
    const registerResponse = await app.inject({
      method: "POST",
      url: "/users/register",
      payload: {
        initiaAddress: merchantAddress,
      },
    });

    expect(registerResponse.statusCode).toBe(201);
    const { userId } = registerResponse.json<{ userId: string }>();

    const strategyResponse = await app.inject({
      method: "POST",
      url: "/strategies",
      payload: {
        userId,
        inputDenom: "uusdc",
        targetPoolId:
          "0xdbf06c48af3984ec6d9ae8a9aa7dbb0bb1e784aa9b8c4a5681af660cf8558d7d",
        validatorAddress: "initvaloper1cduny8wdjupu2lhya9npc9j4x5ytn05kt36x0c",
        minBalanceAmount: "100",
        maxAmountPerRun: "10000000",
        maxSlippageBps: 100,
        cooldownSeconds: 10,
      },
    });

    expect(strategyResponse.statusCode).toBe(201);
    const { strategyId } = strategyResponse.json<{ strategyId: string }>();

    await app.db.insert(positions).values({
      strategyId,
      userId,
      lastInputBalance: "250000",
      lastLpBalance: "0",
      lastDelegatedLpBalance: "828440",
      lastRewardSnapshot: null,
      lastSyncedAt: new Date("2026-04-25T00:00:00.000Z"),
    });

    await app.db.insert(executions).values({
      strategyId,
      userId,
      status: "success",
      inputAmount: "1000000",
      lpAmount: "828440",
      provideTxHash: "0xprovide",
      delegateTxHash: "0xdelegate",
      errorCode: null,
      errorMessage: null,
      startedAt: new Date("2026-04-25T00:00:00.000Z"),
      finishedAt: new Date("2026-04-25T00:00:30.000Z"),
    });

    const balanceResponse = await app.inject({
      method: "GET",
      url: `/merchants/${merchantAddress}/balance`,
    });

    expect(balanceResponse.statusCode).toBe(200);
    expect(balanceResponse.json()).toEqual({
      principal_available: "250000",
      principal_staked: "1000000",
      yield_earned: "0",
      apy_bps: 2480,
    });
  });
});
