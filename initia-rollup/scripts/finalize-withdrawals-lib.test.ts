import test from 'node:test';
import assert from 'node:assert/strict';

import {
  classifyOutputProposalQueryError,
  classifyFinalizeError,
  getDefaultFinalizerStatePath,
  isWithdrawalReadyForFinalize,
  parseDurationMs,
  toFinalizePayload,
  type ExecutorWithdrawal,
} from './finalize-withdrawals-lib.js';

test('toFinalizePayload strips executor-only metadata fields', () => {
  const withdrawal: ExecutorWithdrawal = {
    sequence: 1,
    from: 'init1from',
    to: 'init1to',
    amount: {
      denom: 'uusdc',
      amount: '500000',
    },
    output_index: 24,
    bridge_id: 1912,
    withdrawal_proofs: ['proof-a'],
    version: 'AQ==',
    storage_root: 'storage-root',
    last_block_hash: 'last-block-hash',
    tx_time: '2026-04-25T16:42:57.83064+05:30',
    tx_height: 73,
    tx_hash: '8B3A53E9876715B32C7539C6BEA7A819B3BA7CF0094A7735EC6F7B5D0166C37C',
  };

  assert.deepEqual(toFinalizePayload(withdrawal), {
    sequence: 1,
    from: 'init1from',
    to: 'init1to',
    amount: {
      denom: 'uusdc',
      amount: '500000',
    },
    output_index: 24,
    bridge_id: 1912,
    withdrawal_proofs: ['proof-a'],
    version: 'AQ==',
    storage_root: 'storage-root',
    last_block_hash: 'last-block-hash',
  });
});

test('getDefaultFinalizerStatePath scopes state by chain and bridge', () => {
  assert.equal(
    getDefaultFinalizerStatePath({
      bridgeId: '1912',
      rollupChainId: 'utars-chain-1',
      homeDir: '/Users/tester',
    }),
    '/Users/tester/.local/state/initia-rollup/finalizers/utars-chain-1-1912.json',
  );

  assert.equal(
    getDefaultFinalizerStatePath({
      bridgeId: '1912',
      rollupChainId: 'utars-chain-1',
      homeDir: '/Users/tester',
      xdgStateHome: '/tmp/xdg-state',
    }),
    '/tmp/xdg-state/initia-rollup/finalizers/utars-chain-1-1912.json',
  );
});

test('parseDurationMs handles hour minute and second components', () => {
  assert.equal(parseDurationMs('5m0s'), 300_000);
  assert.equal(parseDurationMs('1h2m3s'), 3_723_000);
  assert.equal(parseDurationMs('45s'), 45_000);
});

test('classifyFinalizeError recognizes retryable and terminal finalize responses', () => {
  assert.equal(
    classifyFinalizeError('rpc error: code = Unknown desc = output has not finalized'),
    'not-finalized',
  );
  assert.equal(
    classifyFinalizeError('rpc error: code = Unknown desc = withdrawal already finalized'),
    'already-finalized',
  );
  assert.equal(
    classifyFinalizeError('rpc error: code = Unknown desc = failed to verify withdrawal tx: invalid output root'),
    'failed-to-verify',
  );
  assert.equal(classifyFinalizeError('some unrelated error'), 'unknown');
});

test('classifyOutputProposalQueryError recognizes output-proposal race errors', () => {
  assert.equal(
    classifyOutputProposalQueryError(
      'rpc error: code = InvalidArgument desc = collections: not found: key \'("1912", "73")\' of type github.com/cosmos/gogoproto/opinit.ophost.v1.Output: invalid request',
    ),
    'not-found',
  );

  assert.equal(
    classifyOutputProposalQueryError(
      'Error: error in json rpc client, with http response metadata: (Status: 502 Bad Gateway, Protocol HTTP/1.1). error unmarshalling: invalid character \'e\' looking for beginning of value',
    ),
    'transient',
  );

  assert.equal(classifyOutputProposalQueryError('some unrelated error'), 'unknown');
});

test('isWithdrawalReadyForFinalize rejects placeholder withdrawal records', () => {
  const pendingWithdrawal: ExecutorWithdrawal = {
    sequence: 2,
    from: 'init1from',
    to: 'init1to',
    amount: {
      denom: 'uusdc',
      amount: '500000',
    },
    output_index: 0,
    bridge_id: 1912,
    withdrawal_proofs: null,
    version: 'AQ==',
    storage_root: null,
    last_block_hash: null,
    tx_time: '2026-04-25T17:22:26.276316+05:30',
    tx_height: 116,
    tx_hash: '62F1550A13C4246C062C62CB837D34A12520312CEC97F5DD3D970830A450BCB2',
  };

  assert.equal(isWithdrawalReadyForFinalize(pendingWithdrawal), false);
  assert.throws(() => toFinalizePayload(pendingWithdrawal), /not ready for finalization/);
});
