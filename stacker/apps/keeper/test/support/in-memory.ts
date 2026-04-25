type StrategyStatus =
  | "draft"
  | "grant_pending"
  | "active"
  | "executing"
  | "partial_lp"
  | "paused"
  | "expired"
  | "error";

type ExecutionStatus =
  | "queued"
  | "providing"
  | "delegating"
  | "simulated"
  | "success"
  | "failed"
  | "retryable";

export type UserRecord = {
  id: string;
  initiaAddress: string;
};

export type StrategyRecord = {
  id: string;
  userId: string;
  status: StrategyStatus;
  inputDenom: "usdc" | "iusdc" | "uusdc";
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

export type GrantRecord = {
  userId: string;
  keeperAddress: string;
  moveGrantExpiresAt: Date | null;
  stakingGrantExpiresAt: Date | null;
  feegrantExpiresAt: Date | null;
  moveGrantStatus: "pending" | "active" | "revoked" | "expired";
  stakingGrantStatus: "pending" | "active" | "revoked" | "expired";
  feegrantStatus: "pending" | "active" | "revoked" | "expired";
};

export type ExecutionRecord = {
  id: string;
  strategyId: string;
  userId: string;
  status: ExecutionStatus;
  inputAmount: string;
  lpAmount: string | null;
  provideTxHash: string | null;
  delegateTxHash: string | null;
  errorCode: string | null;
  errorMessage: string | null;
  startedAt: Date;
  finishedAt: Date | null;
};

export type PositionRecord = {
  id: string;
  strategyId: string;
  userId: string;
  lastInputBalance: string;
  lastLpBalance: string;
  lastDelegatedLpBalance: string;
  lastRewardSnapshot: string | null;
  lastSyncedAt: Date;
};

export class Deferred<T> {
  promise: Promise<T>;
  resolve!: (value: T) => void;
  reject!: (error: unknown) => void;

  constructor() {
    this.promise = new Promise<T>((resolve, reject) => {
      this.resolve = resolve;
      this.reject = reject;
    });
  }
}

export class InMemoryUsersRepository {
  constructor(private readonly users: UserRecord[]) {}

  async findById(id: string) {
    return this.users.find((user) => user.id === id) ?? null;
  }
}

export class InMemoryStrategiesRepository {
  constructor(private readonly strategies: StrategyRecord[]) {}

  async findRunnableStrategies(now: Date) {
    return this.strategies.filter(
      (strategy) =>
        strategy.nextEligibleAt === null || strategy.nextEligibleAt <= now,
    );
  }

  async patch(id: string, values: Partial<StrategyRecord>) {
    const strategy = this.strategies.find((item) => item.id === id);

    if (!strategy) {
      throw new Error(`Strategy ${id} not found`);
    }

    Object.assign(strategy, values);

    return strategy;
  }

  getById(id: string) {
    return this.strategies.find((item) => item.id === id) ?? null;
  }
}

export class InMemoryGrantsRepository {
  constructor(private readonly grants: GrantRecord[]) {}

  async findByUserId(userId: string) {
    return this.grants.find((grant) => grant.userId === userId) ?? null;
  }
}

let executionSequence = 0;

export class InMemoryExecutionsRepository {
  constructor(private readonly executions: ExecutionRecord[]) {}

  async create(values: Omit<ExecutionRecord, "id">) {
    const execution: ExecutionRecord = {
      id: `execution-${++executionSequence}`,
      ...values,
    };

    this.executions.push(execution);
    return execution;
  }

  async findLatestForStrategy(strategyId: string) {
    const matches = this.executions
      .filter((execution) => execution.strategyId === strategyId)
      .sort(
        (left, right) => right.startedAt.getTime() - left.startedAt.getTime(),
      );

    return matches[0] ?? null;
  }

  async update(id: string, values: Partial<ExecutionRecord>) {
    const execution = this.executions.find((item) => item.id === id);

    if (!execution) {
      throw new Error(`Execution ${id} not found`);
    }

    Object.assign(execution, values);

    return execution;
  }

  list() {
    return this.executions;
  }
}

let positionSequence = 0;

export class InMemoryPositionsRepository {
  constructor(private readonly positions: PositionRecord[]) {}

  async findByStrategyId(strategyId: string) {
    return (
      this.positions.find((position) => position.strategyId === strategyId) ??
      null
    );
  }

  async upsertForStrategy(
    values: Omit<PositionRecord, "id"> & { id?: string },
  ) {
    const existing = this.positions.find(
      (position) => position.strategyId === values.strategyId,
    );

    if (existing) {
      Object.assign(existing, values);
      return existing;
    }

    const position: PositionRecord = {
      id: values.id ?? `position-${++positionSequence}`,
      strategyId: values.strategyId,
      userId: values.userId,
      lastInputBalance: values.lastInputBalance,
      lastLpBalance: values.lastLpBalance,
      lastDelegatedLpBalance: values.lastDelegatedLpBalance,
      lastRewardSnapshot: values.lastRewardSnapshot,
      lastSyncedAt: values.lastSyncedAt,
    };

    this.positions.push(position);
    return position;
  }

  list() {
    return this.positions;
  }
}

export type FakeChainState = {
  inputBalance: string;
  lpBalance: string;
  delegatedLpBalance: string;
  bondedLockedLpBalance?: string;
  provideResult?: {
    txHash: string;
    lpAmount: string;
  };
  provideDelegateResult?: {
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
  };
  provideDelegatePromise?: Promise<{
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
  }>;
  providePromise?: Promise<{
    txHash: string;
    lpAmount: string;
  }>;
  provideError?: Error;
  delegateResult?: {
    txHash: string;
  };
  delegateError?: Error;
  txConfirmations?: Record<string, boolean>;
};

export class FakeKeeperChain {
  readonly mode = "live" as const;
  provideCalls = 0;
  provideDelegateCalls = 0;
  delegateCalls = 0;

  constructor(public readonly state: FakeChainState) {}

  async getInputBalance() {
    return this.state.inputBalance;
  }

  async getLpBalance() {
    return this.state.lpBalance;
  }

  async getDelegatedLpBalance() {
    return this.state.delegatedLpBalance;
  }

  async getBondedLockedLpBalance() {
    return this.state.bondedLockedLpBalance ?? this.state.delegatedLpBalance;
  }

  async provideSingleAssetLiquidity() {
    this.provideCalls += 1;

    if (this.state.provideError) {
      throw this.state.provideError;
    }

    if (this.state.providePromise) {
      return this.state.providePromise;
    }

    if (!this.state.provideResult) {
      throw new Error("Missing provide result");
    }

    return this.state.provideResult;
  }

  async singleAssetProvideDelegate(input?: {
    targetPoolId: string;
    validatorAddress: string;
    releaseTime: string;
  }) {
    this.provideDelegateCalls += 1;

    if (this.state.provideError) {
      throw this.state.provideError;
    }

    if (this.state.provideDelegatePromise) {
      return this.state.provideDelegatePromise;
    }

    if (!this.state.provideDelegateResult) {
      throw new Error("Missing provide+delegate result");
    }

    return {
      ...this.state.provideDelegateResult,
      rewardSnapshot:
        this.state.provideDelegateResult.rewardSnapshot
        ?? (input
          ? {
              kind: "bonded-locked" as const,
              stakingAccount: "0xdryrunstakingaccount",
              metadata: input.targetPoolId,
              releaseTime: input.releaseTime,
              releaseTimeIso: new Date(
                Number(input.releaseTime) * 1000
              ).toISOString(),
              validatorAddress: input.validatorAddress,
              lockedShare: this.state.provideDelegateResult.lpAmount
            }
          : null)
    };
  }

  async delegateLp() {
    this.delegateCalls += 1;

    if (this.state.delegateError) {
      throw this.state.delegateError;
    }

    if (!this.state.delegateResult) {
      throw new Error("Missing delegate result");
    }

    return this.state.delegateResult;
  }

  async isTxConfirmed(txHash: string) {
    return this.state.txConfirmations?.[txHash] ?? true;
  }
}

export function createKeeperFixture(
  overrides: Partial<{
    users: UserRecord[];
    strategies: StrategyRecord[];
    grants: GrantRecord[];
    executions: ExecutionRecord[];
    positions: PositionRecord[];
    chainState: FakeChainState;
  }> = {},
) {
  const users = overrides.users ?? [
    {
      id: "user-1",
      initiaAddress: "init1useraddress",
    },
  ];
  const strategies = overrides.strategies ?? [
    {
      id: "strategy-1",
      userId: "user-1",
      status: "active",
      inputDenom: "usdc",
      targetPoolId: "pool-1",
      dexModuleAddress: "0x1",
      dexModuleName: "dex",
      validatorAddress: "initvaloper1validator",
      minBalanceAmount: "100",
      maxAmountPerRun: "1000",
      maxSlippageBps: "100",
      cooldownSeconds: "300",
      lastExecutedAt: null,
      nextEligibleAt: null,
      pauseReason: null,
    },
  ];
  const grants = overrides.grants ?? [
    {
      userId: "user-1",
      keeperAddress: "init1replacekeeperaddress",
      moveGrantExpiresAt: new Date("2026-05-01T00:00:00.000Z"),
      stakingGrantExpiresAt: new Date("2026-05-01T00:00:00.000Z"),
      feegrantExpiresAt: new Date("2026-05-01T00:00:00.000Z"),
      moveGrantStatus: "active",
      stakingGrantStatus: "active",
      feegrantStatus: "active",
    },
  ];
  const executions = overrides.executions ?? [];
  const positions = overrides.positions ?? [];
  const chainState = overrides.chainState ?? {
    inputBalance: "500",
    lpBalance: "250",
    delegatedLpBalance: "250",
    provideResult: {
      txHash: "provide-1",
      lpAmount: "250",
    },
    delegateResult: {
      txHash: "delegate-1",
    },
  };

  return {
    users,
    strategies,
    grants,
    executions,
    positions,
    usersRepository: new InMemoryUsersRepository(users),
    strategiesRepository: new InMemoryStrategiesRepository(strategies),
    grantsRepository: new InMemoryGrantsRepository(grants),
    executionsRepository: new InMemoryExecutionsRepository(executions),
    positionsRepository: new InMemoryPositionsRepository(positions),
    chain: new FakeKeeperChain(chainState),
  };
}
