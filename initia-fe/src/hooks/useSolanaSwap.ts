import { useCallback, useEffect, useRef, useState } from 'react';

import { QUOTE_API } from '../lib/config';
import { createOrder, generateSecret, sha256Hex, toHex, revealSecret } from '../lib/htlc';
import { normalizeToEvmAddress } from '../lib/bech32';
import {
  connectPhantom,
  getPhantom,
  getSolanaExecutorAddress,
  solanaInitiateHTLC,
} from '../lib/solana';
import type { QuoteMode } from '../types';

const STORAGE_KEY = 'sol.pending_order';

export type SolanaSwapStep =
  | 'idle'
  | 'connecting'
  | 'creating'
  | 'locking'         // submitting initiate() from Phantom
  | 'user_initiated'  // watcher saw source HTLC
  | 'cobi_initiated'  // executor locked destination on Initia
  | 'user_redeemed'   // destination redeemed by merchant/solver
  | 'fulfilled'       // solana-executor redeemed source with preimage
  | 'refunded'
  | 'error';

export interface SolanaSwapState {
  step: SolanaSwapStep;
  orderId: string | null;
  sourceTxHash: string | null;
  userAddress: string | null;
  error: string | null;
}

export interface SolanaSwapActions {
  startSolanaSwap: (params: {
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    sourceAssetId: string;
    destinationAssetId: string; // e.g. "utars_chain_1:usdc" — must match the quote registry
    merchantInitiaAddress: string;
    strategyId?: string;
  }) => Promise<void>;
  reset: () => void;
}

interface SwapLike {
  initiate_tx_hash?: string | null;
  redeem_tx_hash?: string | null;
  refund_tx_hash?: string | null;
}

function extractErrorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object') {
    const obj = e as Record<string, unknown>;
    const shortMessage = obj.shortMessage;
    if (typeof shortMessage === 'string') return shortMessage;
    const details = obj.details;
    if (typeof details === 'string') return details;
    const message = obj.message;
    if (typeof message === 'string') return message;
    const reason = obj.reason;
    if (typeof reason === 'string') return reason;
    try {
      return JSON.stringify(e);
    } catch {
      // fall through
    }
  }
  return String(e);
}

function stepFromOrder(s: { source_swap?: SwapLike; destination_swap?: SwapLike }): SolanaSwapStep {
  const src = s.source_swap ?? {};
  const dst = s.destination_swap ?? {};
  if (src.refund_tx_hash || dst.refund_tx_hash) return 'refunded';
  if (src.redeem_tx_hash) return 'fulfilled';
  if (dst.redeem_tx_hash) return 'user_redeemed';
  if (dst.initiate_tx_hash) return 'cobi_initiated';
  if (src.initiate_tx_hash) return 'user_initiated';
  return 'user_initiated';
}

export function useSolanaSwap(): SolanaSwapState & SolanaSwapActions {
  const [step, setStep] = useState<SolanaSwapStep>('idle');
  const [orderId, setOrderId] = useState<string | null>(null);
  const [sourceTxHash, setSourceTxHash] = useState<string | null>(null);
  const [userAddress, setUserAddress] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const abortRef = useRef(false);

  const startPolling = useCallback((oid: string, secretHash: string) => {
    if (pollRef.current) clearInterval(pollRef.current);
    let polls = 0;
    let revealed = false;
    let revealInFlight = false;
    pollRef.current = setInterval(async () => {
      if (abortRef.current) { clearInterval(pollRef.current!); return; }
      if (++polls > 360) {
        clearInterval(pollRef.current!);
        setError('Timed out waiting for settlement');
        setStep('error');
        localStorage.removeItem(STORAGE_KEY);
        return;
      }
      try {
        const r = await fetch(`${QUOTE_API}/orders/${oid}`);
        if (!r.ok) return;
        const sj = await r.json();
        const s = sj.data ?? sj;
        const derived = stepFromOrder(s);
        setStep(derived);

        // Once destination is locked, reveal the secret to the Initia executor
        // (redeems the destination USDC HTLC), then immediately forward the same
        // secret to the Solana executor so it can redeem the source SOL HTLC.
        if (derived === 'cobi_initiated' && !revealed && !revealInFlight) {
          const secretHex = localStorage.getItem(`sol.secret.${secretHash}`);
          if (secretHex) {
            revealInFlight = true;
            revealSecret(oid, secretHex)
              .then(() => { revealed = true; })
              .catch((e) => { console.warn('revealSecret (Solana) failed, will retry', e); revealed = false; })
              .finally(() => { revealInFlight = false; });
          }
        }

        if (derived === 'fulfilled' || derived === 'refunded') {
          clearInterval(pollRef.current!);
          localStorage.removeItem(STORAGE_KEY);
          localStorage.removeItem(`sol.secret.${secretHash}`);
        }
      } catch {
        // transient; retry
      }
    }, 5000);
  }, []);

  useEffect(() => {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (!saved) return;
    try {
      const { oid, secretHash, sig, addr } = JSON.parse(saved);
      if (!oid) { localStorage.removeItem(STORAGE_KEY); return; }
      setOrderId(oid);
      setSourceTxHash(sig ?? null);
      setUserAddress(addr ?? null);
      setStep('user_initiated');
      startPolling(oid, secretHash);
    } catch {
      localStorage.removeItem(STORAGE_KEY);
    }
  }, [startPolling]);

  const reset = useCallback(() => {
    abortRef.current = true;
    if (pollRef.current) clearInterval(pollRef.current);
    setStep('idle');
    setOrderId(null);
    setSourceTxHash(null);
    setError(null);
    localStorage.removeItem(STORAGE_KEY);
    setTimeout(() => { abortRef.current = false; }, 100);
  }, []);

  const startSolanaSwap = useCallback(async ({
    quoteMode,
    sourceAmountRaw,
    destinationAmountRaw,
    sourceAssetId,
    destinationAssetId,
    merchantInitiaAddress,
    strategyId,
  }: {
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    sourceAssetId: string;
    destinationAssetId: string;
    merchantInitiaAddress: string;
    strategyId?: string;
  }) => {
    abortRef.current = false;
    setError(null);

    try {
      setStep('connecting');
      let initiatorBase58: string;
      const existing = getPhantom()?.publicKey?.toBase58();
      initiatorBase58 = existing ?? (await connectPhantom());
      setUserAddress(initiatorBase58);

      const redeemerBase58 = await getSolanaExecutorAddress();

      const secret = generateSecret();
      const secretHex = toHex(secret);
      const secretHashHex = await sha256Hex(secret);
      const secretHash = secretHashHex.replace(/^0x/, '');
      localStorage.setItem(`sol.secret.${secretHash}`, secretHex);

      setStep('creating');
      const matched = await createOrder({
        from: sourceAssetId,
        to: destinationAssetId,
        from_amount: quoteMode === 'exact-in' ? sourceAmountRaw : undefined,
        to_amount: quoteMode === 'exact-out' ? destinationAmountRaw : undefined,
        initiator_source_address: initiatorBase58,
        initiator_destination_address: normalizeToEvmAddress(merchantInitiaAddress),
        secret_hash: secretHash,
        strategy_id: strategyId,
      });
      const oid: string = matched.create_order?.create_id;
      const expiresInSlots: number = Number(matched.source_swap?.timelock ?? 216000);
      const amountLamports = matched.source_swap?.amount ?? sourceAmountRaw;
      if (!oid) throw new Error('No order id returned');

      setOrderId(oid);
      setStep('locking');

      const sig = await solanaInitiateHTLC({
        initiator: initiatorBase58,
        redeemer: redeemerBase58,
        amountLamports,
        expiresInSlots,
        secretHashHex: secretHash,
      });
      setSourceTxHash(sig);

      localStorage.setItem(STORAGE_KEY, JSON.stringify({
        oid, secretHash, sig, addr: initiatorBase58,
      }));

      setStep('user_initiated');
      startPolling(oid, secretHash);
    } catch (e: unknown) {
      if (abortRef.current) return;
      setError(extractErrorMessage(e));
      setStep('error');
    }
  }, [startPolling]);

  return { step, orderId, sourceTxHash, userAddress, error, startSolanaSwap, reset };
}
