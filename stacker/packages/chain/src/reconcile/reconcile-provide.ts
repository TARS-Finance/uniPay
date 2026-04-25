import { getLpBalance } from "../query/get-lp-balance.js";
import type { KeeperChainClient } from "../query/types.js";

export type ReconcileProvideInput = {
  chain: KeeperChainClient;
  execution: {
    provideTxHash: string | null;
    lpAmount: string | null;
  };
  userAddress: string;
  lpDenom: string;
  lastKnownLpBalance: string;
};

export type ReconcileProvideResult =
  | {
      status: "ready-to-delegate";
      lpAmount: string;
    }
  | {
      status: "pending-confirmation";
    }
  | {
      status: "missing-liquidity";
    };

export async function reconcileProvide(
  input: ReconcileProvideInput
): Promise<ReconcileProvideResult> {
  if (input.execution.lpAmount) {
    return {
      status: "ready-to-delegate",
      lpAmount: input.execution.lpAmount
    };
  }

  if (!input.execution.provideTxHash) {
    return {
      status: "missing-liquidity"
    };
  }

  const confirmed = await input.chain.isTxConfirmed(input.execution.provideTxHash);

  if (!confirmed) {
    return {
      status: "pending-confirmation"
    };
  }

  const currentLpBalance = await getLpBalance(input.chain, {
    userAddress: input.userAddress,
    lpDenom: input.lpDenom
  });
  const previousLpBalance = BigInt(input.lastKnownLpBalance);
  const currentLp = BigInt(currentLpBalance);

  if (currentLp <= previousLpBalance) {
    return {
      status: "missing-liquidity"
    };
  }

  return {
    status: "ready-to-delegate",
    lpAmount: (currentLp - previousLpBalance).toString()
  };
}
