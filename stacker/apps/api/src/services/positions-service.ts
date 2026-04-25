import {
  ExecutionsRepository,
  PositionsRepository,
  StrategiesRepository
} from "@stacker/db";
import { getDelegatedLpKind } from "./position-mode.js";
import { parseRewardLockSnapshot } from "./reward-lock.js";
import type { ChainService } from "./chain-service.js";

function sumBigIntStrings(values: string[]) {
  return values.reduce((total, value) => total + BigInt(value), 0n).toString();
}

function denomToSymbol(denom: string): string {
  if (denom === "uusdc" || denom === "usdc" || denom === "iusdc") return "USDC";
  if (denom === "uinit") return "INIT";
  return denom.toUpperCase();
}

function poolName(inputDenom: string): string {
  return `${denomToSymbol(inputDenom)} / INIT`;
}

export class PositionsService {
  constructor(
    private readonly positionsRepository: PositionsRepository,
    private readonly strategiesRepository: StrategiesRepository,
    private readonly executionsRepository: ExecutionsRepository,
    private readonly chainService?: ChainService,
    private readonly chainConfig?: {
      keeperAddress: string;
      targetPoolId: string;
      validatorAddress: string;
      moduleAddress: string;
      moduleName: string;
    }
  ) {}

  private async fetchYield(principalStaked: bigint): Promise<string> {
    if (!this.chainService || !this.chainConfig) return "0";
    try {
      const yield_ = await this.chainService.computeYieldEarned({
        ...this.chainConfig,
        principalStaked,
      });
      return yield_.toString();
    } catch {
      return "0";
    }
  }

  private async fetchClaimableInitRewards(userInitiaAddress: string | undefined): Promise<string> {
    if (!userInitiaAddress || !this.chainService || !this.chainConfig) return "0";
    try {
      const stakingAddr = await this.chainService.getStakingAddress({
        userAddress: userInitiaAddress,
        moduleAddress: this.chainConfig.moduleAddress,
        moduleName: this.chainConfig.moduleName,
      });
      if (!stakingAddr) return "0";
      const rewards = await this.chainService.getClaimableInitRewards(stakingAddr);
      return rewards.toString();
    } catch {
      return "0";
    }
  }

  async listByUserId(userId: string) {
    const positions = await this.positionsRepository.listByUserId(userId);
    const strategies = await this.strategiesRepository.findByUserId(userId);
    const strategiesById = new Map(
      strategies.map((strategy) => [strategy.id, strategy])
    );
    const delegatedLpKind = getDelegatedLpKind();

    return positions.map((position) => ({
      inputDenom: strategiesById.get(position.strategyId)?.inputDenom ?? null,
      strategyId: position.strategyId,
      targetPoolId: strategiesById.get(position.strategyId)?.targetPoolId ?? null,
      validatorAddress:
        strategiesById.get(position.strategyId)?.validatorAddress ?? null,
      executionMode: "single-asset-provide-delegate" as const,
      delegatedLpKind,
      lastInputBalance: position.lastInputBalance,
      lastLpBalance: position.lastLpBalance,
      lastDelegatedLpBalance: position.lastDelegatedLpBalance,
      lastRewardSnapshot: position.lastRewardSnapshot,
      rewardLock: parseRewardLockSnapshot(position.lastRewardSnapshot),
      lastSyncedAt: position.lastSyncedAt.toISOString()
    }));
  }

  async getMerchantBalance(userId: string, apyBps: number, userInitiaAddress?: string) {
    const [positions, executions] = await Promise.all([
      this.positionsRepository.listByUserId(userId),
      this.executionsRepository.listByUserId(userId)
    ]);

    const principalAvailable = sumBigIntStrings(
      positions.map((position) => position.lastInputBalance)
    );
    const principalStaked = sumBigIntStrings(
      executions
        .filter(
          (execution) =>
            execution.status === "success" || execution.status === "simulated"
        )
        .map((execution) => execution.inputAmount)
    );
    const [yieldEarned, claimableInitRewards] = await Promise.all([
      this.fetchYield(BigInt(principalStaked)),
      this.fetchClaimableInitRewards(userInitiaAddress),
    ]);

    return {
      principal_available: principalAvailable,
      principal_staked: principalStaked,
      yield_earned: yieldEarned,
      claimable_init_rewards: claimableInitRewards,
      apy_bps: apyBps
    };
  }

  async getMerchantOverview(userId: string, apyBps: number, userInitiaAddress?: string) {
    const [positions, strategies, executions] = await Promise.all([
      this.positionsRepository.listByUserId(userId),
      this.strategiesRepository.findByUserId(userId),
      this.executionsRepository.listByUserId(userId)
    ]);

    const principalAvailable = sumBigIntStrings(
      positions.map((p) => p.lastInputBalance)
    );

    const successfulExecs = executions.filter(
      (e) => e.status === "success" || e.status === "simulated"
    );
    const principalStaked = sumBigIntStrings(
      successfulExecs.map((e) => e.inputAmount)
    );

    const activeStrategies = strategies.filter(
      (s) => s.status === "active" || s.status === "executing" || s.status === "partial_lp"
    );

    const [yieldEarned, claimableInitRewards] = await Promise.all([
      this.fetchYield(BigInt(principalStaked)),
      this.fetchClaimableInitRewards(userInitiaAddress),
    ]);

    return {
      principal_available: principalAvailable,
      principal_staked: principalStaked,
      yield_earned: yieldEarned,
      claimable_init_rewards: claimableInitRewards,
      apy_bps: apyBps,
      pool_count: activeStrategies.length,
      total_executions: successfulExecs.length
    };
  }

  async getMerchantPools(userId: string, apyBps: number) {
    const [positions, strategies, executions] = await Promise.all([
      this.positionsRepository.listByUserId(userId),
      this.strategiesRepository.findByUserId(userId),
      this.executionsRepository.listByUserId(userId)
    ]);

    const positionsByStrategyId = new Map(
      positions.map((p) => [p.strategyId, p])
    );
    const executionsByStrategyId = new Map<string, typeof executions>();
    for (const exec of executions) {
      const list = executionsByStrategyId.get(exec.strategyId) ?? [];
      list.push(exec);
      executionsByStrategyId.set(exec.strategyId, list);
    }

    // Compute per-strategy staked amounts first
    const stakedByStrategyId = new Map<string, bigint>();
    for (const strategy of strategies) {
      const stratExecs = executionsByStrategyId.get(strategy.id) ?? [];
      const staked = sumBigIntStrings(
        stratExecs
          .filter((e) => e.status === "success" || e.status === "simulated")
          .map((e) => e.inputAmount)
      );
      stakedByStrategyId.set(strategy.id, BigInt(staked));
    }

    const totalStaked = [...stakedByStrategyId.values()].reduce((a, b) => a + b, 0n);
    const totalYieldStr = await this.fetchYield(totalStaked);
    const totalYield = BigInt(totalYieldStr);

    return strategies.map((strategy) => {
      const position = positionsByStrategyId.get(strategy.id);
      const staked = stakedByStrategyId.get(strategy.id) ?? 0n;
      const stratExecs = executionsByStrategyId.get(strategy.id) ?? [];

      // Distribute yield proportionally to this strategy's staked share
      const earned =
        totalStaked > 0n
          ? (totalYield * staked) / totalStaked
          : 0n;

      return {
        id: strategy.id,
        poolId: strategy.targetPoolId,
        name: poolName(strategy.inputDenom),
        inputDenom: strategy.inputDenom,
        tokens: [denomToSymbol(strategy.inputDenom), "INIT"] as [string, string],
        staked: staked.toString(),
        available: position?.lastInputBalance ?? "0",
        apy_bps: apyBps,
        earned: earned.toString(),
        status: strategy.status,
        lastExecutedAt: strategy.lastExecutedAt?.toISOString() ?? null,
        executionCount: stratExecs.filter(
          (e) => e.status === "success" || e.status === "simulated"
        ).length
      };
    });
  }

  async getMerchantActivity(
    userId: string,
    limit = 50,
    explorerBase = "https://scan.initia.xyz",
    chainId = "initiation-2"
  ) {
    const [executions, strategies] = await Promise.all([
      this.executionsRepository.listByUserId(userId),
      this.strategiesRepository.findByUserId(userId)
    ]);

    const strategiesById = new Map(strategies.map((s) => [s.id, s]));

    return executions.slice(0, limit).map((exec) => {
      const strategy = strategiesById.get(exec.strategyId);
      const staked = exec.status === "success" || exec.status === "simulated";
      // Prefer the delegate (staking) tx; fall back to the provide (LP) tx
      const primaryHash = exec.delegateTxHash ?? exec.provideTxHash ?? null;
      const txUrl = primaryHash
        ? `${explorerBase}/${chainId}/txs/${primaryHash}`
        : null;

      return {
        id: exec.id,
        strategyId: exec.strategyId,
        inputDenom: strategy?.inputDenom ?? "uusdc",
        amount: exec.inputAmount,
        lpAmount: exec.lpAmount ?? "0",
        status: exec.status,
        staked,
        provideTxHash: exec.provideTxHash ?? null,
        delegateTxHash: exec.delegateTxHash ?? null,
        txUrl,
        errorMessage: exec.errorMessage ?? null,
        startedAt: exec.startedAt.toISOString(),
        finishedAt: exec.finishedAt?.toISOString() ?? null
      };
    });
  }

  async getMerchantChart(userId: string) {
    const executions = await this.executionsRepository.listByUserId(userId);

    const successfulExecs = executions
      .filter((e) => e.status === "success" || e.status === "simulated")
      .sort((a, b) => a.startedAt.getTime() - b.startedAt.getTime());

    // Build daily cumulative staked total
    const dayMap = new Map<string, bigint>();
    let cumulative = 0n;

    for (const exec of successfulExecs) {
      const day = exec.startedAt.toISOString().slice(0, 10);
      cumulative += BigInt(exec.inputAmount);
      dayMap.set(day, cumulative);
    }

    const points = Array.from(dayMap.entries()).map(([date, staked]) => ({
      date,
      cumulative_staked: staked.toString()
    }));

    // Pad with today if there's no data
    if (points.length === 0) {
      points.push({
        date: new Date().toISOString().slice(0, 10),
        cumulative_staked: "0"
      });
    }

    return { points };
  }
}
