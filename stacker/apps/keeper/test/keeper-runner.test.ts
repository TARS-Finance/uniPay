import { describe, expect, it, vi } from "vitest";
import { createKeeperRunner } from "../src/runner/keeper-runner.js";
import { StrategyLocks } from "../src/runner/locks.js";
import { createKeeperFixture, Deferred } from "./support/in-memory.js";

const now = new Date("2026-04-23T12:00:00.000Z");
const baseStrategy = createKeeperFixture().strategies[0]!;
const baseGrant = createKeeperFixture().grants[0]!;

function createLoggerStub() {
  const logger = {
    child: vi.fn(),
    debug: vi.fn(),
    info: vi.fn(),
    warn: vi.fn(),
    error: vi.fn()
  };

  logger.child.mockImplementation(() => logger);

  return logger;
}

describe("keeper runner", () => {
  it("skips a strategy when the input balance is below threshold", async () => {
    const fixture = createKeeperFixture({
      chainState: {
        inputBalance: "50",
        lpBalance: "0",
        delegatedLpBalance: "0"
      }
    });
    const logger = createLoggerStub();

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400",
      logger
    });

    const result = await runner.runTick();

    expect(result[0]).toMatchObject({
      strategyId: "strategy-1",
      outcome: "skipped",
      reason: "below-threshold"
    });
    expect(logger.info).toHaveBeenCalledWith(
      expect.objectContaining({
        inputBalanceRaw: "50",
        inputBalanceDisplay: "0.00005 USDC (raw 50 usdc)",
        minBalanceAmountRaw: "100",
        minBalanceAmountDisplay: "0.0001 USDC (raw 100 usdc)"
      }),
      "keeper strategy skipped below threshold"
    );
    expect(fixture.chain.provideCalls).toBe(0);
    expect(fixture.executionsRepository.list()).toHaveLength(0);
  });

  it("logs upstream chain error details when provide+delegate fails", async () => {
    const fixture = createKeeperFixture({
      chainState: {
        inputBalance: "500",
        lpBalance: "0",
        delegatedLpBalance: "0",
        provideError: Object.assign(
          new Error("Request failed with status code 500"),
          {
            code: "ERR_BAD_RESPONSE",
            response: {
              status: 500,
              data: {
                code: 2,
                message:
                  "VM aborted: location=0xlock::lock_staking, code=65544"
              }
            }
          }
        )
      }
    });
    const logger = createLoggerStub();

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400",
      logger
    });

    const result = await runner.runTick();

    expect(result[0]).toMatchObject({
      strategyId: "strategy-1",
      outcome: "skipped",
      reason: "provide-failed"
    });
    expect(logger.error).toHaveBeenCalledWith(
      expect.objectContaining({
        error: expect.stringContaining("VM aborted"),
        inputAmountRaw: "500",
        inputAmountDisplay: "0.0005 USDC (raw 500 usdc)"
      }),
      "keeper strategy provide+delegate failed"
    );
  });

  it("skips a paused strategy", async () => {
    const fixture = createKeeperFixture({
      strategies: [
        {
          ...baseStrategy,
          status: "paused"
        }
      ]
    });

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400"
    });

    const result = await runner.runTick();

    expect(result[0]).toMatchObject({
      strategyId: "strategy-1",
      outcome: "skipped",
      reason: "not-runnable"
    });
    expect(fixture.chain.provideCalls).toBe(0);
  });

  it("marks a strategy expired when grants are no longer valid", async () => {
    const fixture = createKeeperFixture({
      grants: [
        {
          ...baseGrant,
          moveGrantExpiresAt: new Date("2026-04-01T00:00:00.000Z")
        }
      ]
    });

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400"
    });

    const result = await runner.runTick();

    expect(result[0]).toMatchObject({
      strategyId: "strategy-1",
      outcome: "skipped",
      reason: "grant-expired"
    });
    expect(fixture.strategiesRepository.getById("strategy-1")?.status).toBe("expired");
    expect(fixture.chain.provideCalls).toBe(0);
  });

  it("prevents concurrent execution with the strategy lock", async () => {
    const deferred = new Deferred<{
      txHash: string;
      lpAmount: string;
      rewardSnapshot?: {
        kind: "bonded-locked";
        stakingAccount: string;
        metadata: string;
        releaseTime: string;
        releaseTimeIso: string;
        validatorAddress: string;
        lockedShare: string;
      } | null;
    }>();
    const fixture = createKeeperFixture({
      chainState: {
        inputBalance: "500",
        lpBalance: "0",
        delegatedLpBalance: "0",
        bondedLockedLpBalance: "250",
        provideDelegatePromise: deferred.promise
      }
    });
    const locks = new StrategyLocks();
    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks,
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400"
    });

    const firstRun = runner.runTick();
    const secondRun = runner.runTick();

    await Promise.resolve();
    deferred.resolve({
      txHash: "provide-delegate-1",
      lpAmount: "250"
    });

    await Promise.all([firstRun, secondRun]);

    expect(fixture.chain.provideDelegateCalls).toBe(1);
  });

  it("skips a strategy while cooldown is still active", async () => {
    const fixture = createKeeperFixture({
      strategies: [
        {
          ...baseStrategy,
          nextEligibleAt: new Date("2026-04-23T13:00:00.000Z")
        }
      ]
    });

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400"
    });

    const result = await runner.runTick();

    expect(result).toEqual([]);
    expect(fixture.chain.provideCalls).toBe(0);
  });

  it("uses the combined single-asset provide+delegate path by default", async () => {
    const fixture = createKeeperFixture({
      grants: [
        {
          ...baseGrant,
          stakingGrantExpiresAt: null,
          stakingGrantStatus: "pending"
        }
      ],
      chainState: {
        inputBalance: "500",
        lpBalance: "0",
        delegatedLpBalance: "0",
        bondedLockedLpBalance: "250",
        provideDelegateResult: {
          txHash: "provide-delegate-1",
          lpAmount: "250"
        }
      }
    });

    const runner = createKeeperRunner({
      now: () => now,
      usersRepository: fixture.usersRepository,
      strategiesRepository: fixture.strategiesRepository,
      grantsRepository: fixture.grantsRepository,
      executionsRepository: fixture.executionsRepository,
      positionsRepository: fixture.positionsRepository,
      chain: fixture.chain,
      locks: new StrategyLocks(),
      lockStakingModuleAddress: "0xlock",
      lockStakingModuleName: "lock_staking",
      lockupSeconds: "86400"
    });

    const result = await runner.runTick();

    expect(result[0]).toMatchObject({
      strategyId: "strategy-1",
      outcome: "executed",
      reason: "success"
    });
    expect(fixture.chain.provideDelegateCalls).toBe(1);
    expect(fixture.chain.provideCalls).toBe(0);
    expect(fixture.chain.delegateCalls).toBe(0);
    expect(fixture.executionsRepository.list()[0]).toMatchObject({
      status: "success",
      provideTxHash: "provide-delegate-1",
      delegateTxHash: "provide-delegate-1",
      lpAmount: "250"
    });
    expect(fixture.positionsRepository.list()[0]).toMatchObject({
      lastInputBalance: "500",
      lastLpBalance: "0",
      lastDelegatedLpBalance: "250",
      lastRewardSnapshot: JSON.stringify({
        kind: "bonded-locked",
        stakingAccount: "0xdryrunstakingaccount",
        metadata: "pool-1",
        releaseTime: "1777032000",
        releaseTimeIso: "2026-04-24T12:00:00.000Z",
        validatorAddress: "initvaloper1validator",
        lockedShare: "250"
      })
    });
  });
});
