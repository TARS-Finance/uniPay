import { config as loadDotEnv } from "dotenv";
import { resolve } from "node:path";
import { loadEnvironment } from "@stacker/shared";

const ROOT_ENV_PATH = resolve(import.meta.dirname, "../../../.env");

export type KeeperConfig = {
  databaseUrl: string;
  initiaLcdUrl: string;
  initiaChainId?: string;
  keeperPrivateKey: string;
  keeperAddress: string;
  executionMode: "authz" | "direct";
  dexModuleAddress: string;
  dexModuleName: string;
  lockStakingModuleAddress: string;
  lockStakingModuleName: string;
  lockupSeconds: string;
  lpDenom: string;
  mode: "dry-run" | "live";
  dryRunInputBalance: string;
  gasPrices?: string;
  gasAdjustment?: string;
  pollIntervalMs: number;
  logLevel: string;
  logPretty: boolean;
};

export function loadKeeperConfig(
  overrides: Partial<KeeperConfig> = {}
): KeeperConfig {
  loadDotEnv({ path: ROOT_ENV_PATH, quiet: true, override: true });

  const environment = loadEnvironment({
    ...process.env,
    DATABASE_URL: overrides.databaseUrl ?? process.env.DATABASE_URL,
    INITIA_LCD_URL: overrides.initiaLcdUrl ?? process.env.INITIA_LCD_URL,
    KEEPER_PRIVATE_KEY:
      overrides.keeperPrivateKey ?? process.env.KEEPER_PRIVATE_KEY,
    KEEPER_ADDRESS: overrides.keeperAddress ?? process.env.KEEPER_ADDRESS,
    DEX_MODULE_ADDRESS:
      overrides.dexModuleAddress ?? process.env.DEX_MODULE_ADDRESS,
    DEX_MODULE_NAME: overrides.dexModuleName ?? process.env.DEX_MODULE_NAME,
    LOCK_STAKING_MODULE_ADDRESS:
      overrides.lockStakingModuleAddress
      ?? process.env.LOCK_STAKING_MODULE_ADDRESS,
    LOCK_STAKING_MODULE_NAME:
      overrides.lockStakingModuleName
      ?? process.env.LOCK_STAKING_MODULE_NAME,
    LOCKUP_SECONDS: overrides.lockupSeconds ?? process.env.LOCKUP_SECONDS
  });

  return {
    databaseUrl: environment.databaseUrl,
    initiaLcdUrl: environment.initiaLcdUrl,
    keeperPrivateKey: environment.keeperPrivateKey,
    keeperAddress: environment.keeperAddress,
    executionMode:
      process.env.KEEPER_EXECUTION_MODE === "direct" ? "direct" : "authz",
    dexModuleAddress: environment.dexModuleAddress,
    dexModuleName: environment.dexModuleName,
    lockStakingModuleAddress: environment.lockStakingModuleAddress,
    lockStakingModuleName: environment.lockStakingModuleName ?? "lock_staking",
    lockupSeconds: environment.lockupSeconds,
    lpDenom: process.env.LP_DENOM ?? "ulp",
    mode: process.env.KEEPER_MODE === "live" ? "live" : "dry-run",
    dryRunInputBalance: process.env.KEEPER_DRY_RUN_INPUT_BALANCE ?? "0",
    initiaChainId: process.env.INITIA_CHAIN_ID,
    gasPrices: process.env.INITIA_GAS_PRICES,
    gasAdjustment: process.env.INITIA_GAS_ADJUSTMENT,
    pollIntervalMs: Number(process.env.KEEPER_POLL_INTERVAL_MS ?? "60000"),
    logLevel: process.env.KEEPER_LOG_LEVEL ?? "info",
    logPretty:
      process.env.KEEPER_LOG_PRETTY === undefined
        ? process.stdout.isTTY
        : process.env.KEEPER_LOG_PRETTY === "true",
    ...overrides
  };
}
