import { z } from "zod";
import type { StackerEnvironment } from "./public-types.js";

const environmentSchema = z.object({
  DATABASE_URL: z.url(),
  KEEPER_PRIVATE_KEY: z.string().min(1),
  INITIA_LCD_URL: z.url(),
  INITIA_RPC_URL: z.url(),
  KEEPER_ADDRESS: z.string().min(1),
  TARGET_POOL_ID: z.string().min(1),
  DEX_MODULE_ADDRESS: z.string().min(1),
  DEX_MODULE_NAME: z.string().min(1),
  LOCK_STAKING_MODULE_ADDRESS: z.string().min(1),
  LOCK_STAKING_MODULE_NAME: z.string().min(1).optional(),
  LOCKUP_SECONDS: z.string().min(1)
});

export function parseEnvironment(
  input: Record<string, string | undefined>
): StackerEnvironment {
  const result = environmentSchema.safeParse(input);

  if (!result.success) {
    const issues = result.error.issues
      .map((issue) => `${issue.path.join(".")}: ${issue.message}`)
      .join("; ");

    throw new Error(`Invalid environment configuration: ${issues}`);
  }

  return {
    databaseUrl: result.data.DATABASE_URL,
    keeperPrivateKey: result.data.KEEPER_PRIVATE_KEY,
    initiaLcdUrl: result.data.INITIA_LCD_URL,
    initiaRpcUrl: result.data.INITIA_RPC_URL,
    keeperAddress: result.data.KEEPER_ADDRESS,
    targetPoolId: result.data.TARGET_POOL_ID,
    dexModuleAddress: result.data.DEX_MODULE_ADDRESS,
    dexModuleName: result.data.DEX_MODULE_NAME,
    lockStakingModuleAddress: result.data.LOCK_STAKING_MODULE_ADDRESS,
    lockStakingModuleName: result.data.LOCK_STAKING_MODULE_NAME,
    lockupSeconds: result.data.LOCKUP_SECONDS
  };
}

export function loadEnvironment(
  input: Record<string, string | undefined> = process.env
): StackerEnvironment {
  return parseEnvironment(input);
}
