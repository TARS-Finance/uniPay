import {
  ExecutionsRepository,
  GrantsRepository,
  PositionsRepository,
  StrategiesRepository
} from "@stacker/db";
import { canTransitionStrategyStatus, type InputDenom } from "@stacker/shared";
import type { ApiConfig } from "../config.js";
import { getDelegatedLpKind } from "./position-mode.js";
import { parseRewardLockSnapshot } from "./reward-lock.js";

export type CreateStrategyInput = {
  userId: string;
  inputDenom: InputDenom;
  targetPoolId: string;
  validatorAddress: string;
  minBalanceAmount: string;
  maxAmountPerRun: string;
  maxSlippageBps: number;
  cooldownSeconds: number;
};

export class StrategiesService {
  constructor(
    private readonly strategiesRepository: StrategiesRepository,
    private readonly grantsRepository: GrantsRepository,
    private readonly positionsRepository: PositionsRepository,
    private readonly executionsRepository: ExecutionsRepository,
    private readonly config: ApiConfig
  ) {}

  async create(input: CreateStrategyInput) {
    return this.strategiesRepository.create({
      userId: input.userId,
      status: this.config.executionMode === "direct" ? "active" : "grant_pending",
      inputDenom: input.inputDenom,
      targetPoolId: input.targetPoolId,
      dexModuleAddress: this.config.dexModuleAddress,
      dexModuleName: this.config.dexModuleName,
      validatorAddress: input.validatorAddress,
      minBalanceAmount: input.minBalanceAmount,
      maxAmountPerRun: input.maxAmountPerRun,
      maxSlippageBps: String(input.maxSlippageBps),
      cooldownSeconds: String(input.cooldownSeconds)
    });
  }

  async getStatus(strategyId: string) {
    const strategy = await this.strategiesRepository.findById(strategyId);

    if (!strategy) {
      return null;
    }

    const grant = await this.grantsRepository.findByUserId(strategy.userId);
    const position = await this.positionsRepository.findByStrategyId(strategy.id);
    const lastExecution =
      await this.executionsRepository.findLatestForStrategy(strategy.id);
    const grantStatus =
      this.config.executionMode === "direct"
        ? {
            move: "not-required" as const,
            staking: "not-required" as const,
            feegrant: "not-required" as const,
            expiresAt: null
          }
        : {
            move: grant?.moveGrantStatus ?? "pending",
            staking: "not-required" as const,
            feegrant: grant?.feegrantStatus ?? "pending",
            expiresAt: grant?.moveGrantExpiresAt?.toISOString() ?? null
          };

    return {
      strategyId: strategy.id,
      status: strategy.status,
      executionMode: "single-asset-provide-delegate" as const,
      grantStatus,
      balances: {
        input: position?.lastInputBalance ?? "0",
        lp: position?.lastLpBalance ?? "0",
        delegatedLp: position?.lastDelegatedLpBalance ?? "0",
        delegatedLpKind: getDelegatedLpKind()
      },
      rewardLock: parseRewardLockSnapshot(position?.lastRewardSnapshot ?? null),
      lastExecution: lastExecution
        ? {
            status: lastExecution.status,
            provideTxHash: lastExecution.provideTxHash,
            delegateTxHash: lastExecution.delegateTxHash,
            finishedAt: lastExecution.finishedAt?.toISOString()
          }
        : null
    };
  }

  async pause(strategyId: string) {
    const strategy = await this.strategiesRepository.findById(strategyId);

    if (!strategy) {
      return null;
    }

    if (
      strategy.status !== "paused"
      && !canTransitionStrategyStatus(strategy.status, "paused")
    ) {
      return null;
    }

    return this.strategiesRepository.patch(strategyId, {
      status: "paused",
      pauseReason: "user-requested"
    });
  }

  async resume(strategyId: string) {
    const strategy = await this.strategiesRepository.findById(strategyId);

    if (!strategy) {
      return null;
    }

    if (
      strategy.status !== "active"
      && !canTransitionStrategyStatus(strategy.status, "active")
    ) {
      return null;
    }

    return this.strategiesRepository.patch(strategyId, {
      status: "active",
      pauseReason: null
    });
  }
}
