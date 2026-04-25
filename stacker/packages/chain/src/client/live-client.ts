import {
  bcs,
  RawKey,
  RESTClient,
  type RESTClientConfig,
  type Tx,
  Wallet,
} from "@initia/initia.js";
import { delegateLp as buildDelegateLpMsg } from "../staking/delegate-lp.js";
import { provideSingleAssetLiquidity as buildProvideLiquidityMsg } from "../dex/provide-single-asset-liquidity.js";
import { buildDirectSingleAssetProvideDelegate } from "../vip/build-direct-single-asset-provide-delegate.js";
import { singleAssetProvideDelegate as buildProvideDelegateMsg } from "../vip/single-asset-provide-delegate.js";
import type {
  BondedLockRewardSnapshot,
  DelegateLpRequest,
  DelegateLpResult,
  KeeperChainClient,
  ProvideSingleAssetLiquidityRequest,
  ProvideSingleAssetLiquidityResult,
  SingleAssetProvideDelegateRequest,
  SingleAssetProvideDelegateResult,
} from "../query/types.js";

type CoinLike = {
  amount: string;
};

type CoinsLike = {
  get(denom: string): CoinLike | undefined;
};

type WalletLike = {
  createAndSignTx(input: { msgs: unknown[] }): Promise<Tx | unknown>;
  sequence(): Promise<number>;
};

type InjectedWalletLike = WalletLike & {
  accAddress?: string;
};

type RestClientLike = {
  bank: {
    balanceByDenom(address: string, denom: string): Promise<CoinLike | undefined>;
  };
  move: {
    metadata(denom: string): Promise<string>;
    viewFunction?(
      moduleAddress: string,
      moduleName: string,
      functionName: string,
      typeArgs: string[],
      args: string[]
    ): Promise<unknown>;
  };
  mstaking: {
    delegation(
      delegator: string,
      validator: string
    ): Promise<{ balance: CoinsLike }>;
  };
  tx: {
    simulate(input: {
      msgs: unknown[];
      sequence: number;
    }): Promise<{
      result: {
        events: Array<{
          type: string;
          attributes: Array<{ key: string; value: string }>;
        }>;
      };
    }>;
    broadcast(tx: Tx | unknown): Promise<{
      txhash: string;
      raw_log: string;
      code?: number | string;
    }>;
    txInfo(txHash: string): Promise<unknown>;
  };
};

export type CreateLiveKeeperChainClientInput = {
  lcdUrl: string;
  privateKey: string;
  keeperAddress: string;
  chainId?: string;
  gasPrices?: string;
  gasAdjustment?: string;
  restClient?: RestClientLike;
  wallet?: InjectedWalletLike;
  executionMode?: "authz" | "direct";
};

function isNotFoundError(error: unknown) {
  if (!error || typeof error !== "object") {
    return false;
  }

  const candidate = error as {
    status?: number;
    response?: {
      status?: number;
    };
  };

  return candidate.status === 404 || candidate.response?.status === 404;
}

function isBroadcastError(result: {
  code?: number | string;
}) {
  if (result.code === undefined) {
    return false;
  }

  return String(result.code) !== "0";
}

function applySlippageBps(amount: bigint, slippageBps: string) {
  const bps = BigInt(slippageBps);

  if (bps < 0n || bps > 10_000n) {
    throw new Error(`Invalid slippage bps: ${slippageBps}`);
  }

  return (amount * (10_000n - bps)) / 10_000n;
}

type BondedLockedDelegation = {
  metadata?: unknown;
  validator?: unknown;
  amount?: unknown;
};

type TxEventAttribute = {
  key: string;
  value: string;
};

type TxInfoLike = {
  events?: Array<{
    type?: string;
    attributes?: TxEventAttribute[];
  }>;
};

function normalizeObjectAddress(value: string) {
  return value.toLowerCase();
}

function extractBondedLockedDelegations(value: unknown): BondedLockedDelegation[] {
  if (Array.isArray(value)) {
    return value.filter(
      (item): item is BondedLockedDelegation =>
        Boolean(item) && typeof item === "object"
    );
  }

  if (
    value
    && typeof value === "object"
    && "data" in value
    && Array.isArray((value as { data?: unknown[] }).data)
  ) {
    return (value as { data: unknown[] }).data.filter(
      (item): item is BondedLockedDelegation =>
        Boolean(item) && typeof item === "object"
    );
  }

  return [];
}

function extractRewardSnapshotFromTxInfo(input: {
  txInfo: unknown;
  moduleAddress: string;
  targetPoolId: string;
  validatorAddress: string;
}): BondedLockRewardSnapshot | null {
  const txInfo =
    input.txInfo && typeof input.txInfo === "object"
      ? (input.txInfo as TxInfoLike)
      : null;
  const events = txInfo?.events;

  if (!events) {
    return null;
  }

  const targetPoolId = normalizeObjectAddress(input.targetPoolId);

  for (const event of events) {
    const attributes = new Map(
      (event.attributes ?? []).map((attribute) => [attribute.key, attribute.value])
    );

    if (
      attributes.get("type_tag")
      !== `${input.moduleAddress}::lock_staking::DepositDelegationEvent`
    ) {
      continue;
    }

    const metadata = attributes.get("metadata");
    const validatorAddress = attributes.get("validator");
    const stakingAccount = attributes.get("staking_account");
    const releaseTime = attributes.get("release_time");
    const lockedShare = attributes.get("locked_share");

    if (
      !metadata
      || normalizeObjectAddress(metadata) !== targetPoolId
      || validatorAddress !== input.validatorAddress
      || !stakingAccount
      || !releaseTime
      || !lockedShare
    ) {
      continue;
    }

    return {
      kind: "bonded-locked",
      stakingAccount,
      metadata,
      releaseTime,
      releaseTimeIso: new Date(Number(releaseTime) * 1000).toISOString(),
      validatorAddress,
      lockedShare
    };
  }

  return null;
}

async function tryReadRewardSnapshot(input: {
  rest: RestClientLike;
  txHash: string;
  moduleAddress: string;
  targetPoolId: string;
  validatorAddress: string;
}) {
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      const txInfo = await input.rest.tx.txInfo(input.txHash);
      return extractRewardSnapshotFromTxInfo({
        txInfo,
        moduleAddress: input.moduleAddress,
        targetPoolId: input.targetPoolId,
        validatorAddress: input.validatorAddress
      });
    } catch (error) {
      if (!isNotFoundError(error) || attempt === 4) {
        return null;
      }

      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }

  return null;
}

function extractLpQuoteFromSimulation(input: {
  events: Array<{
    type: string;
    attributes: Array<{ key: string; value: string }>;
  }>;
  targetPoolId: string;
}) {
  let lpAmount = 0n;

  for (const event of input.events) {
    const attributes = new Map(
      event.attributes.map((attribute) => [attribute.key, attribute.value])
    );
    const eventType = attributes.get("type_tag");
    const liquidityToken = attributes.get("liquidity_token");
    const liquidity = attributes.get("liquidity");

    if (
      eventType !== "0x1::dex::ProvideEvent"
      || liquidityToken !== input.targetPoolId
      || !liquidity
    ) {
      continue;
    }

    lpAmount += BigInt(liquidity);
  }

  return lpAmount;
}

async function queryBalanceByDenom(
  rest: RestClientLike,
  address: string,
  denom: string
) {
  try {
    const coin = await rest.bank.balanceByDenom(address, denom);
    return coin?.amount ?? "0";
  } catch (error) {
    if (isNotFoundError(error)) {
      return "0";
    }

    throw error;
  }
}

async function simulateQuotedLpAmount(input: {
  rest: RestClientLike;
  wallet: WalletLike;
  msg: unknown;
  targetPoolId: string;
  lpDenom: string;
}) {
  const simulation = await input.rest.tx.simulate({
    msgs: [input.msg],
    sequence: await input.wallet.sequence(),
  });
  const quotedLpAmount = extractLpQuoteFromSimulation({
    events: simulation.result.events,
    targetPoolId: input.targetPoolId,
  });

  if (quotedLpAmount <= 0n) {
    throw new Error(
      `Unable to derive a trustworthy LP quote for ${input.lpDenom} from simulation output.`
    );
  }

  return quotedLpAmount;
}

export class LiveKeeperChainClient implements KeeperChainClient {
  readonly mode = "live" as const;

  constructor(
    private readonly rest: RestClientLike,
    private readonly wallet: WalletLike,
    private readonly keeperAddress: string,
    private readonly signerAddress: string,
    private readonly executionMode: "authz" | "direct" = "authz"
  ) {
    if (executionMode === "authz" && signerAddress !== keeperAddress) {
      throw new Error(
        `Configured keeper address ${keeperAddress} does not match derived wallet address ${signerAddress}`
      );
    }
  }

  async getInputBalance(request: {
    userAddress: string;
    denom: string;
  }): Promise<string> {
    return queryBalanceByDenom(this.rest, request.userAddress, request.denom);
  }

  async getLpBalance(request: {
    userAddress: string;
    lpDenom: string;
  }): Promise<string> {
    return queryBalanceByDenom(this.rest, request.userAddress, request.lpDenom);
  }

  async getDelegatedLpBalance(request: {
    userAddress: string;
    validatorAddress: string;
    lpDenom: string;
  }): Promise<string> {
    try {
      const delegation = await this.rest.mstaking.delegation(
        request.userAddress,
        request.validatorAddress
      );

      return delegation.balance.get(request.lpDenom)?.amount ?? "0";
    } catch (error) {
      if (isNotFoundError(error)) {
        return "0";
      }

      throw error;
    }
  }

  async getBondedLockedLpBalance(request: {
    userAddress: string;
    targetPoolId: string;
    validatorAddress: string;
    moduleAddress: string;
    moduleName: string;
  }): Promise<string> {
    if (!this.rest.move.viewFunction) {
      throw new Error("Configured REST client does not support Move view functions.");
    }

    try {
      const response = await this.rest.move.viewFunction(
        request.moduleAddress,
        request.moduleName,
        "get_bonded_locked_delegations",
        [],
        [bcs.address().serialize(request.userAddress).toBase64()]
      );

      const targetPoolId = normalizeObjectAddress(request.targetPoolId);
      let total = 0n;

      for (const item of extractBondedLockedDelegations(response)) {
        const metadata =
          typeof item.metadata === "string"
            ? normalizeObjectAddress(item.metadata)
            : null;
        const validator =
          typeof item.validator === "string" ? item.validator : null;
        const amount = typeof item.amount === "string" ? item.amount : null;

        if (
          metadata !== targetPoolId
          || validator !== request.validatorAddress
          || !amount
        ) {
          continue;
        }

        total += BigInt(amount);
      }

      return total.toString();
    } catch (error) {
      if (isNotFoundError(error)) {
        return "0";
      }

      throw error;
    }
  }

  async provideSingleAssetLiquidity(
    request: ProvideSingleAssetLiquidityRequest
  ): Promise<ProvideSingleAssetLiquidityResult> {
    const beforeLpBalance = BigInt(
      await this.getLpBalance({
        userAddress: request.userAddress,
        lpDenom: request.lpDenom,
      })
    );
    const inputCoinMetadata = await this.rest.move.metadata(request.inputDenom);
    const simulationMsg = buildProvideLiquidityMsg({
      grantee: this.keeperAddress,
      userAddress: request.userAddress,
      moduleAddress: request.moduleAddress,
      moduleName: request.moduleName,
      args: [
        bcs.object().serialize(request.targetPoolId).toBase64(),
        bcs.object().serialize(inputCoinMetadata).toBase64(),
        bcs.u64().serialize(BigInt(request.amount)).toBase64(),
        bcs.option(bcs.u64()).serialize(null).toBase64(),
      ],
    });
    const quotedLpAmount = await simulateQuotedLpAmount({
      rest: this.rest,
      wallet: this.wallet,
      msg: simulationMsg,
      targetPoolId: request.targetPoolId,
      lpDenom: request.lpDenom,
    });

    const minLiquidity = applySlippageBps(
      quotedLpAmount,
      request.maxSlippageBps
    );
    const msg = buildProvideLiquidityMsg({
      grantee: this.keeperAddress,
      userAddress: request.userAddress,
      moduleAddress: request.moduleAddress,
      moduleName: request.moduleName,
      args: [
        bcs.object().serialize(request.targetPoolId).toBase64(),
        bcs.object().serialize(inputCoinMetadata).toBase64(),
        bcs.u64().serialize(BigInt(request.amount)).toBase64(),
        bcs.option(bcs.u64()).serialize(minLiquidity).toBase64(),
      ],
    });
    const signedTx = await this.wallet.createAndSignTx({
      msgs: [msg],
    });
    const broadcast = await this.rest.tx.broadcast(signedTx);

    if (isBroadcastError(broadcast)) {
      throw new Error(
        `Provide liquidity tx failed (${broadcast.code}): ${broadcast.raw_log}`
      );
    }

    const afterLpBalance = BigInt(
      await this.getLpBalance({
        userAddress: request.userAddress,
        lpDenom: request.lpDenom,
      })
    );

    return {
      txHash: broadcast.txhash,
      lpAmount: (afterLpBalance - beforeLpBalance).toString(),
    };
  }

  async singleAssetProvideDelegate(
    request: SingleAssetProvideDelegateRequest
  ): Promise<SingleAssetProvideDelegateResult> {
    if (
      this.executionMode === "direct"
      && request.userAddress !== this.signerAddress
    ) {
      throw new Error(
        `Configured direct signer ${this.signerAddress} does not match requested user address ${request.userAddress}`
      );
    }

    const beforeDelegatedBalance = BigInt(
      await this.getBondedLockedLpBalance({
        userAddress: request.userAddress,
        targetPoolId: request.targetPoolId,
        validatorAddress: request.validatorAddress,
        moduleAddress: request.moduleAddress,
        moduleName: request.moduleName,
      })
    );
    const inputCoinMetadata = await this.rest.move.metadata(request.inputDenom);
    const buildMessage = (minLiquidity: bigint | null) => {
      const args = [
        bcs.object().serialize(request.targetPoolId).toBase64(),
        bcs.object().serialize(inputCoinMetadata).toBase64(),
        bcs.u64().serialize(BigInt(request.amount)).toBase64(),
        bcs.option(bcs.u64()).serialize(minLiquidity).toBase64(),
        bcs.u64().serialize(BigInt(request.releaseTime)).toBase64(),
        bcs.string().serialize(request.validatorAddress).toBase64(),
      ];

      return this.executionMode === "direct"
        ? buildDirectSingleAssetProvideDelegate({
            userAddress: request.userAddress,
            moduleAddress: request.moduleAddress,
            moduleName: request.moduleName,
            args,
          })
        : buildProvideDelegateMsg({
            grantee: this.keeperAddress,
            userAddress: request.userAddress,
            moduleAddress: request.moduleAddress,
            moduleName: request.moduleName,
            args,
          });
    };
    const simulationMsg = buildMessage(null);
    const quotedLpAmount = await simulateQuotedLpAmount({
      rest: this.rest,
      wallet: this.wallet,
      msg: simulationMsg,
      targetPoolId: request.targetPoolId,
      lpDenom: request.lpDenom,
    });
    const minLiquidity = applySlippageBps(
      quotedLpAmount,
      request.maxSlippageBps
    );
    const msg = buildMessage(minLiquidity);
    const signedTx = await this.wallet.createAndSignTx({
      msgs: [msg],
    });
    const broadcast = await this.rest.tx.broadcast(signedTx);

    if (isBroadcastError(broadcast)) {
      throw new Error(
        `Provide+delegate tx failed (${broadcast.code}): ${broadcast.raw_log}`
      );
    }

    const afterDelegatedBalance = BigInt(
      await this.getBondedLockedLpBalance({
        userAddress: request.userAddress,
        targetPoolId: request.targetPoolId,
        validatorAddress: request.validatorAddress,
        moduleAddress: request.moduleAddress,
        moduleName: request.moduleName,
      })
    );
    const delegatedDelta = afterDelegatedBalance - beforeDelegatedBalance;
    const rewardSnapshot = await tryReadRewardSnapshot({
      rest: this.rest,
      txHash: broadcast.txhash,
      moduleAddress: request.moduleAddress,
      targetPoolId: request.targetPoolId,
      validatorAddress: request.validatorAddress
    });

    return {
      txHash: broadcast.txhash,
      lpAmount: (delegatedDelta > 0n ? delegatedDelta : quotedLpAmount).toString(),
      rewardSnapshot,
    };
  }

  async delegateLp(request: DelegateLpRequest): Promise<DelegateLpResult> {
    const msg = buildDelegateLpMsg({
      grantee: this.keeperAddress,
      userAddress: request.userAddress,
      validatorAddress: request.validatorAddress,
      lpDenom: request.lpDenom,
      amount: request.amount,
    });
    const signedTx = await this.wallet.createAndSignTx({
      msgs: [msg],
    });
    const broadcast = await this.rest.tx.broadcast(signedTx);

    if (isBroadcastError(broadcast)) {
      throw new Error(`Delegate tx failed (${broadcast.code}): ${broadcast.raw_log}`);
    }

    return {
      txHash: broadcast.txhash,
    };
  }

  async isTxConfirmed(txHash: string): Promise<boolean> {
    try {
      await this.rest.tx.txInfo(txHash);
      return true;
    } catch (error) {
      if (isNotFoundError(error)) {
        return false;
      }

      throw error;
    }
  }
}

export function createLiveKeeperChainClient(
  input: CreateLiveKeeperChainClientInput
) {
  const key = RawKey.fromHex(input.privateKey);
  const rest: RestClientLike =
    input.restClient
      ? input.restClient
      : new RESTClient(input.lcdUrl, {
          chainId: input.chainId,
          gasPrices: input.gasPrices,
          gasAdjustment: input.gasAdjustment,
        } satisfies RESTClientConfig);
  const wallet =
    input.wallet
    ?? new Wallet(rest as RESTClient, key);
  const signerAddress =
    input.wallet && "accAddress" in input.wallet && typeof input.wallet.accAddress === "string"
      ? input.wallet.accAddress
      : key.accAddress;

  return new LiveKeeperChainClient(
    rest,
    wallet,
    input.keeperAddress,
    signerAddress,
    input.executionMode ?? "authz"
  );
}
