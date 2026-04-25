import type { KeeperChainClient } from "./types.js";

export async function getBondedLockedLpBalance(
  chain: KeeperChainClient,
  input: {
    userAddress: string;
    targetPoolId: string;
    validatorAddress: string;
    moduleAddress: string;
    moduleName: string;
  }
) {
  return chain.getBondedLockedLpBalance(input);
}
