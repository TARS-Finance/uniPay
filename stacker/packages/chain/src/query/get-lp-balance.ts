import type { KeeperChainClient } from "./types.js";

export async function getLpBalance(
  chain: KeeperChainClient,
  input: {
    userAddress: string;
    lpDenom: string;
  }
) {
  return chain.getLpBalance(input);
}
