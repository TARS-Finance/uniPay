import 'dotenv/config';

import { homedir } from 'node:os';
import { dirname } from 'node:path';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';

import { execText, need, sleep } from './shared.js';
import {
  classifyOutputProposalQueryError,
  classifyFinalizeError,
  getDefaultFinalizerStatePath,
  isWithdrawalReadyForFinalize,
  parseDurationMs,
  toFinalizePayload,
  type ExecutorWithdrawal,
  type FinalizerState,
} from './finalize-withdrawals-lib.js';

type ExecutorStatus = {
  child?: {
    last_withdrawal_l2_sequence?: number;
  };
};

type OutputProposalResponse = {
  output_proposal?: {
    l1_block_time?: string;
  };
};

type BridgeResponse = {
  bridge_config?: {
    finalization_period?: string;
  };
};

const colors = {
  red: '\x1b[31m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  blue: '\x1b[34m',
  gray: '\x1b[90m',
  reset: '\x1b[0m',
} as const;

let shuttingDown = false;

function colorize(color: keyof typeof colors, message: string) {
  return `${colors[color]}${message}${colors.reset}`;
}

function info(message: string) {
  console.log(colorize('blue', `>> ${message}`));
}

function warn(message: string) {
  console.log(colorize('yellow', `!! ${message}`));
}

function success(message: string) {
  console.log(colorize('green', `ok ${message}`));
}

function errorLog(message: string) {
  console.error(colorize('red', `xx ${message}`));
}

async function fetchJson<T>(url: string) {
  const response = await fetch(url);

  if (!response.ok) {
    throw new Error(`request failed for ${url}: ${response.status} ${response.statusText}`);
  }

  return (await response.json()) as T;
}

function loadState(path: string, bridgeId: string, rollupChainId: string): FinalizerState {
  try {
    const parsed = JSON.parse(readFileSync(path, 'utf8')) as FinalizerState;

    if (parsed.bridgeId !== bridgeId || parsed.rollupChainId !== rollupChainId) {
      return {
        bridgeId,
        rollupChainId,
        nextSequence: 1,
        updatedAt: new Date().toISOString(),
      };
    }

    return parsed;
  } catch {
    return {
      bridgeId,
      rollupChainId,
      nextSequence: 1,
      updatedAt: new Date().toISOString(),
    };
  }
}

function saveState(path: string, state: FinalizerState) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, `${JSON.stringify(state, null, 2)}\n`);
}

function extractExecError(error: unknown) {
  if (typeof error === 'object' && error !== null) {
    const stderr = Reflect.get(error, 'stderr');

    if (typeof stderr === 'string' && stderr.trim().length > 0) {
      return stderr.trim();
    }
  }

  if (error instanceof Error) {
    return error.message;
  }

  return String(error);
}

function createWithdrawalTempPath(bridgeId: string, sequence: number) {
  return `/tmp/initia-rollup-withdrawal-${bridgeId}-${sequence}.json`;
}

function finalizeOnce({
  withdrawal,
  signer,
  keyringBackend,
  l1RpcUrl,
  l1ChainId,
  l1GasPrices,
}: {
  withdrawal: ExecutorWithdrawal;
  signer: string;
  keyringBackend: string;
  l1RpcUrl: string;
  l1ChainId: string;
  l1GasPrices: string;
}) {
  const tempPath = createWithdrawalTempPath(String(withdrawal.bridge_id), withdrawal.sequence);

  try {
    writeFileSync(tempPath, `${JSON.stringify(toFinalizePayload(withdrawal), null, 2)}\n`);

    const output = execText('initiad', [
      'tx',
      'ophost',
      'finalize-token-withdrawal',
      tempPath,
      '--from',
      signer,
      '--keyring-backend',
      keyringBackend,
      '--node',
      l1RpcUrl,
      '--chain-id',
      l1ChainId,
      '--gas',
      'auto',
      '--gas-adjustment',
      '1.6',
      '--gas-prices',
      l1GasPrices,
      '-y',
      '-o',
      'json',
    ]).trim();

    return {
      kind: 'success' as const,
      output,
    };
  } catch (error) {
    const errorText = extractExecError(error);

    return {
      kind: classifyFinalizeError(errorText),
      errorText,
    };
  } finally {
    rmSync(tempPath, { force: true });
  }
}

async function readFinalizationPeriodMs(bridgeId: string, l1RpcUrl: string) {
  const bridge = JSON.parse(
    execText('initiad', [
      'query',
      'ophost',
      'bridge',
      '--bridge-id',
      bridgeId,
      '--node',
      l1RpcUrl,
      '-o',
      'json',
    ]),
  ) as BridgeResponse;

  const finalizationPeriod = bridge.bridge_config?.finalization_period;

  if (!finalizationPeriod) {
    throw new Error(`missing finalization_period for bridge ${bridgeId}`);
  }

  return parseDurationMs(finalizationPeriod);
}

async function readOutputAvailableAtMs(bridgeId: string, outputIndex: number, l1RpcUrl: string, finalizationPeriodMs: number) {
  let output: OutputProposalResponse;

  try {
    output = JSON.parse(
      execText('initiad', [
        'query',
        'ophost',
        'output-proposal',
        '--bridge-id',
        bridgeId,
        '--output-index',
        String(outputIndex),
        '--node',
        l1RpcUrl,
        '-o',
        'json',
      ]),
    ) as OutputProposalResponse;
  } catch (error) {
    const errorText = extractExecError(error);
    const queryErrorKind = classifyOutputProposalQueryError(errorText);

    if (queryErrorKind === 'not-found' || queryErrorKind === 'transient') {
      return null;
    }

    throw error;
  }

  const l1BlockTime = output.output_proposal?.l1_block_time;

  if (!l1BlockTime) {
    throw new Error(`missing l1_block_time for output ${outputIndex}`);
  }

  return Date.parse(l1BlockTime) + finalizationPeriodMs;
}

async function main() {
  const bridgeId = need('BRIDGE_ID');
  const rollupChainId = need('ROLLUP_CHAIN_ID');
  const l1RpcUrl = need('L1_RPC_URL');
  const l1ChainId = need('L1_CHAIN_ID');
  const l1GasPrices = need('L1_GAS_PRICES');
  const executorApiUrl = process.env.EXECUTOR_API_URL ?? 'http://localhost:3000';
  const signer = process.env.WITHDRAWAL_FINALIZER_SIGNER ?? 'merchant';
  const keyringBackend = process.env.WITHDRAWAL_FINALIZER_KEYRING_BACKEND ?? 'test';
  const pollMs = Number(process.env.WITHDRAWAL_FINALIZER_POLL_MS ?? '5000');
  const statePath =
    process.env.WITHDRAWAL_FINALIZER_STATE_PATH ??
    getDefaultFinalizerStatePath({
      bridgeId,
      rollupChainId,
      homeDir: homedir(),
      xdgStateHome: process.env.XDG_STATE_HOME,
    });

  const finalizationPeriodMs = await readFinalizationPeriodMs(bridgeId, l1RpcUrl);
  const state = loadState(statePath, bridgeId, rollupChainId);

  info(`watching bridge ${bridgeId} on ${rollupChainId}`);
  info(`using executor API ${executorApiUrl}`);
  info(`state file ${statePath}`);
  info(`finalizer signer ${signer}`);

  for (;;) {
    if (shuttingDown) {
      return;
    }

    const status = await fetchJson<ExecutorStatus>(`${executorApiUrl}/status`);
    const lastWithdrawalSequence = status.child?.last_withdrawal_l2_sequence ?? 0;

    if (lastWithdrawalSequence < state.nextSequence) {
      await sleep(pollMs);
      continue;
    }

    for (let sequence = state.nextSequence; sequence <= lastWithdrawalSequence; sequence += 1) {
      const withdrawal = await fetchJson<ExecutorWithdrawal>(`${executorApiUrl}/withdrawal/${sequence}`);

      if (!isWithdrawalReadyForFinalize(withdrawal)) {
        warn(`sequence ${sequence} is known but proof data is not ready yet`);
        break;
      }

      const availableAtMs = await readOutputAvailableAtMs(
        bridgeId,
        withdrawal.output_index,
        l1RpcUrl,
        finalizationPeriodMs,
      );

      if (availableAtMs === null) {
        warn(`sequence ${sequence} output ${withdrawal.output_index} is not readable on L1 yet`);
        break;
      }

      if (Date.now() < availableAtMs) {
        warn(
          `sequence ${sequence} is waiting for finalization until ${new Date(availableAtMs).toISOString()}`,
        );
        break;
      }

      info(`finalizing withdrawal sequence ${sequence}`);
      const result = finalizeOnce({
        withdrawal,
        signer,
        keyringBackend,
        l1RpcUrl,
        l1ChainId,
        l1GasPrices,
      });

      if (result.kind === 'success') {
        success(`sequence ${sequence} finalized`);
        state.nextSequence = sequence + 1;
        state.updatedAt = new Date().toISOString();
        saveState(statePath, state);
        continue;
      }

      if (result.kind === 'already-finalized') {
        warn(`sequence ${sequence} was already finalized elsewhere`);
        state.nextSequence = sequence + 1;
        state.updatedAt = new Date().toISOString();
        saveState(statePath, state);
        continue;
      }

      if (result.kind === 'not-finalized') {
        warn(`sequence ${sequence} is not finalized on L1 yet`);
        break;
      }

      errorLog(`sequence ${sequence} failed: ${result.errorText}`);
      throw new Error(`failed to finalize sequence ${sequence}`);
    }

    await sleep(pollMs);
  }
}

function installSignalHandlers() {
  const handleShutdown = () => {
    if (shuttingDown) {
      return;
    }

    shuttingDown = true;
    warn('received interrupt, stopping withdrawal finalizer');
  };

  process.once('SIGINT', handleShutdown);
  process.once('SIGTERM', handleShutdown);
}

installSignalHandlers();
main().catch((error) => {
  if (shuttingDown) {
    process.exit(0);
  }

  errorLog(error instanceof Error ? error.message : String(error));
  process.exit(1);
});
