import {
  getBondedLockedLpBalance,
  type KeeperChainClient,
  type KeeperMode,
} from "@stacker/chain";
import {
  computeNextEligibleAt,
  isGrantBundleActive,
  minBigIntString,
  serializeError,
} from "./retry-policy.js";
import { describeInputAmount, noopLogger, type LoggerLike } from "../logger.js";
import { StrategyLocks } from "./locks.js";

type UserRecord = {
  id: string;
  initiaAddress: string;
};

type StrategyRecord = {
  id: string;
  userId: string;
  status:
    | "draft"
    | "grant_pending"
    | "active"
    | "executing"
    | "partial_lp"
    | "paused"
    | "expired"
    | "error";
  inputDenom: string;
  targetPoolId: string;
  dexModuleAddress: string;
  dexModuleName: string;
  validatorAddress: string;
  minBalanceAmount: string;
  maxAmountPerRun: string;
  maxSlippageBps: string;
  cooldownSeconds: string;
  lastExecutedAt: Date | null;
  nextEligibleAt: Date | null;
  pauseReason: string | null;
};

type GrantRecord = {
  userId: string;
  keeperAddress: string;
  moveGrantExpiresAt: Date | null;
  stakingGrantExpiresAt: Date | null;
  feegrantExpiresAt: Date | null;
  moveGrantStatus: "pending" | "active" | "revoked" | "expired";
  stakingGrantStatus: "pending" | "active" | "revoked" | "expired";
  feegrantStatus: "pending" | "active" | "revoked" | "expired";
};

type ExecutionRecord = {
  id: string;
  strategyId: string;
  userId: string;
  status:
    | "queued"
    | "providing"
    | "delegating"
    | "simulated"
    | "success"
    | "failed"
    | "retryable";
  inputAmount: string;
  lpAmount: string | null;
  provideTxHash: string | null;
  delegateTxHash: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  startedAt: Date;
  finishedAt: Date | null;
};

type PositionRecord = {
  strategyId: string;
  userId: string;
  lastInputBalance: string;
  lastLpBalance: string;
  lastDelegatedLpBalance: string;
  lastRewardSnapshot: string | null;
  lastSyncedAt: Date;
};

type KeeperDependencies = {
  now: () => Date;
  usersRepository: {
    findById(id: string): Promise<UserRecord | null>;
  };
  strategiesRepository: {
    findRunnableStrategies(now: Date): Promise<StrategyRecord[]>;
    patch(id: string, values: Partial<StrategyRecord>): Promise<StrategyRecord>;
  };
  grantsRepository: {
    findByUserId(userId: string): Promise<GrantRecord | null>;
  };
  executionsRepository: {
    create(values: Omit<ExecutionRecord, "id">): Promise<ExecutionRecord>;
    update(
      id: string,
      values: Partial<ExecutionRecord>,
    ): Promise<ExecutionRecord>;
  };
  positionsRepository: {
    findByStrategyId(strategyId: string): Promise<PositionRecord | null>;
    upsertForStrategy(
      values: Omit<PositionRecord, "id"> & { id?: string },
    ): Promise<PositionRecord>;
  };
  chain: KeeperChainClient;
  locks?: StrategyLocks;
  lpDenom?: string;
  lockStakingModuleAddress: string;
  lockStakingModuleName: string;
  lockupSeconds: string;
  requireGrants?: boolean;
  logger?: LoggerLike;
};

export type TickResult = {
  strategyId: string;
  outcome: "executed" | "skipped";
  reason:
    | "success"
    | "below-threshold"
    | "not-runnable"
    | "grant-expired"
    | "locked"
    | "provide-failed";
};

function buildResult(
  strategyId: string,
  outcome: TickResult["outcome"],
  reason: TickResult["reason"],
): TickResult {
  return { strategyId, outcome, reason };
}

export function createKeeperRunner(dependencies: KeeperDependencies) {
  const locks = dependencies.locks ?? new StrategyLocks();
  const lpDenom = dependencies.lpDenom ?? "ulp";
  const requireGrants = dependencies.requireGrants ?? true;
  const logger = dependencies.logger ?? noopLogger;
  const executionStatusForCompletion = (
    mode: KeeperMode,
  ): "success" | "simulated" => (mode === "dry-run" ? "simulated" : "success");

  async function syncPosition(
    strategy: StrategyRecord,
    user: UserRecord,
    now: Date,
    rewardSnapshot?: string | null,
  ) {
    const existingPosition = await dependencies.positionsRepository.findByStrategyId(
      strategy.id,
    );
    const [lastInputBalance, lastLpBalance, lastDelegatedLpBalance] =
      await Promise.all([
        dependencies.chain.getInputBalance({
          userAddress: user.initiaAddress,
          denom: strategy.inputDenom,
        }),
        dependencies.chain.getLpBalance({
          userAddress: user.initiaAddress,
          lpDenom,
        }),
        getBondedLockedLpBalance(dependencies.chain, {
          userAddress: user.initiaAddress,
          targetPoolId: strategy.targetPoolId,
          validatorAddress: strategy.validatorAddress,
          moduleAddress: dependencies.lockStakingModuleAddress,
          moduleName: dependencies.lockStakingModuleName,
        }),
      ]);

    await dependencies.positionsRepository.upsertForStrategy({
      strategyId: strategy.id,
      userId: strategy.userId,
      lastInputBalance,
      lastLpBalance,
      lastDelegatedLpBalance,
      lastRewardSnapshot:
        rewardSnapshot ?? existingPosition?.lastRewardSnapshot ?? null,
      lastSyncedAt: now,
    });
  }

  async function executeActiveStrategy(
    strategy: StrategyRecord,
    user: UserRecord,
    now: Date,
    strategyLogger: LoggerLike,
  ): Promise<TickResult> {
    const inputBalance = await dependencies.chain.getInputBalance({
      userAddress: user.initiaAddress,
      denom: strategy.inputDenom,
    });

    if (BigInt(inputBalance) < BigInt(strategy.minBalanceAmount)) {
      strategyLogger.info(
        {
          inputBalanceRaw: inputBalance,
          inputBalanceDisplay: describeInputAmount(strategy.inputDenom, inputBalance),
          minBalanceAmountRaw: strategy.minBalanceAmount,
          minBalanceAmountDisplay: describeInputAmount(
            strategy.inputDenom,
            strategy.minBalanceAmount
          )
        },
        "keeper strategy skipped below threshold"
      );
      return buildResult(strategy.id, "skipped", "below-threshold");
    }

    const inputAmount = minBigIntString(inputBalance, strategy.maxAmountPerRun);

    const execution = await dependencies.executionsRepository.create({
      strategyId: strategy.id,
      userId: strategy.userId,
      status: "providing",
      inputAmount,
      lpAmount: null,
      provideTxHash: null,
      delegateTxHash: null,
      errorCode: null,
      errorMessage: null,
      startedAt: now,
      finishedAt: null,
    });

    await dependencies.strategiesRepository.patch(strategy.id, {
      status: "executing",
    });

    try {
      const releaseTime = Math.floor(now.getTime() / 1000)
        + Number(dependencies.lockupSeconds);
      strategyLogger.info(
        {
          inputBalanceRaw: inputBalance,
          inputBalanceDisplay: describeInputAmount(strategy.inputDenom, inputBalance),
          minBalanceAmountRaw: strategy.minBalanceAmount,
          minBalanceAmountDisplay: describeInputAmount(
            strategy.inputDenom,
            strategy.minBalanceAmount
          ),
          maxAmountPerRunRaw: strategy.maxAmountPerRun,
          maxAmountPerRunDisplay: describeInputAmount(
            strategy.inputDenom,
            strategy.maxAmountPerRun
          ),
          inputAmountRaw: inputAmount,
          inputAmountDisplay: describeInputAmount(strategy.inputDenom, inputAmount),
          lockupSeconds: dependencies.lockupSeconds,
          releaseTime,
          releaseTimeIso: new Date(releaseTime * 1000).toISOString()
        },
        "keeper strategy executing provide+delegate"
      );
      const provided = await dependencies.chain.singleAssetProvideDelegate({
        userAddress: user.initiaAddress,
        targetPoolId: strategy.targetPoolId,
        inputDenom: strategy.inputDenom,
        lpDenom,
        amount: inputAmount,
        maxSlippageBps: strategy.maxSlippageBps,
        moduleAddress: dependencies.lockStakingModuleAddress,
        moduleName: dependencies.lockStakingModuleName,
        releaseTime: String(releaseTime),
        validatorAddress: strategy.validatorAddress,
      });

      await dependencies.executionsRepository.update(execution.id, {
        status: executionStatusForCompletion(dependencies.chain.mode),
        provideTxHash: provided.txHash,
        delegateTxHash: provided.txHash,
        lpAmount: provided.lpAmount,
        finishedAt: now,
        errorCode: null,
        errorMessage: null,
      });
      await syncPosition(
        strategy,
        user,
        now,
        provided.rewardSnapshot
          ? JSON.stringify(provided.rewardSnapshot)
          : null,
      );
      await dependencies.strategiesRepository.patch(strategy.id, {
        status: "active",
        lastExecutedAt: now,
        nextEligibleAt: computeNextEligibleAt(now, strategy.cooldownSeconds),
        pauseReason: null,
      });
      strategyLogger.info(
        {
          txHash: provided.txHash,
          lpAmountRaw: provided.lpAmount,
          rewardSnapshot: provided.rewardSnapshot ?? null
        },
        "keeper strategy provide+delegate succeeded"
      );

      return buildResult(strategy.id, "executed", "success");
    } catch (error) {
      const serializedError = serializeError(error);

      await dependencies.executionsRepository.update(execution.id, {
        status: "failed",
        errorCode: "PROVIDE_FAILED",
        errorMessage: serializedError,
        finishedAt: now,
      });
      await dependencies.strategiesRepository.patch(strategy.id, {
        status: "active",
        nextEligibleAt: computeNextEligibleAt(now, strategy.cooldownSeconds),
      });
      strategyLogger.error(
        {
          error: serializedError,
          inputAmountRaw: inputAmount,
          inputAmountDisplay: describeInputAmount(strategy.inputDenom, inputAmount)
        },
        "keeper strategy provide+delegate failed"
      );

      return buildResult(strategy.id, "skipped", "provide-failed");
    }
  }

  async function runStrategy(
    strategy: StrategyRecord,
    now: Date,
  ): Promise<TickResult> {
    if (strategy.status !== "active") {
      logger.debug(
        {
          strategyId: strategy.id,
          status: strategy.status
        },
        "keeper strategy skipped because it is not active"
      );
      return buildResult(strategy.id, "skipped", "not-runnable");
    }

    if (!locks.acquire(strategy.id)) {
      logger.debug(
        {
          strategyId: strategy.id
        },
        "keeper strategy skipped because lock is already held"
      );
      return buildResult(strategy.id, "skipped", "locked");
    }

    try {
      const user = await dependencies.usersRepository.findById(strategy.userId);
      const grant = requireGrants
        ? await dependencies.grantsRepository.findByUserId(strategy.userId)
        : null;

      if (!user) {
        logger.warn(
          {
            strategyId: strategy.id,
            userId: strategy.userId
          },
          "keeper strategy skipped because user was not found"
        );
        return buildResult(strategy.id, "skipped", "not-runnable");
      }

      const strategyLogger = logger.child({
        strategyId: strategy.id,
        userId: strategy.userId,
        userAddress: user.initiaAddress,
        inputDenom: strategy.inputDenom,
        targetPoolId: strategy.targetPoolId,
        validatorAddress: strategy.validatorAddress
      });

      if (
        requireGrants
        && !isGrantBundleActive(grant, now, {
          requiresStakingGrant: false,
        })
      ) {
        await dependencies.strategiesRepository.patch(strategy.id, {
          status: "expired",
        });
        strategyLogger.warn(
          {
            grant
          },
          "keeper strategy skipped because required grants are not active"
        );

        return buildResult(strategy.id, "skipped", "grant-expired");
      }

      return executeActiveStrategy(strategy, user, now, strategyLogger);
    } finally {
      locks.release(strategy.id);
    }
  }

  return {
    locks,
    async runTick(): Promise<TickResult[]> {
      const now = dependencies.now();
      const strategies =
        await dependencies.strategiesRepository.findRunnableStrategies(now);
      const results: TickResult[] = [];

      logger.debug(
        {
          runnableStrategyCount: strategies.length,
          tickedAt: now.toISOString()
        },
        "keeper tick started"
      );

      for (const strategy of strategies) {
        results.push(await runStrategy(strategy, now));
      }

      return results;
    },
    runStrategy,
  };
}
