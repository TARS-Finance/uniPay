import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { and, eq, ne } from "drizzle-orm";
import {
  StrategiesRepository,
  UsersRepository,
  executions,
  grants,
  openDatabase,
  strategies
} from "../packages/db/src/index.js";
import { positions } from "../packages/db/src/index.js";
import type { InputDenom } from "../packages/shared/src/index.js";

type DemoSeedConfig = {
  merchantInitiaAddress: string;
  executionMode: "authz" | "direct";
  inputDenom: InputDenom;
  targetPoolId: string;
  validatorAddress: string;
  minBalanceAmount: string;
  maxAmountPerRun: string;
  maxSlippageBps: string;
  cooldownSeconds: string;
  dexModuleAddress: string;
  dexModuleName: string;
};

const ROOT_ENV_PATH = resolve(import.meta.dirname, "../.env");

function loadEnvFileIfPresent() {
  if (!existsSync(ROOT_ENV_PATH)) {
    return;
  }

  const contents = readFileSync(ROOT_ENV_PATH, "utf8");

  for (const rawLine of contents.split(/\r?\n/u)) {
    const line = rawLine.trim();

    if (!line || line.startsWith("#")) {
      continue;
    }

    const separatorIndex = line.indexOf("=");

    if (separatorIndex <= 0) {
      continue;
    }

    const name = line.slice(0, separatorIndex).trim();
    const value = line.slice(separatorIndex + 1).trim();

    process.env[name] = value;
  }
}

function required(name: string) {
  const value = process.env[name];

  if (!value) {
    throw new Error(`Missing required environment variable: ${name}`);
  }

  return value;
}

function loadConfig(): DemoSeedConfig {
  return {
    merchantInitiaAddress:
      process.env.MERCHANT_INITIA_ADDRESS
      ?? process.env.KEEPER_ADDRESS
      ?? required("MERCHANT_INITIA_ADDRESS"),
    executionMode:
      process.env.KEEPER_EXECUTION_MODE === "direct" ? "direct" : "authz",
    inputDenom: (process.env.MERCHANT_INPUT_DENOM ?? "uusdc") as InputDenom,
    targetPoolId:
      process.env.TARGET_POOL_ID ?? required("TARGET_POOL_ID"),
    validatorAddress:
      process.env.MERCHANT_VALIDATOR_ADDRESS
      ?? required("MERCHANT_VALIDATOR_ADDRESS"),
    minBalanceAmount: process.env.MERCHANT_MIN_BALANCE_AMOUNT ?? "100",
    maxAmountPerRun: process.env.MERCHANT_MAX_AMOUNT_PER_RUN ?? "10000000",
    maxSlippageBps: process.env.MERCHANT_MAX_SLIPPAGE_BPS ?? "100",
    cooldownSeconds: process.env.MERCHANT_COOLDOWN_SECONDS ?? "10",
    dexModuleAddress:
      process.env.DEX_MODULE_ADDRESS ?? required("DEX_MODULE_ADDRESS"),
    dexModuleName: process.env.DEX_MODULE_NAME ?? "dex"
  };
}

async function main() {
  loadEnvFileIfPresent();
  const config = loadConfig();
  const { client, db } = openDatabase();
  const usersRepository = new UsersRepository(db);
  const strategiesRepository = new StrategiesRepository(db);

  await client.connect();

  try {
    const existingUser = await usersRepository.findByInitiaAddress(
      config.merchantInitiaAddress
    );
    const user =
      existingUser
      ?? await usersRepository.create(config.merchantInitiaAddress);
    await db.delete(executions).where(eq(executions.userId, user.id));
    await db.delete(positions).where(eq(positions.userId, user.id));
    await db.delete(grants).where(eq(grants.userId, user.id));
    const existingStrategies = await strategiesRepository.findByUserId(user.id);
    const existingStrategy =
      existingStrategies.find(
        (strategy) => strategy.targetPoolId === config.targetPoolId
      )
      ?? existingStrategies[0]
      ?? null;
    const strategyValues = {
      userId: user.id,
      status: config.executionMode === "direct" ? "active" : "grant_pending",
      inputDenom: config.inputDenom,
      targetPoolId: config.targetPoolId,
      dexModuleAddress: config.dexModuleAddress,
      dexModuleName: config.dexModuleName,
      validatorAddress: config.validatorAddress,
      minBalanceAmount: config.minBalanceAmount,
      maxAmountPerRun: config.maxAmountPerRun,
      maxSlippageBps: config.maxSlippageBps,
      cooldownSeconds: config.cooldownSeconds,
      lastExecutedAt: null,
      nextEligibleAt: null,
      pauseReason: null
    } as const;

    const strategy =
      existingStrategy
      ? await strategiesRepository.patch(existingStrategy.id, strategyValues)
      : await strategiesRepository.create(strategyValues);
    let pausedOtherStrategies = 0;

    if (config.executionMode === "direct") {
      const paused = await db
        .update(strategies)
        .set({
          status: "paused",
          pauseReason: `demo-seed disabled for ${config.merchantInitiaAddress}`,
          updatedAt: new Date()
        })
        .where(
          and(
            ne(strategies.id, strategy.id),
            ne(strategies.status, "paused")
          )
        )
        .returning({ id: strategies.id });

      pausedOtherStrategies = paused.length;
    }

    console.log("demo merchant seeded");
    console.log(`merchant: ${user.initiaAddress}`);
    console.log(`user id: ${user.id}`);
    console.log(`strategy id: ${strategy.id}`);
    console.log(`strategy status: ${strategy.status}`);
    console.log(`input denom: ${strategy.inputDenom}`);
    console.log(`target pool id: ${strategy.targetPoolId}`);
    console.log(`validator: ${strategy.validatorAddress}`);
    console.log(`paused other strategies: ${pausedOtherStrategies}`);
  } finally {
    await client.end();
  }
}

await main();
