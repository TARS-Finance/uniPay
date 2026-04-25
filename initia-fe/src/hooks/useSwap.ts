import { useState, useCallback, useRef, useEffect } from 'react';
import {
  generateSecret,
  toHex,
  sha256Hex,
  storeSecret,
  loadSecret,
  clearSecret,
  initiateERC20HTLC,
  waitForReceipt,
  createOrder,
  revealSecret,
} from '../lib/htlc';
import { QUOTE_API } from '../lib/config';
import type { Hex } from 'viem';
import { useWallet } from '../lib/wallet-context';
import type { SourceChainConfig } from '../lib/config';
import { normalizeToEvmAddress } from '../lib/bech32';
import type { QuoteMode } from '../types';

const STORAGE_KEY = 'evm.pending_order';
const SECRET_KEY_PREFIX = 'htlc.secret.';

export type SwapStep =
  | 'idle'
  | 'loading'
  | 'connecting'      // step 0: switching chain
  | 'locking'         // step 1: approve (if needed) + source HTLC tx
  | 'user_initiated'  // step 2: source confirmed, executor initiating destination
  | 'cobi_initiated'  // step 3: destination locked, revealing secret
  | 'user_redeemed'   // step 4: destination redeemed
  | 'fulfilled'       // step 5: source redeemed by executor — fully done
  | 'refunded'
  | 'done'
  | 'error';

export interface SwapState {
  step: SwapStep;
  stepIndex: number;
  error: string | null;
  sourceTxHash: string | null;
  destinationTxHash: string | null;
  redeemTxHash: string | null;
  orderId: string | null;
  userAddress: string | null;
}

export interface SwapActions {
  startSwap: (params: {
    chainConfig: SourceChainConfig;
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    receiverAddress: string;
    from: string;
    to: string;
    strategyId?: string;
  }) => Promise<void>;
  reset: () => void;
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
    try { return JSON.stringify(e); } catch { /* fall through */ }
  }
  return String(e);
}

const STEP_INDEX: Record<SwapStep, number> = {
  idle:           0,
  loading:        0,
  connecting:     0,
  locking:        1,
  user_initiated: 2,
  cobi_initiated: 3,
  user_redeemed:  4,
  fulfilled:      4,
  refunded:       0,
  done:           4,
  error:          0,
};

interface SwapLike {
  initiate_tx_hash?: string | null;
  redeem_tx_hash?: string | null;
  refund_tx_hash?: string | null;
}

function stepFromOrder(s: { source_swap?: SwapLike; destination_swap?: SwapLike }): SwapStep {
  const src = s.source_swap ?? {};
  const dst = s.destination_swap ?? {};
  if (src.refund_tx_hash || dst.refund_tx_hash) return 'refunded';
  if (src.redeem_tx_hash) return 'fulfilled';
  if (dst.redeem_tx_hash) return 'user_redeemed';
  if (dst.initiate_tx_hash) return 'cobi_initiated';
  if (src.initiate_tx_hash) return 'user_initiated';
  return 'user_initiated';
}

function listStoredSecretOrderIds(): string[] {
  try {
    return Object.keys(localStorage)
      .filter((key) => key.startsWith(SECRET_KEY_PREFIX))
      .map((key) => key.slice(SECRET_KEY_PREFIX.length))
      .filter(Boolean);
  } catch {
    return [];
  }
}

export function useSwap(): SwapState & SwapActions {
  const { address: connectedAddress, provider: walletProvider, switchToChain } = useWallet();

  const [step, setStep] = useState<SwapStep>('idle');
  const [error, setError] = useState<string | null>(null);
  const [sourceTxHash, setSourceTxHash] = useState<string | null>(null);
  const [destinationTxHash, setDestinationTxHash] = useState<string | null>(null);
  const [redeemTxHash, setRedeemTxHash] = useState<string | null>(null);
  const [orderId, setOrderId] = useState<string | null>(null);
  const [userAddress, setUserAddress] = useState<string | null>(null);
  const abortRef = useRef(false);
  const pollRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const tryReveal = useCallback(async (oid: string): Promise<string | null> => {
    const secretHex = loadSecret(oid);
    if (!secretHex) return null;
    const txHash = await revealSecret(oid, secretHex);
    setRedeemTxHash(txHash);
    // Secret cleared only when src.redeem_tx_hash confirms on-chain (fulfilled state in poll loop)
    return txHash;
  }, []);

  const startPolling = useCallback((oid: string) => {
    if (pollRef.current) clearInterval(pollRef.current);
    let polls = 0;
    let secretRevealed = false;
    let secretRevealInFlight = false;
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

        if (s.destination_swap?.initiate_tx_hash) {
          setDestinationTxHash(s.destination_swap.initiate_tx_hash);
        }

        if (derived === 'cobi_initiated' && !secretRevealed && !secretRevealInFlight) {
          secretRevealInFlight = true;
          tryReveal(oid)
            .then((txHash) => {
              if (txHash) secretRevealed = true;
            })
            .catch((e) => { console.warn('revealSecret (EVM) failed, will retry', e); })
            .finally(() => { secretRevealInFlight = false; });
        }

        if (derived === 'fulfilled' || derived === 'refunded') {
          clearInterval(pollRef.current!);
          localStorage.removeItem(STORAGE_KEY);
          clearSecret(oid);
          if (derived === 'fulfilled') setStep('done');
        }
      } catch {
        // transient; retry
      }
    }, 10000);
  }, [tryReveal]);

  const resumeOrder = useCallback(async (oid: string, saved?: { sourceTx?: string | null; addr?: string | null }) => {
    setOrderId(oid);
    setSourceTxHash(saved?.sourceTx ?? null);
    setUserAddress(saved?.addr ?? null);
    setStep('loading');

    try {
      const sj = await fetch(`${QUOTE_API}/orders/${oid}`).then((r) => r.ok ? r.json() : null);
      if (!sj) {
        setStep('user_initiated');
        startPolling(oid);
        return;
      }
      const s = sj.data ?? sj;
      const derived = stepFromOrder(s);
      const sourceTx = s.source_swap?.initiate_tx_hash ?? saved?.sourceTx ?? null;
      const addr = s.source_swap?.initiator ?? s.create_order?.initiator_source_address ?? saved?.addr ?? null;

      if (sourceTx) setSourceTxHash(sourceTx);
      if (addr) setUserAddress(addr);
      setStep(derived);
      if (s.destination_swap?.initiate_tx_hash) {
        setDestinationTxHash(s.destination_swap.initiate_tx_hash);
      }

      localStorage.setItem(STORAGE_KEY, JSON.stringify({ oid, sourceTx, addr }));

      if (derived === 'cobi_initiated') {
        try {
          await tryReveal(oid);
        } catch (e) {
          console.warn('revealSecret (EVM resume) failed, will retry', e);
        }
      }

      if (derived === 'fulfilled' || derived === 'refunded') {
        localStorage.removeItem(STORAGE_KEY);
        clearSecret(oid);
        if (derived === 'fulfilled') setStep('done');
        return;
      }

      startPolling(oid);
    } catch {
      setStep('user_initiated');
      startPolling(oid);
    }
  }, [startPolling, tryReveal]);

  // On mount: resume any in-progress EVM order from localStorage
  useEffect(() => {
    const recover = async () => {
      const saved = localStorage.getItem(STORAGE_KEY);
      if (saved) {
        try {
          const { oid, sourceTx, addr } = JSON.parse(saved);
          if (!oid) {
            localStorage.removeItem(STORAGE_KEY);
          } else {
            await resumeOrder(oid, { sourceTx, addr });
            return;
          }
        } catch {
          localStorage.removeItem(STORAGE_KEY);
        }
      }

      const candidates = listStoredSecretOrderIds();
      for (const oid of candidates) {
        try {
          const sj = await fetch(`${QUOTE_API}/orders/${oid}`).then((r) => r.ok ? r.json() : null);
          if (!sj) continue;
          const s = sj.data ?? sj;
          const derived = stepFromOrder(s);
          if (derived === 'fulfilled' || derived === 'refunded') {
            clearSecret(oid);
            continue;
          }
          await resumeOrder(oid, {
            sourceTx: s.source_swap?.initiate_tx_hash ?? null,
            addr: s.source_swap?.initiator ?? s.create_order?.initiator_source_address ?? null,
          });
          return;
        } catch {
          // ignore bad orphan candidates and continue scanning
        }
      }
    };

    recover();
  }, [resumeOrder]);

  const reset = useCallback(() => {
    abortRef.current = true;
    if (pollRef.current) clearInterval(pollRef.current);
    setStep('idle');
    setError(null);
    setSourceTxHash(null);
    setDestinationTxHash(null);
    setRedeemTxHash(null);
    setOrderId(null);
    setUserAddress(null);
    localStorage.removeItem(STORAGE_KEY);
    setTimeout(() => { abortRef.current = false; }, 100);
  }, []);

  const startSwap = useCallback(async ({
    chainConfig: chainCfg,
    quoteMode,
    sourceAmountRaw,
    destinationAmountRaw,
    receiverAddress,
    from,
    to,
    strategyId,
  }: {
    chainConfig: SourceChainConfig;
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    receiverAddress: string;
    from: string;
    to: string;
    strategyId?: string;
  }) => {
    abortRef.current = false;
    setError(null);

    if (!connectedAddress || !walletProvider) {
      setError('Wallet not connected. Please connect your wallet first.');
      setStep('error');
      return;
    }

    try {
      // Step 0: switch to source chain
      setStep('connecting');
      await switchToChain(chainCfg.chainIdHex, {
        chainName: chainCfg.chainName,
        nativeCurrency: chainCfg.nativeCurrency,
        rpcUrls: chainCfg.rpcUrls,
      });
      const addr = connectedAddress;
      setUserAddress(addr);

      if (abortRef.current) return;

      const secretBytes = generateSecret();
      const secretHex = toHex(secretBytes);
      const secretHash = await sha256Hex(secretBytes);
      const secretHashNoPrefix = secretHash.startsWith('0x') ? secretHash.slice(2) : secretHash;
      const destinationAddress = normalizeToEvmAddress(receiverAddress || addr);

      // Step 1: create order via quote service (deterministic create_id from secret_hash)
      const matched = await createOrder({
        from,
        to,
        from_amount: quoteMode === 'exact-in' ? sourceAmountRaw : undefined,
        to_amount: quoteMode === 'exact-out' ? destinationAmountRaw : undefined,
        initiator_source_address: addr,
        initiator_destination_address: destinationAddress,
        secret_hash: secretHashNoPrefix,
        strategy_id: strategyId,
      });

      const oid = matched.create_order.create_id;
      setOrderId(oid);
      storeSecret(oid, secretHex);

      const src = matched.source_swap;
      if (abortRef.current) return;

      // Step 2: initiate HTLC using quote-derived parameters
      setStep('locking');
      const txHash = await initiateERC20HTLC({
        from: addr,
        htlcAddress: src.htlc_address,
        tokenAddress: src.token_address,
        redeemer: src.redeemer,
        timelock: BigInt(src.timelock),
        amount: BigInt(src.amount),
        secretHash: secretHash as Hex,
        chainIdHex: chainCfg.chainIdHex,
        provider: walletProvider,
      });
      setSourceTxHash(txHash);

      await waitForReceipt(txHash, walletProvider);

      if (abortRef.current) return;

      localStorage.setItem(STORAGE_KEY, JSON.stringify({ oid, sourceTx: txHash, addr }));

      setStep('user_initiated');
      startPolling(oid);
    } catch (e: unknown) {
      if (abortRef.current) return;
      const msg = extractErrorMessage(e);
      setError(msg);
      setStep('error');
    }
  }, [connectedAddress, walletProvider, switchToChain, startPolling]);

  return {
    step,
    stepIndex: STEP_INDEX[step],
    error,
    sourceTxHash,
    destinationTxHash,
    redeemTxHash,
    orderId,
    userAddress,
    startSwap,
    reset,
  };
}
