import type { KeeperChainClient } from "./types.js";

export async function getDelegatedLpBalance(
  chain: KeeperChainClient,
  input: {
    userAddress: string;
    validatorAddress: string;
    lpDenom: string;
  }
) {
  return chain.getDelegatedLpBalance(input);
}
