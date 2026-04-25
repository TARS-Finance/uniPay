import { startTransition, useState, useEffect, useCallback, useRef } from 'react';
import type { HistoryTx } from '../types';
import { QUOTE_API } from '../lib/config';
import { resolveTokenByAddress, subscribeIcons } from './useStrategies';

interface SingleSwap {
  swap_id?: string;
  initiate_tx_hash?: string | null;
  redeem_tx_hash?: string | null;
  refund_tx_hash?: string | null;
  chain?: string;
  amount?: string;
  secret_hash?: string;
  initiator?: string;
  redeemer?: string;
}

interface CreateOrder {
  create_id: string;
  created_at: string;
  source_chain: string;
  destination_chain: string;
  source_asset: string;
  destination_asset: string;
  source_amount: string;
  destination_amount: string;
  initiator_source_address: string;
  initiator_destination_address?: string;
  refund_address?: string;
  additional_data?: {
    strategy_id?: string;
    input_token_price?: number;
    output_token_price?: number;
  };
}

interface MatchedOrderVerbose {
  created_at: string;
  updated_at?: string;
  create_order: CreateOrder;
  source_swap: SingleSwap;
  destination_swap: SingleSwap;
}

interface BackendResponse {
  ok: boolean;
  data: {
    data: MatchedOrderVerbose[];
    page: number;
    per_page: number;
    total: number;
  };
}

function deriveStatus(src: SingleSwap, dst: SingleSwap): HistoryTx['status'] {
  if (dst.redeem_tx_hash) return 'Settled';
  if (src.refund_tx_hash || dst.refund_tx_hash) return 'Refunded';
  return 'Pending';
}

function detectToken(chain: string, asset: string): { sym: string; decimals: number } {
  const c = chain.toLowerCase();
  const a = asset.toLowerCase();

  if (c.includes('bitcoin')) return { sym: 'BTC', decimals: 8 };

  // Try to resolve by on-chain address (token contract OR HTLC) using the strategies cache
  if (a.startsWith('0x')) {
    const resolved = resolveTokenByAddress(a);
    if (resolved) return resolved;
  }

  // Parse "chain:symbol" format if the backend returns it
  if (a.includes(':')) {
    const sym = (a.split(':').pop() ?? '').toUpperCase();
    const knownDecimals: Record<string, number> = { BTC: 8, ETH: 18, USDC: 6, SOL: 9 };
    if (sym) return { sym, decimals: knownDecimals[sym] ?? 6 };
  }

  // Solana native currency — also catches raw HTLC program IDs on Solana
  if (c.includes('solana')) return { sym: 'SOL', decimals: 9 };

  return { sym: 'USDC', decimals: 6 };
}

function parseRaw(raw: string, decimals: number): number {
  return parseFloat(raw) / Math.pow(10, decimals);
}

// Parse strategy_id like "maker:source_chain:source_token->dest_chain:dest_token"
// Returns { srcSym, dstSym } or null if the format doesn't match.
function parseStrategyId(id?: string): { srcSym: string; dstSym: string } | null {
  if (!id) return null;
  // Drop the maker address prefix (everything up to the first ":")
  const rest = id.replace(/^[^:]+:/, '');
  const arrow = rest.indexOf('->');
  if (arrow === -1) return null;
  const srcPart = rest.slice(0, arrow); // e.g. "solana_devnet:sol"
  const dstPart = rest.slice(arrow + 2); // e.g. "utars_chain_1:usdc"
  const srcSym = srcPart.split(':').pop()?.toUpperCase() ?? '';
  const dstSym = dstPart.split(':').pop()?.toUpperCase() ?? '';
  if (!srcSym || !dstSym) return null;
  return { srcSym, dstSym };
}

// Bitcoin backend returns tx hashes as "txid:vout" — strip the output index so
// the hash is a clean 64-char id that the explorer can resolve.
function cleanHash(h: string | null | undefined): string | undefined {
  if (!h) return undefined;
  return h.split(':')[0];
}

const KNOWN_DECIMALS: Record<string, number> = { BTC: 8, ETH: 18, USDC: 6, SOL: 9 };

function toHistoryTx(order: MatchedOrderVerbose): HistoryTx {
  const co = order.create_order;

  let src = detectToken(co.source_chain, co.source_asset);
  let dst = detectToken(co.destination_chain, co.destination_asset);

  // strategy_id is the most reliable source of truth for token symbols
  const parsed = parseStrategyId(co.additional_data?.strategy_id);
  if (parsed) {
    src = { sym: parsed.srcSym, decimals: KNOWN_DECIMALS[parsed.srcSym] ?? src.decimals };
    dst = { sym: parsed.dstSym, decimals: KNOWN_DECIMALS[parsed.dstSym] ?? dst.decimals };
  }

  // chain key: use the raw chain name from the API directly (CHAINS in data.ts now covers these)
  const chainKey = co.source_chain;

  const ts = new Date(order.created_at || co.created_at).getTime();
  const updatedTs = order.updated_at ? new Date(order.updated_at).getTime() : undefined;
  const settleSecs = updatedTs ? Math.max(0, Math.round((updatedTs - ts) / 1000)) : undefined;

  const srcAmountHuman = parseRaw(co.source_amount, src.decimals);
  const dstAmountHuman = parseRaw(co.destination_amount, dst.decimals);
  const inPrice = co.additional_data?.input_token_price;
  const outPrice = co.additional_data?.output_token_price;

  return {
    id: co.create_id,
    amount: srcAmountHuman,
    token: src.sym,
    chain: chainKey,
    destChain: co.destination_chain,
    destToken: dst.sym,
    status: deriveStatus(order.source_swap, order.destination_swap),
    ts,
    initAmount: dstAmountHuman,
    srcHash: cleanHash(order.source_swap.initiate_tx_hash) ?? '',
    initHash: cleanHash(order.destination_swap.redeem_tx_hash) ?? null,
    swapId: order.source_swap.swap_id ?? '',

    orderId: co.create_id,
    srcInitiator: order.source_swap.initiator ?? co.initiator_source_address,
    srcRedeemer: order.source_swap.redeemer,
    srcInitiateHash: cleanHash(order.source_swap.initiate_tx_hash),
    srcRedeemHash: cleanHash(order.source_swap.redeem_tx_hash),
    srcRefundHash: cleanHash(order.source_swap.refund_tx_hash),
    dstInitiator: order.destination_swap.initiator,
    dstRedeemer: order.destination_swap.redeemer ?? co.initiator_destination_address,
    dstInitiateHash: cleanHash(order.destination_swap.initiate_tx_hash),
    dstRedeemHash: cleanHash(order.destination_swap.redeem_tx_hash),
    dstRefundHash: cleanHash(order.destination_swap.refund_tx_hash),
    refundAddr: co.refund_address ?? co.initiator_source_address,
    destinationAddr: co.initiator_destination_address,
    settleSecs,
    // USD notional = (raw_amount / 10^decimals) * unit_token_price
    srcPrice: inPrice !== undefined ? srcAmountHuman * inPrice : undefined,
    dstPrice: outPrice !== undefined ? dstAmountHuman * outPrice : undefined,
    srcUnitPrice: inPrice,
    dstUnitPrice: outPrice,
  };
}

export function useOrders(address: string | null) {
  const [orders, setOrders] = useState<HistoryTx[]>([]);
  const [loading, setLoading] = useState(false);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Keep the last raw payload so we can re-derive HistoryTx entries after the
  // strategies address→token map populates (otherwise the first fetch resolves
  // before strategies load and everything falls through to the native-token fallback).
  const lastRawRef = useRef<MatchedOrderVerbose[] | null>(null);
  const hydratedRef = useRef(false);

  const deriveFromRaw = useCallback((raw: MatchedOrderVerbose[]): HistoryTx[] => {
    const seen = new Set<string>();
    const deduped: HistoryTx[] = [];
    for (const item of raw) {
      const tx = toHistoryTx(item);
      if (seen.has(tx.id)) continue;
      seen.add(tx.id);
      deduped.push(tx);
    }
    return deduped;
  }, []);

  const fetch_ = useCallback(async () => {
    if (!address) {
      setOrders([]);
      lastRawRef.current = null;
      hydratedRef.current = false;
      setLoading(false);
      setRefreshing(false);
      return;
    }
    const shouldRefreshInPlace = hydratedRef.current && lastRawRef.current !== null;
    setError(null);
    if (shouldRefreshInPlace) setRefreshing(true);
    else setLoading(true);
    try {
      const url = `${QUOTE_API}/orders?address=${encodeURIComponent(address)}&page=1&per_page=50`;
      const res = await fetch(url);
      if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
      const json: BackendResponse = await res.json();
      lastRawRef.current = json.data.data;
      hydratedRef.current = true;
      startTransition(() => {
        setOrders(deriveFromRaw(json.data.data));
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, [address, deriveFromRaw]);

  // Re-derive whenever the strategies cache refreshes so tokens resolve correctly
  // even if `/orders` returned before `/strategies`.
  useEffect(() => {
    return subscribeIcons(() => {
      if (lastRawRef.current) {
        startTransition(() => {
          setOrders(deriveFromRaw(lastRawRef.current!));
        });
      }
    });
  }, [deriveFromRaw]);

  useEffect(() => {
    hydratedRef.current = false;
    lastRawRef.current = null;
    setOrders([]);
    setLoading(false);
    setRefreshing(false);
    setError(null);
  }, [address]);

  // Initial fetch
  useEffect(() => { fetch_(); }, [fetch_]);

  // Auto-poll every 3 seconds when an address is set
  useEffect(() => {
    if (!address) return;
    const id = setInterval(() => { fetch_(); }, 3_000);
    return () => clearInterval(id);
  }, [address, fetch_]);

  return { orders, loading, refreshing, error, refetch: fetch_ };
}
