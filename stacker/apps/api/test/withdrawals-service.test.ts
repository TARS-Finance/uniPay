import { describe, expect, it, vi } from "vitest";
import { WithdrawalsService } from "../src/services/withdrawals-service.js";

describe("WithdrawalsService", () => {
  it("uses the stored locked LP share when building unbond messages", async () => {
    const strategiesRepository = {
      findById: vi.fn(async () => ({
        id: "strategy-1",
        userId: "user-1",
        targetPoolId: "pool-1",
        validatorAddress: "initvaloper1validator"
      }))
    };
    const positionsRepository = {
      findByStrategyId: vi.fn(async () => ({
        strategyId: "strategy-1",
        lastDelegatedLpBalance: "828440",
        lastRewardSnapshot: JSON.stringify({
          kind: "bonded-locked",
          stakingAccount: "0xstaking",
          metadata: "pool-1",
          releaseTime: "1777057735",
          releaseTimeIso: "2026-04-24T19:08:55.000Z",
          validatorAddress: "initvaloper1validator",
          lockedShare: "828440"
        })
      }))
    };

    const chainService = {
      getPoolInfo: vi.fn(async () => ({
        coinAAmount: 1_000_000n,
        coinBAmount: 5_000_000n,
        totalShare: 2_000_000n
      })),
      computeLpFromInput: vi.fn(() => 400_000n),
      buildUndelegateMessages: vi.fn((input: { lpAmount: bigint }) => [
        {
          typeUrl: "/initia.move.v1.MsgExecute",
          value: { lpAmount: input.lpAmount.toString() }
        }
      ]),
      getUnbondingTimeMs: vi.fn(async () => 86_400_000)
    };

    const service = new WithdrawalsService(
      {
        create: vi.fn(),
        findById: vi.fn(),
        update: vi.fn(),
        listByUserId: vi.fn()
      } as never,
      strategiesRepository as never,
      chainService as never,
      {
        lockStakingModuleAddress: "0xlock",
        lockStakingModuleName: "lock_staking",
        chainId: "initiation-1",
        explorerBase: "https://explorer.test"
      },
      positionsRepository as never
    );

    const result = await service.createUnbond({
      userId: "user-1",
      initiaAddress: "init1merchant",
      strategyId: "strategy-1",
      inputAmount: "1000000"
    });

    expect(chainService.buildUndelegateMessages).toHaveBeenCalledWith(
      expect.objectContaining({
        lpAmount: 828_440n
      })
    );
    expect(result.lpAmount).toBe("828440");
  });
});
