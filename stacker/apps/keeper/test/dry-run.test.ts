import { describe, expect, it } from "vitest";
import { createDryRunKeeperChainClient } from "@stacker/chain";
import { createKeeperRunner } from "../src/runner/keeper-runner.js";
import { StrategyLocks } from "../src/runner/locks.js";
import { createKeeperFixture } from "./support/in-memory.js";

const now = new Date("2026-04-23T12:00:00.000Z");

describe("keeper dry-run mode", () => {
  it("runs the single-asset provide+delegate path by default in dry-run mode", async () => {
    const fixture = createKeeperFixture();
    const chain = createDryRunKeeperChainClient({
      keeperAddress: "init1replacekeeperaddress",
      lpDenom: "ulp",
      startingBalances: {
        "init1useraddress:usdc": "500",
        "init1useraddress:ulp": "0",
        "init1useraddress:initvaloper1validator:ulp": "0",
        "init1useraddress:initvaloper1validator:pool-1:bonded-locked": "0"
      }
    });

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain,
      locks: new StrategyLocks(),
      lpDenom: "ulp",
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400"
    });

    const results = await runner.runTick();

    expect(results).toEqual([
      {
        strategyId: "strategy-1",
        outcome: "executed",
        reason: "success"
      }
    ]);
    expect(chain.mode).toBe("dry-run");
    expect(chain.broadcastCalls).toBe(0);
    expect(chain.getPlannedMessages()).toHaveLength(1);
    expect(chain.getPlannedMessages().map((message) => message["@type"])).toEqual([
      "/cosmos.authz.v1beta1.MsgExec"
    ]);
    expect(fixture.executionsRepository.list()[0]).toMatchObject({
      status: "simulated",
      provideTxHash: "dry-run-provide-delegate-1",
      delegateTxHash: "dry-run-provide-delegate-1"
    });
  });
});
