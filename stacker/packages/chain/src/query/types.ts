export type KeeperMode = "dry-run" | "live";

type SingleAssetProvideBaseRequest = {
  userAddress: string;
  targetPoolId: string;
  inputDenom: string;
  lpDenom: string;
  amount: string;
  maxSlippageBps: string;
  moduleAddress: string;
  moduleName: string;
};

export type ProvideSingleAssetLiquidityRequest = SingleAssetProvideBaseRequest;

export type ProvideSingleAssetLiquidityResult = {
  txHash: string;
  lpAmount: string;
};

export type SingleAssetProvideDelegateRequest =
  SingleAssetProvideBaseRequest & {
    releaseTime: string;
    validatorAddress: string;
  };

export type BondedLockRewardSnapshot = {
  kind: "bonded-locked";
  stakingAccount: string;
  metadata: string;
  releaseTime: string;
  releaseTimeIso: string;
  validatorAddress: string;
  lockedShare: string;
};

export type SingleAssetProvideDelegateResult = {
  txHash: string;
  lpAmount: string;
  rewardSnapshot?: BondedLockRewardSnapshot | null;
};

export type DelegateLpRequest = {
  userAddress: string;
  validatorAddress: string;
  lpDenom: string;
  amount: string;
};

export type DelegateLpResult = {
  txHash: string;
};

export interface KeeperChainClient {
  readonly mode: KeeperMode;
  getInputBalance(input: {
    userAddress: string;
    denom: string;
  }): Promise<string>;
  getLpBalance(input: {
    userAddress: string;
    lpDenom: string;
  }): Promise<string>;
  getDelegatedLpBalance(input: {
    userAddress: string;
    validatorAddress: string;
    lpDenom: string;
  }): Promise<string>;
  getBondedLockedLpBalance(input: {
    userAddress: string;
    targetPoolId: string;
    validatorAddress: string;
    moduleAddress: string;
    moduleName: string;
  }): Promise<string>;
  provideSingleAssetLiquidity(
    input: ProvideSingleAssetLiquidityRequest
  ): Promise<ProvideSingleAssetLiquidityResult>;
  singleAssetProvideDelegate(
    input: SingleAssetProvideDelegateRequest
  ): Promise<SingleAssetProvideDelegateResult>;
  delegateLp(input: DelegateLpRequest): Promise<DelegateLpResult>;
  isTxConfirmed(txHash: string): Promise<boolean>;
}
