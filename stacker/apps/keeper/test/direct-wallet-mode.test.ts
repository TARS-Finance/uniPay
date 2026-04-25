import { describe, expect, it } from "vitest";
import { createKeeperRunner } from "../src/runner/keeper-runner.js";
import { StrategyLocks } from "../src/runner/locks.js";
import { createKeeperFixture } from "./support/in-memory.js";

describe("keeper direct-wallet mode", () => {
  it("executes an active strategy without grants when direct-wallet mode is enabled", async () => {
    const fixture = createKeeperFixture({
      grants: [],
      chainState: {
        inputBalance: "1000000",
        lpBalance: "0",
        delegatedLpBalance: "0",
        bondedLockedLpBalance: "828440",
        provideDelegateResult: {
          txHash: "provide-delegate-1",
          lpAmount: "828440",
        },
      },
    });

    const runner = createKeeperRunner({
      now: () => new Date("2026-04-25T00:00:00.000Z"),
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress:
        "0x81c3ea419d2fd3a27971021d9dd3cc708def05e5d6a09d39b2f1f9ba18312264",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "10",
      requireGrants: false,
    });

    const result = await runner.runTick();

    expect(result).toEqual([
      {
        strategyId: "strategy-1",
        outcome: "executed",
        reason: "success",
      },
    ]);
    expect(fixture.chain.provideDelegateCalls).toBe(1);
    expect(fixture.executionsRepository.list()[0]).toMatchObject({
      status: "success",
      inputAmount: "1000",
    });
  });
});
