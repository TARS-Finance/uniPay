import { describe, expect, it } from "vitest";
import { parseEnvironment } from "../src/config/env.js";

describe("parseEnvironment", () => {
  it("parses a valid environment", () => {
    const parsed = parseEnvironment({
      DATABASE_URL: "postgres://stacker:stacker@localhost:5432/stacker",
      KEEPER_PRIVATE_KEY: "test-private-key",
      INITIA_LCD_URL: "https://lcd.testnet.initia.xyz",
      INITIA_RPC_URL: "https://rpc.testnet.initia.xyz",
      KEEPER_ADDRESS: "init1keeperaddress",
      TARGET_POOL_ID: "pool-1",
      DEX_MODULE_ADDRESS: "0x1",
      DEX_MODULE_NAME: "dex",
      LOCK_STAKING_MODULE_ADDRESS: "0xlock",
      LOCKUP_SECONDS: "86400"
    });

    expect(parsed.targetPoolId).toBe("pool-1");
    expect(parsed.dexModuleName).toBe("dex");
  });

  it("fails when a required variable is missing", () => {
    expect(() =>
      parseEnvironment({
        DATABASE_URL: "postgres://stacker:stacker@localhost:5432/stacker",
        INITIA_LCD_URL: "https://lcd.testnet.initia.xyz",
        INITIA_RPC_URL: "https://rpc.testnet.initia.xyz",
        KEEPER_ADDRESS: "init1keeperaddress",
        TARGET_POOL_ID: "pool-1",
        DEX_MODULE_ADDRESS: "0x1",
        DEX_MODULE_NAME: "dex",
        LOCK_STAKING_MODULE_ADDRESS: "0xlock",
        LOCKUP_SECONDS: "86400"
      })
    ).toThrowError(/KEEPER_PRIVATE_KEY/);
  });

  it("fails when a URL is malformed", () => {
    expect(() =>
      parseEnvironment({
        DATABASE_URL: "not-a-url",
        KEEPER_PRIVATE_KEY: "test-private-key",
        INITIA_LCD_URL: "https://lcd.testnet.initia.xyz",
        INITIA_RPC_URL: "https://rpc.testnet.initia.xyz",
        KEEPER_ADDRESS: "init1keeperaddress",
        TARGET_POOL_ID: "pool-1",
        DEX_MODULE_ADDRESS: "0x1",
        DEX_MODULE_NAME: "dex",
        LOCK_STAKING_MODULE_ADDRESS: "0xlock",
        LOCKUP_SECONDS: "86400"
      })
    ).toThrowError(/DATABASE_URL/);
  });
});
