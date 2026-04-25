import { delegateLp as buildDelegateLpMsg } from "../staking/delegate-lp.js";
import { provideSingleAssetLiquidity as buildProvideLiquidityMsg } from "../dex/provide-single-asset-liquidity.js";
import { singleAssetProvideDelegate as buildProvideDelegateMsg } from "../vip/single-asset-provide-delegate.js";
import type {
  BondedLockRewardSnapshot,
  DelegateLpRequest,
  DelegateLpResult,
  KeeperChainClient,
  ProvideSingleAssetLiquidityRequest,
  ProvideSingleAssetLiquidityResult,
  SingleAssetProvideDelegateRequest,
  SingleAssetProvideDelegateResult
} from "../query/types.js";

type DryRunInput = {
  keeperAddress: string;
  lpDenom: string;
  startingBalances?: Record<string, string>;
  defaultInputBalance?: string;
};

type PlannedMessage = {
  "@type": string;
};

function userBalanceKey(userAddress: string, denom: string): string {
  return `${userAddress}:${denom}`;
}

function delegatedBalanceKey(
  userAddress: string,
  validatorAddress: string,
  denom: string
): string {
  return `${userAddress}:${validatorAddress}:${denom}`;
}

function bondedLockedBalanceKey(
  userAddress: string,
  validatorAddress: string,
  targetPoolId: string
): string {
  return `${userAddress}:${validatorAddress}:${targetPoolId}:bonded-locked`;
}

function encodeProvideArgs(input: ProvideSingleAssetLiquidityRequest): string[] {
  return [
    Buffer.from(
      JSON.stringify({
        targetPoolId: input.targetPoolId,
        inputDenom: input.inputDenom,
        amount: input.amount,
        maxSlippageBps: input.maxSlippageBps
      }),
      "utf8"
    ).toString("base64")
  ];
}

function buildDryRunRewardSnapshot(
  input: SingleAssetProvideDelegateRequest
): BondedLockRewardSnapshot {
  return {
    kind: "bonded-locked",
    stakingAccount: "0xdryrunstakingaccount",
    metadata: input.targetPoolId,
    releaseTime: input.releaseTime,
    releaseTimeIso: new Date(
      Number(input.releaseTime) * 1000
    ).toISOString(),
    validatorAddress: input.validatorAddress,
    lockedShare: input.amount
  };
}

function encodeProvideDelegateArgs(
  input: SingleAssetProvideDelegateRequest
): string[] {
  return [
    Buffer.from(
      JSON.stringify({
        targetPoolId: input.targetPoolId,
        inputDenom: input.inputDenom,
        amount: input.amount,
        maxSlippageBps: input.maxSlippageBps,
        releaseTime: input.releaseTime,
        validatorAddress: input.validatorAddress
      }),
      "utf8"
    ).toString("base64")
  ];
}

export class DryRunKeeperChainClient implements KeeperChainClient {
  readonly mode = "dry-run" as const;
  readonly broadcastCalls = 0;
  private readonly plannedMessages: PlannedMessage[] = [];
  private readonly balances = new Map<string, bigint>();
  private txSequence = 0;

  constructor(private readonly input: DryRunInput) {
    Object.entries(input.startingBalances ?? {}).forEach(([key, value]) => {
      this.balances.set(key, BigInt(value));
    });
  }

  getPlannedMessages(): PlannedMessage[] {
    return [...this.plannedMessages];
  }

  async getInputBalance(request: {
    userAddress: string;
    denom: string;
  }): Promise<string> {
    const key = userBalanceKey(request.userAddress, request.denom);
    const value =
      this.balances.get(key)
      ?? (request.denom === this.input.lpDenom
        ? 0n
        : BigInt(this.input.defaultInputBalance ?? "0"));

    return value.toString();
  }

  async getLpBalance(request: {
    userAddress: string;
    lpDenom: string;
  }): Promise<string> {
    const key = userBalanceKey(request.userAddress, request.lpDenom);
    return (this.balances.get(key) ?? 0n).toString();
  }

  async getDelegatedLpBalance(request: {
    userAddress: string;
    validatorAddress: string;
    lpDenom: string;
  }): Promise<string> {
    const key = delegatedBalanceKey(
      request.userAddress,
      request.validatorAddress,
      request.lpDenom
    );

    return (this.balances.get(key) ?? 0n).toString();
  }

  async getBondedLockedLpBalance(request: {
    userAddress: string;
    targetPoolId: string;
    validatorAddress: string;
    moduleAddress: string;
    moduleName: string;
  }): Promise<string> {
    const key = bondedLockedBalanceKey(
      request.userAddress,
      request.validatorAddress,
      request.targetPoolId
    );

    return (this.balances.get(key) ?? 0n).toString();
  }

  async provideSingleAssetLiquidity(
    request: ProvideSingleAssetLiquidityRequest
  ): Promise<ProvideSingleAssetLiquidityResult> {
    const inputKey = userBalanceKey(request.userAddress, request.inputDenom);
    const lpKey = userBalanceKey(request.userAddress, request.lpDenom);
    const currentInput = BigInt(await this.getInputBalance({
      userAddress: request.userAddress,
      denom: request.inputDenom
    }));
    const amount = BigInt(request.amount);

    if (currentInput < amount) {
      throw new Error("Insufficient dry-run balance");
    }

    this.balances.set(inputKey, currentInput - amount);
    this.balances.set(lpKey, (this.balances.get(lpKey) ?? 0n) + amount);
    this.plannedMessages.push(
      buildProvideLiquidityMsg({
        grantee: this.input.keeperAddress,
        userAddress: request.userAddress,
        moduleAddress: request.moduleAddress,
        moduleName: request.moduleName,
        args: encodeProvideArgs(request)
      }).toData()
    );

    const txHash = `dry-run-provide-${++this.txSequence}`;

    return {
      txHash,
      lpAmount: amount.toString()
    };
  }

  async singleAssetProvideDelegate(
    request: SingleAssetProvideDelegateRequest
  ): Promise<SingleAssetProvideDelegateResult> {
    const inputKey = userBalanceKey(request.userAddress, request.inputDenom);
    const delegatedKey = delegatedBalanceKey(
      request.userAddress,
      request.validatorAddress,
      request.lpDenom
    );
    const bondedLockedKey = bondedLockedBalanceKey(
      request.userAddress,
      request.validatorAddress,
      request.targetPoolId
    );
    const currentInput = BigInt(await this.getInputBalance({
      userAddress: request.userAddress,
      denom: request.inputDenom
    }));
    const amount = BigInt(request.amount);

    if (currentInput < amount) {
      throw new Error("Insufficient dry-run balance");
    }

    this.balances.set(inputKey, currentInput - amount);
    this.balances.set(
      delegatedKey,
      (this.balances.get(delegatedKey) ?? 0n) + amount
    );
    this.balances.set(
      bondedLockedKey,
      (this.balances.get(bondedLockedKey) ?? 0n) + amount
    );
    this.plannedMessages.push(
      buildProvideDelegateMsg({
        grantee: this.input.keeperAddress,
        userAddress: request.userAddress,
        moduleAddress: request.moduleAddress,
        moduleName: request.moduleName,
        args: encodeProvideDelegateArgs(request)
      }).toData()
    );

    return {
      txHash: `dry-run-provide-delegate-${++this.txSequence}`,
      lpAmount: amount.toString(),
      rewardSnapshot: buildDryRunRewardSnapshot(request)
    };
  }

  async delegateLp(request: DelegateLpRequest): Promise<DelegateLpResult> {
    const lpKey = userBalanceKey(request.userAddress, request.lpDenom);
    const delegatedKey = delegatedBalanceKey(
      request.userAddress,
      request.validatorAddress,
      request.lpDenom
    );
    const currentLp = BigInt(await this.getLpBalance({
      userAddress: request.userAddress,
      lpDenom: request.lpDenom
    }));
    const amount = BigInt(request.amount);

    if (currentLp < amount) {
      throw new Error("Insufficient dry-run LP balance");
    }

    this.balances.set(lpKey, currentLp - amount);
    this.balances.set(
      delegatedKey,
      (this.balances.get(delegatedKey) ?? 0n) + amount
    );
    this.plannedMessages.push(
      buildDelegateLpMsg({
        grantee: this.input.keeperAddress,
        userAddress: request.userAddress,
        validatorAddress: request.validatorAddress,
        lpDenom: request.lpDenom,
        amount: request.amount
      }).toData()
    );

    return {
      txHash: `dry-run-delegate-${++this.txSequence}`
    };
  }

  async isTxConfirmed(): Promise<boolean> {
    return true;
  }
}

export function createDryRunKeeperChainClient(input: DryRunInput) {
  return new DryRunKeeperChainClient(input);
}
