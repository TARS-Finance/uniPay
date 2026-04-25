import { getDelegatedLpBalance } from "../query/get-delegated-lp-balance.js";
import { getInputBalance } from "../query/get-input-balance.js";
import { getLpBalance } from "../query/get-lp-balance.js";
import type { KeeperChainClient } from "../query/types.js";

export async function reconcileDelegate(
  chain: KeeperChainClient,
  input: {
    userAddress: string;
    inputDenom: string;
    lpDenom: string;
    validatorAddress: string;
  }
) {
  const [lastInputBalance, lastLpBalance, lastDelegatedLpBalance] =
    await Promise.all([
      getInputBalance(chain, {
        userAddress: input.userAddress,
        denom: input.inputDenom
      }),
      getLpBalance(chain, {
        userAddress: input.userAddress,
        lpDenom: input.lpDenom
      }),
      getDelegatedLpBalance(chain, {
        userAddress: input.userAddress,
        validatorAddress: input.validatorAddress,
        lpDenom: input.lpDenom
      })
    ]);

  return {
    lastInputBalance,
    lastLpBalance,
    lastDelegatedLpBalance
  };
}
