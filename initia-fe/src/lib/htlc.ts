import { encodeFunctionData, parseAbi, type Hex } from 'viem';
import type { EIP1193Provider } from './wallet-context';
import { EXECUTOR_API, QUOTE_API } from './config';

const ERC20_HTLC_ABI = parseAbi([
  'function initiate(address redeemer, uint256 timelock, uint256 amount, bytes32 secretHash)',
]);

const ERC20_ABI = parseAbi([
  'function approve(address spender, uint256 amount) returns (bool)',
  'function allowance(address owner, address spender) view returns (uint256)',
  'function balanceOf(address account) view returns (uint256)',
]);

// ── Secret helpers ────────────────────────────────────────────────────────────

export function generateSecret(): Uint8Array {
  const secret = new Uint8Array(32);
  crypto.getRandomValues(secret);
  return secret;
}

export function toHex(bytes: Uint8Array): Hex {
  return ('0x' + Array.from(bytes).map((b) => b.toString(16).padStart(2, '0')).join('')) as Hex;
}

export async function sha256Hex(bytes: Uint8Array): Promise<Hex> {
  const buf = await crypto.subtle.digest('SHA-256', bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer);
  return ('0x' + Array.from(new Uint8Array(buf)).map((b) => b.toString(16).padStart(2, '0')).join('')) as Hex;
}

// ── LocalStorage ─────────────────────────────────────────────────────────────

const SECRET_KEY = (orderId: string) => `htlc.secret.${orderId}`;

export function storeSecret(orderId: string, secretHex: Hex) {
  localStorage.setItem(SECRET_KEY(orderId), secretHex);
}

export function loadSecret(orderId: string): Hex | null {
  return (localStorage.getItem(SECRET_KEY(orderId)) as Hex | null);
}

export function clearSecret(orderId: string) {
  localStorage.removeItem(SECRET_KEY(orderId));
}

// ── Wallet interaction ────────────────────────────────────────────────────────

// EIP-1559 fee suggestion with a safety buffer on baseFee so the tx doesn't
// get rejected with `max fee per gas less than block base fee` when the base
// fee ticks up between estimation and submission.
async function suggestFees(provider: EIP1193Provider): Promise<{ maxFeePerGas: string; maxPriorityFeePerGas: string } | Record<string, never>> {
  try {
    const block = await provider.request({
      method: 'eth_getBlockByNumber',
      params: ['latest', false],
    }) as { baseFeePerGas?: string } | null;
    const baseFee = block?.baseFeePerGas ? BigInt(block.baseFeePerGas) : 0n;
    if (baseFee === 0n) return {};

    let priority = 1_000_000n; // 0.001 gwei default
    try {
      const pHex = await provider.request({ method: 'eth_maxPriorityFeePerGas' }) as string;
      priority = BigInt(pHex);
    } catch { /* fall back to default priority */ }

    // 2x buffer on base fee is standard practice to survive a few blocks of increase.
    const maxFee = baseFee * 2n + priority;
    return {
      maxFeePerGas: '0x' + maxFee.toString(16),
      maxPriorityFeePerGas: '0x' + priority.toString(16),
    };
  } catch {
    return {};
  }
}

// Approves MaxUint256 so only the very first swap ever needs an approval prompt.
// Subsequent swaps skip this entirely (allowance check passes).
async function ensureAllowance(params: {
  from: string;
  tokenAddress: string;
  spender: string;
  amount: bigint;
  chainIdHex: string;
  provider: EIP1193Provider;
}): Promise<void> {
  const allowanceData = encodeFunctionData({
    abi: ERC20_ABI,
    functionName: 'allowance',
    args: [params.from as Hex, params.spender as Hex],
  });
  const raw = await params.provider.request({
    method: 'eth_call',
    params: [{ to: params.tokenAddress, data: allowanceData }, 'latest'],
  }) as string;
  if (BigInt(raw) >= params.amount) return;

  const approveData = encodeFunctionData({
    abi: ERC20_ABI,
    functionName: 'approve',
    args: [params.spender as Hex, (1n << 256n) - 1n],
  });
  const txHash = await params.provider.request({
    method: 'eth_sendTransaction',
    params: [{ from: params.from, to: params.tokenAddress, data: approveData, chainId: params.chainIdHex }],
  }) as string;
  await waitForReceipt(txHash, params.provider);
}

export async function initiateERC20HTLC(params: {
  from: string;
  htlcAddress: string;
  tokenAddress: string;
  redeemer: string;
  timelock: bigint;
  amount: bigint;
  secretHash: Hex;
  chainIdHex: string;
  provider: EIP1193Provider;
}): Promise<string> {
  await ensureAllowance({
    from: params.from,
    tokenAddress: params.tokenAddress,
    spender: params.htlcAddress,
    amount: params.amount,
    chainIdHex: params.chainIdHex,
    provider: params.provider,
  });

  const data = encodeFunctionData({
    abi: ERC20_HTLC_ABI,
    functionName: 'initiate',
    args: [params.redeemer as Hex, params.timelock, params.amount, params.secretHash],
  });

  const fees = await suggestFees(params.provider);
  const txReq = { from: params.from, to: params.htlcAddress, data, chainId: params.chainIdHex, ...fees };

  // Dry-run: simulate via eth_call first so a revert surfaces before the wallet popup.
  try {
    const callResult = await params.provider.request({
      method: 'eth_call',
      params: [txReq, 'latest'],
    });
    console.log('[htlc.initiate] eth_call dry-run OK:', callResult);
  } catch (e) {
    console.error('[htlc.initiate] eth_call dry-run reverted:', e);
    const msg = e instanceof Error ? e.message : String(e);
    throw new Error(`HTLC initiate would revert — aborted before wallet popup. ${msg}`);
  }

  // Estimate gas so we can detect absurd gas quotes (which usually indicate a revert path).
  try {
    const gasHex = await params.provider.request({
      method: 'eth_estimateGas',
      params: [txReq],
    }) as string;
    console.log('[htlc.initiate] eth_estimateGas:', gasHex, '=', BigInt(gasHex).toString(), 'gas');
  } catch (e) {
    console.error('[htlc.initiate] eth_estimateGas failed:', e);
    const msg = e instanceof Error ? e.message : String(e);
    throw new Error(`HTLC initiate gas estimation failed — aborted before wallet popup. ${msg}`);
  }

  return await params.provider.request({
    method: 'eth_sendTransaction',
    params: [txReq],
  }) as string;
}

export async function waitForReceipt(
  txHash: string,
  provider: EIP1193Provider,
): Promise<{ blockNumber: number }> {
  for (let i = 0; i < 120; i++) {
    await sleep(3000);
    const receipt = await provider.request({
      method: 'eth_getTransactionReceipt',
      params: [txHash],
    }) as { blockNumber?: string; status?: string } | null;
    if (receipt?.blockNumber) {
      if (receipt.status === '0x0') throw new Error('Source HTLC tx reverted');
      return { blockNumber: parseInt(receipt.blockNumber, 16) };
    }
  }
  throw new Error('Timed out waiting for source tx receipt');
}

// ── Quote service API calls ───────────────────────────────────────────────────

export interface QuoteSwap {
  swap_id: string;
  chain: string;
  asset: string;
  htlc_address: string;
  token_address: string;
  initiator: string;
  redeemer: string;
  timelock: number;
  amount: string;
  filled_amount: string;
  secret_hash: string;
  secret: string;
  initiate_tx_hash: string;
  redeem_tx_hash: string;
  refund_tx_hash: string;
}

export interface QuoteMatchedOrder {
  source_swap: QuoteSwap;
  destination_swap: QuoteSwap;
  create_order: { create_id: string; source_chain: string; destination_chain: string };
}

export async function createOrder(params: {
  from: string;
  to: string;
  from_amount?: string;
  to_amount?: string;
  initiator_source_address: string;
  initiator_destination_address: string;
  secret_hash: string;
  strategy_id?: string;
  slippage?: number;
}): Promise<QuoteMatchedOrder> {
  const res = await fetch(`${QUOTE_API}/orders`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(params),
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok) {
    throw new Error(json?.error || `${res.status} ${res.statusText}`);
  }
  return (json.data ?? json) as QuoteMatchedOrder;
}

export async function pollOrderStatus(orderId: string): Promise<QuoteMatchedOrder> {
  const res = await fetch(`${QUOTE_API}/orders/${orderId}`);
  const json = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(json?.error || 'Order not found');
  return (json.data ?? json) as QuoteMatchedOrder;
}

// ── Executor API calls ────────────────────────────────────────────────────────

export async function revealSecret(orderId: string, secretHex: string): Promise<string> {
  const res = await fetch(`${EXECUTOR_API}/secret`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ order_id: orderId, secret: secretHex }),
  });
  if (!res.ok) {
    const err = await res.json().catch(() => ({ error: res.statusText }));
    throw new Error(err.error || 'Failed to reveal secret');
  }
  const { tx_hash } = await res.json();
  return tx_hash;
}

// ── Utils ─────────────────────────────────────────────────────────────────────

function sleep(ms: number) {
  return new Promise<void>((r) => setTimeout(r, ms));
}

export function genOrderId(): string {
  const ts = Date.now().toString(36);
  const rand = Math.random().toString(36).slice(2, 6);
  return `fe-${ts}-${rand}`;
}
