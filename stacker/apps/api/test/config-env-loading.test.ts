import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { parse } from "dotenv";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { loadApiConfig } from "../src/config.js";

const REQUIRED_KEYS = [
  "DATABASE_URL",
  "KEEPER_PRIVATE_KEY",
  "INITIA_LCD_URL",
  "INITIA_RPC_URL",
  "KEEPER_ADDRESS",
  "TARGET_POOL_ID",
  "DEX_MODULE_ADDRESS",
  "DEX_MODULE_NAME",
  "LOCK_STAKING_MODULE_ADDRESS",
  "LOCK_STAKING_MODULE_NAME",
  "LOCKUP_SECONDS",
  "KEEPER_EXECUTION_MODE",
  "MERCHANT_DEMO_APY_BPS",
] as const;

describe("api config env loading", () => {
  const originalEnv = { ...process.env };
  const originalCwd = process.cwd();
  const repoRoot = resolve(import.meta.dirname, "../../..");
  const apiDir = resolve(import.meta.dirname, "..");

  beforeEach(() => {
    process.chdir(apiDir);
    process.env = { ...originalEnv };
    process.env.NODE_ENV = "development";

    for (const key of REQUIRED_KEYS) {
      delete process.env[key];
    }
  });

  afterEach(() => {
    process.chdir(originalCwd);
    process.env = { ...originalEnv };
  });

  it("loads stacker/.env even when started from apps/api", () => {
    const envFile = parse(readFileSync(resolve(repoRoot, ".env"), "utf8"));
    const config = loadApiConfig();

    expect(config.databaseUrl).toBe(envFile.DATABASE_URL);
    expect(config.initiaLcdUrl).toBe(envFile.INITIA_LCD_URL);
    expect(config.keeperAddress).toBe(envFile.KEEPER_ADDRESS);
    expect(config.executionMode).toBe("direct");
    expect(config.merchantDemoApyBps).toBe(Number(envFile.MERCHANT_DEMO_APY_BPS));
  });
});
