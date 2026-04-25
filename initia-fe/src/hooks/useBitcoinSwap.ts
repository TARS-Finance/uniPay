import { useState, useCallback, useRef, useEffect } from 'react';
import { bech32, bech32m, base58 } from '@scure/base';
import { createOrder, generateSecret, toHex, sha256Hex, revealSecret } from '../lib/htlc';
import { QUOTE_API } from '../lib/config';
import { normalizeToEvmAddress } from '../lib/bech32';
import type { QuoteMode } from '../types';

// Validates a Bitcoin address (mainnet or testnet, any type: P2PKH, P2SH, P2WPKH, P2WSH, P2TR).
// For P2TR (SegWit v1) returns the 32-byte x-only pubkey hex (so the orderbook can build the
// HTLC refund leaf directly). For other types returns the address itself — the orderbook is
// responsible for handling the non-pubkey case.
function validateBitcoinRefundAddress(address: string): string {
  const trimmed = address.trim();
  if (!trimmed) throw new Error('Bitcoin refund address is required');

  // Try bech32 / bech32m (segwit). bech32 = v0 (P2WPKH/P2WSH), bech32m = v1+ (P2TR).
  const lower = trimmed.toLowerCase();
  const hrp = lower.startsWith('bc1') ? 'bc' : lower.startsWith('tb1') ? 'tb' : lower.startsWith('bcrt1') ? 'bcrt' : null;
  if (hrp) {
    const tryDecode = (codec: typeof bech32 | typeof bech32m) => {
      try { return codec.decode(lower as `${string}1${string}`, 90); } catch { return null; }
    };
    const m = tryDecode(bech32m);
    if (m && m.words[0] === 1) {
      const program = bech32m.fromWords(m.words.slice(1));
      if (program.length !== 32) throw new Error('Taproot witness program must be 32 bytes');
      return Array.from(program).map((b) => b.toString(16).padStart(2, '0')).join('');
    }
    const v0 = tryDecode(bech32);
    if (v0 && v0.words[0] === 0) {
      const program = bech32.fromWords(v0.words.slice(1));
      if (program.length !== 20 && program.length !== 32) {
        throw new Error('Invalid SegWit v0 program length');
      }
      return trimmed;
    }
    throw new Error('Invalid Bitcoin bech32 address');
  }

  // Legacy base58 (P2PKH / P2SH).
  try {
    const decoded = base58.decode(trimmed);
    if (decoded.length !== 25) throw new Error();
    return trimmed;
  } catch {
    throw new Error('Invalid Bitcoin address');
  }
}

const STORAGE_KEY = 'btc.pending_order';

export type BitcoinSwapStep =
  | 'idle'
  | 'loading'
  | 'creating'
  | 'sending'           // UniSat popup open — waiting for user to confirm send
  | 'awaiting'         // order created, no on-chain progress yet
  | 'user_initiated'   // user's BTC deposit detected (source initiate tx)
  | 'cobi_initiated'   // executor locked destination asset
  | 'user_redeemed'    // user received destination funds (destination redeem)
  | 'fulfilled'        // executor claimed source BTC with secret (source redeem) — fully done
  | 'refunded'         // either leg was refunded
  | 'done'             // alias kept for legacy callers — equivalent to fulfilled
  | 'error';

export interface BitcoinSwapState {
  step: BitcoinSwapStep;
  depositAddress: string | null;
  sourceAmountSats: string | null;
  orderId: string | null;
  error: string | null;
  sourceTxHash: string | null;
  destinationTxHash: string | null;
  redeemTxHash: string | null;
}

export interface BitcoinSwapActions {
  startBitcoinSwap: (params: {
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    destinationAsset: string;
    receiverAddress: string;
    sourceAsset?: string;
    btcRefundAddress?: string;
    strategyId?: string;
    // UniSat integration — if provided, wallet sends BTC instead of QR
    unisatSendBitcoin?: (to: string, satoshis: number) => Promise<string>;
  }) => Promise<void>;
  reset: () => void;
}

interface SwapLike {
  initiate_tx_hash?: string | null;
  redeem_tx_hash?: string | null;
  refund_tx_hash?: string | null;
}

function stepFromOrder(s: { source_swap?: SwapLike; destination_swap?: SwapLike }): BitcoinSwapStep {
  const src = s.source_swap ?? {};
  const dst = s.destination_swap ?? {};

  if (src.refund_tx_hash || dst.refund_tx_hash) return 'refunded';
  if (src.redeem_tx_hash) return 'fulfilled';
  if (dst.redeem_tx_hash) return 'user_redeemed';
  if (dst.initiate_tx_hash) return 'cobi_initiated';
  if (src.initiate_tx_hash) return 'user_initiated';
  return 'awaiting';
}

export function useBitcoinSwap(): BitcoinSwapState & BitcoinSwapActions {
  const [step, setStep] = useState<BitcoinSwapStep>('idle');
  const [depositAddress, setDepositAddress] = useState<string | null>(null);
  const [sourceAmountSats, setSourceAmountSats] = useState<string | null>(null);
  const [orderId, setOrderId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sourceTxHash, setSourceTxHash] = useState<string | null>(null);
  const [destinationTxHash, setDestinationTxHash] = useState<string | null>(null);
  const [redeemTxHash, setRedeemTxHash] = useState<string | null>(null);
  const abortRef = useRef(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const startPolling = useCallback((oid: string, secretHash: string) => {
    if (pollRef.current) clearInterval(pollRef.current);
    let polls = 0;
    let secretRevealInFlight = false;
    let secretRevealed = false;
    pollRef.current = setInterval(async () => {
      if (abortRef.current) { clearInterval(pollRef.current!); return; }
      if (++polls > 360) {
        clearInterval(pollRef.current!);
        setError('Timed out waiting for Bitcoin deposit');
        setStep('error');
        localStorage.removeItem(STORAGE_KEY);
        localStorage.removeItem(`btc.secret.${secretHash}`);
        return;
      }
      try {
        const sr = await fetch(`${QUOTE_API}/orders/${oid}`);
        if (!sr.ok) return;
        const sj = await sr.json();
        const s = sj.data ?? sj;
        const derived = stepFromOrder(s);
        setStep(derived);
        setSourceTxHash(s.source_swap?.initiate_tx_hash || null);
        setDestinationTxHash(s.destination_swap?.initiate_tx_hash || null);
        setRedeemTxHash(s.destination_swap?.redeem_tx_hash || null);

        // Once destination is locked, reveal the secret to the executor so it can
        // redeem the destination on behalf of the user (which also unlocks source).
        if (
          derived === 'cobi_initiated' &&
          !secretRevealed &&
          !secretRevealInFlight
        ) {
          const secretHex = localStorage.getItem(`btc.secret.${secretHash}`);
          if (secretHex) {
            secretRevealInFlight = true;
            revealSecret(oid, secretHex)
              .then(() => { secretRevealed = true; })
              .catch((e) => { console.warn('revealSecret failed, will retry', e); })
              .finally(() => { secretRevealInFlight = false; });
          }
        }

        if (derived === 'fulfilled' || derived === 'refunded') {
          clearInterval(pollRef.current!);
          localStorage.removeItem(STORAGE_KEY);
          localStorage.removeItem(`btc.secret.${secretHash}`);
        }
      } catch {
        // ignore transient poll errors
      }
    }, 5000);
  }, []);

  // On mount: resume any in-progress order from localStorage
  useEffect(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (!saved) return;
    try {
      const { oid, addr, secretHash, sourceAmountSats: savedSourceAmountSats } = JSON.parse(saved);
      if (!oid || !addr) { localStorage.removeItem(STORAGE_KEY); return; }

      setOrderId(oid);
      setDepositAddress(addr);
      setSourceAmountSats(savedSourceAmountSats ?? null);
      setStep('loading');

      // Immediately derive current step from API, then keep polling
      fetch(`${QUOTE_API}/orders/${oid}`)
        .then(r => r.ok ? r.json() : null)
        .then(sj => {
          if (!sj) { setStep('awaiting'); startPolling(oid, secretHash); return; }
          const s = sj.data ?? sj;
          const derived = stepFromOrder(s);
          setStep(derived);
          setSourceTxHash(s.source_swap?.initiate_tx_hash || null);
          setDestinationTxHash(s.destination_swap?.initiate_tx_hash || null);
          setRedeemTxHash(s.destination_swap?.redeem_tx_hash || null);
          if (derived === 'fulfilled' || derived === 'refunded') {
            localStorage.removeItem(STORAGE_KEY);
            localStorage.removeItem(`btc.secret.${secretHash}`);
          } else {
            startPolling(oid, secretHash);
          }
        })
        .catch(() => { setStep('awaiting'); startPolling(oid, secretHash); });
    } catch {
      localStorage.removeItem(STORAGE_KEY);
    }
  }, [startPolling]);

  const reset = useCallback(() => {
    abortRef.current = true;
    if (pollRef.current) clearInterval(pollRef.current);
    try {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved) {
        const { secretHash } = JSON.parse(saved);
        if (secretHash) localStorage.removeItem(`btc.secret.${secretHash}`);
      }
    } catch { /* ignore */ }
    setStep('idle');
    setDepositAddress(null);
    setSourceAmountSats(null);
    setOrderId(null);
    setError(null);
    setSourceTxHash(null);
    setDestinationTxHash(null);
    setRedeemTxHash(null);
    localStorage.removeItem(STORAGE_KEY);
    setTimeout(() => { abortRef.current = false; }, 100);
  }, []);

  const startBitcoinSwap = useCallback(async ({
    quoteMode,
    sourceAmountRaw,
    destinationAmountRaw,
    destinationAsset,
    receiverAddress,
    sourceAsset = 'bitcoin_testnet:btc',
    btcRefundAddress,
    strategyId,
    unisatSendBitcoin,
  }: {
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    destinationAsset: string;
    receiverAddress: string;
    sourceAsset?: string;
    btcRefundAddress?: string;
    strategyId?: string;
    unisatSendBitcoin?: (to: string, satoshis: number) => Promise<string>;
  }) => {
    abortRef.current = false;
    setError(null);
    setStep('creating');

    let pendingSecretHash: string | null = null;

    try {
      const sourceIsBitcoin = sourceAsset.startsWith('bitcoin');
      let refundXOnlyPubkey: string;
      try {
        const validated = validateBitcoinRefundAddress(btcRefundAddress ?? '');
        // When the source chain is Bitcoin, the orderbook expects a 32-byte x-only pubkey
        // (used in the HTLC refund leaf). The user's refund destination address is tracked
        // separately by the executor — here we just need any legit x-only pubkey to satisfy
        // the script construction. Use the BIP-341 NUMS point.
        const NUMS_X_ONLY_PUBKEY = '50929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803acb';
        refundXOnlyPubkey = sourceIsBitcoin ? NUMS_X_ONLY_PUBKEY : validated;
      } catch (e) {
        setError(e instanceof Error ? e.message : 'Invalid Bitcoin refund address');
        setStep('error');
        return;
      }

      const secretBytes = generateSecret();
      const secretHex = toHex(secretBytes);
      const secretHashHex = await sha256Hex(secretBytes);
      const secretHash = secretHashHex.replace(/^0x/, '');
      pendingSecretHash = secretHash;

      localStorage.setItem(`btc.secret.${secretHash}`, secretHex);

      const matched = await createOrder({
        from: sourceAsset,
        to: destinationAsset,
        from_amount: quoteMode === 'exact-in' ? sourceAmountRaw : undefined,
        to_amount: quoteMode === 'exact-out' ? destinationAmountRaw : undefined,
        initiator_source_address: refundXOnlyPubkey,
        initiator_destination_address: normalizeToEvmAddress(receiverAddress),
        secret_hash: secretHash,
        strategy_id: strategyId,
      });
      const oid: string = matched.create_order?.create_id;
      const addr: string = matched.source_swap?.swap_id;
      const createdSourceAmount = matched.source_swap?.amount || sourceAmountRaw;

      if (!oid) throw new Error('No order id returned');
      if (!addr) throw new Error('No deposit address returned from order');

      localStorage.setItem(STORAGE_KEY, JSON.stringify({
        oid,
        addr,
        secretHash,
        sourceAmountSats: createdSourceAmount,
      }));

      setOrderId(oid);
      setDepositAddress(addr);
      setSourceAmountSats(createdSourceAmount);

      // If UniSat is connected, send BTC directly from the wallet
      if (unisatSendBitcoin) {
        setStep('sending');
        try {
          await unisatSendBitcoin(addr, Number(createdSourceAmount));
        } catch (e: unknown) {
          if (abortRef.current) return;
          setError(e instanceof Error ? e.message : 'UniSat send failed');
          setStep('error');
          localStorage.removeItem(STORAGE_KEY);
          return;
        }
      }

      setStep('awaiting');
      startPolling(oid, secretHash);
    } catch (e: unknown) {
      if (pendingSecretHash) localStorage.removeItem(`btc.secret.${pendingSecretHash}`);
      if (abortRef.current) return;
      setError(e instanceof Error ? e.message : String(e));
      setStep('error');
    }
  }, [startPolling]);

  return {
    step,
    depositAddress,
    sourceAmountSats,
    orderId,
    error,
    sourceTxHash,
    destinationTxHash,
    redeemTxHash,
    startBitcoinSwap,
    reset,
  };
}
