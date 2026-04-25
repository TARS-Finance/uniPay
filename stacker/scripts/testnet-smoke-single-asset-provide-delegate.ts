import { bcs, RESTClient, RawKey, Wallet } from "@initia/initia.js";
import { buildDirectSingleAssetProvideDelegate } from "../packages/chain/src/index.js";

type SmokeConfig = {
  lcdUrl: string;
  gasPrices: string;
  gasAdjustment: string;
  privateKey: string;
  targetPoolId: string;
  inputDenom: string;
  amount: string;
  maxSlippageBps: string;
  lockStakingModuleAddress: string;
  lockStakingModuleName: string;
  validatorAddress: string;
  lockupSeconds: string;
  explorerBaseUrl: string;
  confirmBroadcast: boolean;
};

type TxEvent = {
  attributes: Array<{ key: string; value: string }>;
};

function loadEnvFileIfPresent() {
  const candidate = process as typeof process & {
    loadEnvFile?: (path?: string) => void;
  };

  candidate.loadEnvFile?.(".env");
}

function required(name: string) {
  const value = process.env[name];

  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }

  return value;
}

function loadConfig(): SmokeConfig {
  return {
    lcdUrl: process.env.SMOKE_LCD_URL ?? process.env.INITIA_LCD_URL ?? "https://rest.testnet.initia.xyz",
    gasPrices: process.env.SMOKE_GAS_PRICES ?? process.env.INITIA_GAS_PRICES ?? "0.015uinit",
    gasAdjustment: process.env.SMOKE_GAS_ADJUSTMENT ?? process.env.INITIA_GAS_ADJUSTMENT ?? "1.75",
    privateKey: required("SMOKE_PRIVATE_KEY"),
    targetPoolId: process.env.SMOKE_TARGET_POOL_ID ?? process.env.TARGET_POOL_ID ?? required("SMOKE_TARGET_POOL_ID"),
    inputDenom: required("SMOKE_INPUT_DENOM"),
    amount: required("SMOKE_AMOUNT"),
    maxSlippageBps: process.env.SMOKE_MAX_SLIPPAGE_BPS ?? "100",
    lockStakingModuleAddress:
      process.env.SMOKE_LOCK_STAKING_MODULE_ADDRESS
      ?? process.env.LOCK_STAKING_MODULE_ADDRESS
      ?? required("SMOKE_LOCK_STAKING_MODULE_ADDRESS"),
    lockStakingModuleName:
      process.env.SMOKE_LOCK_STAKING_MODULE_NAME
      ?? process.env.LOCK_STAKING_MODULE_NAME
      ?? "lock_staking",
    validatorAddress: required("SMOKE_VALIDATOR_ADDRESS"),
    lockupSeconds: process.env.SMOKE_LOCKUP_SECONDS ?? process.env.LOCKUP_SECONDS ?? "86400",
    explorerBaseUrl:
      process.env.SMOKE_EXPLORER_BASE_URL
      ?? "https://scan.testnet.initia.xyz/initiation-2/txs",
    confirmBroadcast: process.env.SMOKE_CONFIRM_BROADCAST === "true"
  };
}

function applySlippageBps(amount: bigint, slippageBps: string) {
  const bps = BigInt(slippageBps);

  if (bps < 0n || bps > 10_000n) {
    throw new Error(`Invalid slippage bps: ${slippageBps}`);
  }

  return (amount * (10_000n - bps)) / 10_000n;
}

function extractQuotedLiquidity(events: TxEvent[], targetPoolId: string) {
  let liquidity = 0n;

  for (const event of events) {
    const attributes = new Map(
      event.attributes.map((attribute) => [attribute.key, attribute.value])
    );

    if (attributes.get("type_tag") !== "0x1::dex::ProvideEvent") {
      continue;
    }

    if (attributes.get("liquidity_token") !== targetPoolId) {
      continue;
    }

    const minted = attributes.get("liquidity");

    if (minted) {
      liquidity += BigInt(minted);
    }
  }

  return liquidity;
}

async function pollTx(rest: RESTClient, txHash: string) {
  for (let attempt = 0; attempt < 10; attempt += 1) {
    try {
      return await rest.tx.txInfo(txHash);
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 1_500));
    }
  }

  return null;
}

async function main() {
  loadEnvFileIfPresent();
  const config = loadConfig();
  const key = RawKey.fromHex(config.privateKey);
  const rest = new RESTClient(config.lcdUrl, {
    gasPrices: config.gasPrices,
    gasAdjustment: config.gasAdjustment
  });
  const wallet = new Wallet(rest, key);
  const inputMetadata = await rest.move.metadata(config.inputDenom);
  const releaseTime = BigInt(
    Math.floor(Date.now() / 1000) + Number(config.lockupSeconds)
  );

  const simulationMsg = buildDirectSingleAssetProvideDelegate({
    userAddress: key.accAddress,
    moduleAddress: config.lockStakingModuleAddress,
    moduleName: config.lockStakingModuleName,
    args: [
      bcs.object().serialize(config.targetPoolId).toBase64(),
      bcs.object().serialize(inputMetadata).toBase64(),
      bcs.u64().serialize(BigInt(config.amount)).toBase64(),
      bcs.option(bcs.u64()).serialize(null).toBase64(),
      bcs.u64().serialize(releaseTime).toBase64(),
      bcs.string().serialize(config.validatorAddress).toBase64()
    ]
  });
  const simulation = await rest.tx.simulate({
    msgs: [simulationMsg],
    sequence: await wallet.sequence()
  });
  const quotedLiquidity = extractQuotedLiquidity(
    simulation.result.events as TxEvent[],
    config.targetPoolId
  );

  if (quotedLiquidity <= 0n) {
    throw new Error(
      `Simulation did not emit a usable ProvideEvent for pool ${config.targetPoolId}.`
    );
  }

  const minLiquidity = applySlippageBps(
    quotedLiquidity,
    config.maxSlippageBps
  );

  console.log("direct smoke prepared");
  console.log(`user address: ${key.accAddress}`);
  console.log(`input denom: ${config.inputDenom}`);
  console.log(`input metadata object: ${inputMetadata}`);
  console.log(`pool id: ${config.targetPoolId}`);
  console.log(`validator: ${config.validatorAddress}`);
  console.log(`amount in: ${config.amount}`);
  console.log(`quoted liquidity: ${quotedLiquidity}`);
  console.log(`min liquidity: ${minLiquidity}`);
  console.log(`release time: ${releaseTime} (${new Date(Number(releaseTime) * 1000).toISOString()})`);

  if (!config.confirmBroadcast) {
    console.log("preview only: set SMOKE_CONFIRM_BROADCAST=true to broadcast");
    return;
  }

  const msg = buildDirectSingleAssetProvideDelegate({
    userAddress: key.accAddress,
    moduleAddress: config.lockStakingModuleAddress,
    moduleName: config.lockStakingModuleName,
    args: [
      bcs.object().serialize(config.targetPoolId).toBase64(),
      bcs.object().serialize(inputMetadata).toBase64(),
      bcs.u64().serialize(BigInt(config.amount)).toBase64(),
      bcs.option(bcs.u64()).serialize(minLiquidity).toBase64(),
      bcs.u64().serialize(releaseTime).toBase64(),
      bcs.string().serialize(config.validatorAddress).toBase64()
    ]
  });
  const signedTx = await wallet.createAndSignTx({
    msgs: [msg]
  });
  const broadcast = await rest.tx.broadcast(signedTx);

  console.log(`tx hash: ${broadcast.txhash}`);
  console.log(`explorer: ${config.explorerBaseUrl}/${broadcast.txhash}`);

  if ("code" in broadcast && broadcast.code !== undefined && String(broadcast.code) !== "0") {
    throw new Error(
      `Broadcast failed (${broadcast.code}): ${broadcast.raw_log}`
    );
  }

  const txInfo = await pollTx(rest, broadcast.txhash);

  if (!txInfo) {
    console.log("tx submitted, but confirmation polling timed out");
    return;
  }

  console.log("tx confirmed");
  console.log(JSON.stringify(txInfo, null, 2));
}

await main();
