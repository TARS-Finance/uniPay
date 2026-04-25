import { config as loadDotEnv } from "dotenv";
import { resolve } from "node:path";
import { loadEnvironment } from "@stacker/shared";

const ROOT_ENV_PATH = resolve(import.meta.dirname, "../../../.env");

export type ApiConfig = {
  port: number;
  databaseUrl: string;
  initiaLcdUrl: string;
  executionMode: "authz" | "direct";
  keeperAddress: string;
  dexModuleAddress: string;
  dexModuleName: string;
  lockStakingModuleAddress: string;
  lockStakingModuleName: string;
  lockupSeconds: string;
  feeDenom: string;
  lpDenom: string;
  grantExpiryHours: number;
  merchantDemoApyBps: number;
  merchantInputDenom: string;
  merchantValidatorAddress: string;
  targetPoolId: string;
  initiaChainId: string;
  initiaExplorerUrl: string;
};

export function loadApiConfig(overrides: Partial<ApiConfig> = {}): ApiConfig {
  loadDotEnv({ path: ROOT_ENV_PATH, quiet: true, override: true });
  const defaultExecutionMode =
    process.env.NODE_ENV === "test"
      ? "authz"
      : process.env.KEEPER_EXECUTION_MODE === "direct"
        ? "direct"
        : "authz";

  const env = loadEnvironment({
    ...process.env,
    DATABASE_URL: overrides.databaseUrl ?? process.env.DATABASE_URL,
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
    port: Number(process.env.API_PORT ?? "3000"),
    databaseUrl: env.databaseUrl,
    initiaLcdUrl: env.initiaLcdUrl,
    executionMode: defaultExecutionMode,
    keeperAddress: env.keeperAddress,
    dexModuleAddress: env.dexModuleAddress,
    dexModuleName: env.dexModuleName,
    lockStakingModuleAddress: env.lockStakingModuleAddress,
    lockStakingModuleName: env.lockStakingModuleName ?? "lock_staking",
    lockupSeconds: env.lockupSeconds,
    feeDenom: process.env.FEE_DENOM ?? "uinit",
    lpDenom: process.env.LP_DENOM ?? "ulp",
    grantExpiryHours: Number(process.env.GRANT_EXPIRY_HOURS ?? "720"),
    merchantDemoApyBps: Number(process.env.MERCHANT_DEMO_APY_BPS ?? "0"),
    merchantInputDenom: process.env.MERCHANT_INPUT_DENOM ?? "uusdc",
    merchantValidatorAddress: process.env.MERCHANT_VALIDATOR_ADDRESS ?? "",
    targetPoolId: process.env.TARGET_POOL_ID ?? "",
    initiaChainId: process.env.INITIA_CHAIN_ID ?? "initiation-2",
    initiaExplorerUrl: process.env.INITIA_EXPLORER_URL ?? "https://scan.initia.xyz",
    ...overrides
  };
}
