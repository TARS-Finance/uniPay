/**
 * Mock stacker API server — mirrors the real /merchants/:address/* endpoints
 * with realistic data for the single USDC/INIT pool defined in .env
 *
 * Run:  node mock-server.mjs
 * Port: 3010  (matches API_PORT in .env)
 */

import http from 'node:http';

const PORT        = 3010;
const CHAIN_ID    = 'initiation-2';
const EXPLORER    = 'https://scan.initia.xyz';

// ── Mock data seeded from .env values ────────────────────────────────────────

const POOL_ID   = '0xdbf06c48af3984ec6d9ae8a9aa7dbb0bb1e784aa9b8c4a5681af660cf8558d7d';
const APY_BPS   = 2480;      // MERCHANT_DEMO_APY_BPS=2480  →  24.80% APY
const INPUT_DENOM = 'uusdc'; // MERCHANT_INPUT_DENOM=uusdc

// Amounts in micro-USDC (6 decimals)
const PRINCIPAL_AVAILABLE = '2480440000';   // 2 480.44 USDC
const PRINCIPAL_STAKED    = '18240920000';  // 18 240.92 USDC
const YIELD_EARNED        = '942170000';    //   942.17 USDC

const STRATEGY_ID = 'a1b2c3d4-0001-0001-0001-000000000001';

const MOCK_POOLS = [
  {
    id:              STRATEGY_ID,
    poolId:          POOL_ID,
    name:            'USDC / INIT',
    inputDenom:      INPUT_DENOM,
    tokens:          ['USDC', 'INIT'],
    staked:          PRINCIPAL_STAKED,
    available:       PRINCIPAL_AVAILABLE,
    apy_bps:         APY_BPS,
    earned:          YIELD_EARNED,
    status:          'active',
    lastExecutedAt:  new Date(Date.now() - 1000 * 60 * 8).toISOString(),
    executionCount:  47,
  },
];

// Generate mock activity (47 keeper runs, most successful)
function buildActivity() {
  const activity = [];
  const now = Date.now();
  const statuses = ['success','success','success','success','success','success','simulated','failed'];
  for (let i = 0; i < 47; i++) {
    const status = statuses[i % statuses.length];
    const staked = status === 'success' || status === 'simulated';
    const minsAgo = i * 10 + Math.floor(Math.random() * 5);
    const started = new Date(now - minsAgo * 60 * 1000).toISOString();
    const finished = new Date(now - minsAgo * 60 * 1000 + 4200).toISOString();
    const amount = String(1_000_000 + Math.floor(Math.random() * 9_000_000)); // 1–10 USDC
    const provideHash = staked ? `${i.toString(16).padStart(2,'0')}A${'0'.repeat(62)}`.slice(0,64).toUpperCase() : null;
    const delegateHash = staked ? `${i.toString(16).padStart(2,'0')}B${'0'.repeat(62)}`.slice(0,64).toUpperCase() : null;
    const primaryHash = delegateHash ?? provideHash;
    activity.push({
      id:              `exec-${String(i + 1).padStart(4, '0')}`,
      strategyId:      STRATEGY_ID,
      inputDenom:      INPUT_DENOM,
      amount,
      lpAmount:        staked ? String(Math.floor(Number(amount) * 0.97)) : '0',
      status,
      staked,
      provideTxHash:   provideHash,
      delegateTxHash:  delegateHash,
      txUrl:           primaryHash ? `${EXPLORER}/${CHAIN_ID}/txs/${primaryHash}` : null,
      errorMessage:    status === 'failed' ? 'slippage exceeded max_slippage_bps' : null,
      startedAt:       started,
      finishedAt:      finished,
    });
  }
  return activity;
}

const ACTIVITY = buildActivity();

// Build chart: last 30 days of cumulative staked totals
function buildChart() {
  const points = [];
  const now = Date.now();
  let cumulative = 0n;
  const dailyStep = BigInt('600000000'); // ~600 USDC / day
  for (let i = 29; i >= 0; i--) {
    const d = new Date(now - i * 86400 * 1000);
    const date = d.toISOString().slice(0, 10);
    cumulative += dailyStep + BigInt(Math.floor(Math.random() * 50_000_000));
    points.push({ date, cumulative_staked: cumulative.toString() });
  }
  return points;
}

const CHART_POINTS = buildChart();

// ── Response helpers ──────────────────────────────────────────────────────────

function json(res, statusCode, body) {
  const payload = JSON.stringify(body, null, 2);
  res.writeHead(statusCode, {
    'Content-Type': 'application/json',
    'Access-Control-Allow-Origin': '*',
    'Access-Control-Allow-Headers': 'Content-Type',
  });
  res.end(payload);
}

function notFound(res) {
  json(res, 404, { error: 'Merchant not found' });
}

// In-memory withdrawal store for the mock
const MOCK_WITHDRAWALS = [];
let withdrawalCounter = 1;

// Read JSON body helper
function readBody(req) {
  return new Promise((resolve) => {
    let data = '';
    req.on('data', chunk => { data += chunk; });
    req.on('end', () => {
      try { resolve(JSON.parse(data || '{}')); } catch { resolve({}); }
    });
  });
}

// ── Request router ────────────────────────────────────────────────────────────

http.createServer(async (req, res) => {
  if (req.method === 'OPTIONS') {
    res.writeHead(204, {
      'Access-Control-Allow-Origin': '*',
      'Access-Control-Allow-Headers': 'Content-Type',
      'Access-Control-Allow-Methods': 'GET,POST,PATCH,OPTIONS',
    });
    res.end();
    return;
  }

  const url = new URL(req.url, `http://localhost:${PORT}`);
  const path = url.pathname;

  console.log(`${req.method} ${path}`);

  // Match /merchants/:address/:endpoint[/:id]
  const m = path.match(/^\/merchants\/([^/]+)\/([^/]+)(?:\/([^/]+))?$/);
  if (!m) {
    json(res, 404, { error: 'Not found' });
    return;
  }

  const [, address, endpoint, subId] = m;
  if (!address) { notFound(res); return; }

  // ── GET endpoints ──────────────────────────────────────────────────────────
  if (req.method === 'GET') {
    switch (endpoint) {
      case 'balance':
        json(res, 200, {
          principal_available: PRINCIPAL_AVAILABLE,
          principal_staked:    PRINCIPAL_STAKED,
          yield_earned:        YIELD_EARNED,
          apy_bps:             APY_BPS,
        });
        break;

      case 'overview':
        json(res, 200, {
          principal_available: PRINCIPAL_AVAILABLE,
          principal_staked:    PRINCIPAL_STAKED,
          yield_earned:        YIELD_EARNED,
          apy_bps:             APY_BPS,
          pool_count:          1,
          total_executions:    ACTIVITY.filter(a => a.staked).length,
        });
        break;

      case 'pools':
        json(res, 200, { pools: MOCK_POOLS });
        break;

      case 'activity': {
        const limit = Number(url.searchParams.get('limit') ?? '50');
        json(res, 200, { activity: ACTIVITY.slice(0, limit) });
        break;
      }

      case 'chart':
        json(res, 200, { points: CHART_POINTS });
        break;

      case 'withdrawals':
        json(res, 200, { withdrawals: MOCK_WITHDRAWALS.filter(w => w._address === address).map(w => {
          const { _address, ...rest } = w;
          return rest;
        })});
        break;

      default:
        json(res, 404, { error: `Unknown endpoint: ${endpoint}` });
    }
    return;
  }

  // ── POST /merchants/:address/withdrawals ───────────────────────────────────
  if (req.method === 'POST' && endpoint === 'withdrawals') {
    const body = await readBody(req);
    const { strategyId, inputAmount } = body;

    if (!strategyId || !inputAmount) {
      json(res, 400, { error: 'strategyId and inputAmount required' });
      return;
    }

    const inputBig = BigInt(inputAmount);
    // Mock LP computation: roughly 1 LP per 0.73 USDC (based on pool math)
    const lpAmount = (inputBig * 137n) / 100n;
    const withdrawalId = `mock-withdrawal-${String(withdrawalCounter++).padStart(4, '0')}`;
    const now = new Date().toISOString();

    const record = {
      _address: address,
      id: withdrawalId,
      strategyId,
      inputAmount,
      lpAmount: lpAmount.toString(),
      status: 'pending',
      txHash: null,
      requestedAt: now,
      confirmedAt: null,
      txUrl: null,
    };
    MOCK_WITHDRAWALS.push(record);

    // Return mock MsgExecute — args are placeholder base64 strings
    json(res, 201, {
      withdrawalId,
      lpAmount: lpAmount.toString(),
      messages: [{
        typeUrl: '/initia.move.v1.MsgExecute',
        value: {
          sender: address,
          moduleAddress: 'init1s8uea4q27da35tqcq3p7cq08dz0ms9j6yv8ulynjnlfpvhulcfvserkvmq',
          moduleName: 'lock_staking',
          functionName: 'unlock_and_undelegate',
          typeArgs: [],
          args: [
            'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=', // placeholder
            'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
            'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
          ],
        },
      }],
      chainId: CHAIN_ID,
    });
    return;
  }

  // ── PATCH /merchants/:address/withdrawals/:id ──────────────────────────────
  if (req.method === 'PATCH' && endpoint === 'withdrawals' && subId) {
    const body = await readBody(req);
    const { txHash } = body;

    if (!txHash) {
      json(res, 400, { error: 'txHash required' });
      return;
    }

    const record = MOCK_WITHDRAWALS.find(w => w.id === subId && w._address === address);
    if (!record) {
      json(res, 404, { error: 'Withdrawal not found' });
      return;
    }

    record.status = 'confirmed';
    record.txHash = txHash;
    record.confirmedAt = new Date().toISOString();
    record.txUrl = `${EXPLORER}/${CHAIN_ID}/txs/${txHash}`;

    json(res, 200, { status: record.status, txHash: record.txHash });
    return;
  }

  json(res, 404, { error: 'Not found' });
}).listen(PORT, () => {
  console.log(`\nMock stacker running on http://localhost:${PORT}`);
  console.log('Endpoints:');
  console.log(`  GET  /merchants/:address/balance`);
  console.log(`  GET  /merchants/:address/overview`);
  console.log(`  GET  /merchants/:address/pools`);
  console.log(`  GET  /merchants/:address/activity`);
  console.log(`  GET  /merchants/:address/chart`);
  console.log(`  GET  /merchants/:address/withdrawals`);
  console.log(`  POST /merchants/:address/withdrawals`);
  console.log(`  PATCH /merchants/:address/withdrawals/:id`);
  console.log(`\nPool: USDC / INIT  |  APY ${APY_BPS / 100}%  |  Staked: ${Number(PRINCIPAL_STAKED) / 1e6} USDC\n`);
});
