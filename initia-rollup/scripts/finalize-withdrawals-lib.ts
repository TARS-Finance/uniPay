import { resolve } from 'node:path';

export type ExecutorWithdrawal = {
  sequence: number;
  from: string;
  to: string;
  amount: {
    denom: string;
    amount: string;
  };
  output_index: number;
  bridge_id: number;
  withdrawal_proofs: string[] | null;
  version: string;
  storage_root: string | null;
  last_block_hash: string | null;
  tx_time?: string;
  tx_height?: number;
  tx_hash?: string;
};

export type FinalizePayload = Omit<
  ExecutorWithdrawal,
  'tx_time' | 'tx_height' | 'tx_hash' | 'withdrawal_proofs' | 'storage_root' | 'last_block_hash'
> & {
  withdrawal_proofs: string[];
  storage_root: string;
  last_block_hash: string;
};

export type FinalizerState = {
  bridgeId: string;
  rollupChainId: string;
  nextSequence: number;
  updatedAt: string;
};

export type FinalizeErrorKind =
  | 'not-finalized'
  | 'already-finalized'
  | 'failed-to-verify'
  | 'unknown';

export type OutputProposalQueryKind = 'not-found' | 'transient' | 'unknown';

type ReadyExecutorWithdrawal = ExecutorWithdrawal & {
  withdrawal_proofs: string[];
  storage_root: string;
  last_block_hash: string;
};

export function toFinalizePayload(withdrawal: ExecutorWithdrawal): FinalizePayload {
  if (!isWithdrawalReadyForFinalize(withdrawal)) {
    throw new Error(`withdrawal sequence ${withdrawal.sequence} is not ready for finalization`);
  }

  const withdrawalProofs = withdrawal.withdrawal_proofs;
  const storageRoot = withdrawal.storage_root;
  const lastBlockHash = withdrawal.last_block_hash;

  return {
    sequence: withdrawal.sequence,
    from: withdrawal.from,
    to: withdrawal.to,
    amount: withdrawal.amount,
    output_index: withdrawal.output_index,
    bridge_id: withdrawal.bridge_id,
    withdrawal_proofs: withdrawalProofs,
    version: withdrawal.version,
    storage_root: storageRoot,
    last_block_hash: lastBlockHash,
  };
}

export function isWithdrawalReadyForFinalize(
  withdrawal: ExecutorWithdrawal,
): withdrawal is ReadyExecutorWithdrawal {
  return (
    withdrawal.output_index > 0 &&
    Array.isArray(withdrawal.withdrawal_proofs) &&
    withdrawal.withdrawal_proofs.length > 0 &&
    typeof withdrawal.storage_root === 'string' &&
    withdrawal.storage_root.length > 0 &&
    typeof withdrawal.last_block_hash === 'string' &&
    withdrawal.last_block_hash.length > 0
  );
}

export function getDefaultFinalizerStatePath({
  bridgeId,
  rollupChainId,
  homeDir,
  xdgStateHome,
}: {
  bridgeId: string;
  rollupChainId: string;
  homeDir: string;
  xdgStateHome?: string;
}) {
  const root = xdgStateHome ?? resolve(homeDir, '.local/state');
  return resolve(root, 'initia-rollup/finalizers', `${rollupChainId}-${bridgeId}.json`);
}

export function parseDurationMs(duration: string) {
  const matches = duration.matchAll(/(\d+)(ms|h|m|s)/g);
  let total = 0;
  let matched = false;

  for (const match of matches) {
    matched = true;
    const value = Number(match[1]);
    const unit = match[2];

    switch (unit) {
      case 'h':
        total += value * 60 * 60 * 1000;
        break;
      case 'm':
        total += value * 60 * 1000;
        break;
      case 's':
        total += value * 1000;
        break;
      case 'ms':
        total += value;
        break;
      default:
        throw new Error(`unsupported duration unit ${unit}`);
    }
  }

  if (!matched) {
    throw new Error(`could not parse duration ${duration}`);
  }

  return total;
}

export function classifyFinalizeError(errorText: string): FinalizeErrorKind {
  if (errorText.includes('output has not finalized')) {
    return 'not-finalized';
  }

  if (errorText.includes('withdrawal already finalized')) {
    return 'already-finalized';
  }

  if (errorText.includes('failed to verify withdrawal tx')) {
    return 'failed-to-verify';
  }

  return 'unknown';
}

export function classifyOutputProposalQueryError(errorText: string): OutputProposalQueryKind {
  if (errorText.includes('collections: not found')) {
    return 'not-found';
  }

  if (
    errorText.includes('502 Bad Gateway') ||
    errorText.includes('503 Service Unavailable') ||
    errorText.includes('504 Gateway Timeout') ||
    errorText.includes('invalid character') ||
    errorText.includes('context deadline exceeded') ||
    errorText.includes('connection refused') ||
    errorText.includes('EOF')
  ) {
    return 'transient';
  }

  return 'unknown';
}
