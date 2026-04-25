import type { KeeperChainClient } from "./types.js";

export async function getInputBalance(
  chain: KeeperChainClient,
  input: {
    userAddress: string;
    denom: string;
  }
) {
  return chain.getInputBalance(input);
}
