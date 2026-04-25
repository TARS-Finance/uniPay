import { useState, useEffect } from "react";
import { DEST_CHAINS, QUOTE_API } from "../lib/config";

export interface Strategy {
  id: string;
  sourceChain: string; // "base_sepolia"
  destChain: string; // e.g. "utars_chain_1" — sourced live from /strategies
  sourceTokenId: string; // "usd-coin"
  destTokenId: string; // "usd-coin"
  sourceAsset: string; // /chains asset id, e.g. "base_sepolia:usdc" — used as quote from/to
  destAsset: string; // /chains asset id, e.g. "utars_chain_1:usdc" — used as quote from/to
  sourceDisplaySymbol?: string; // "USDC"
  destDisplaySymbol?: string; // "USDC"
  sourceDecimals: number;
  destDecimals: number;
  minAmount: string; // raw integer
  maxAmount: string;
  fee: number; // bips
}

export interface SourceOption {
  chain: string;
  tokenId: string;
  displayToken: string;
  displayChain: string;
  assetDisplayName: string; // "{chain} {TOKEN_SYMBOL}" derived from /chains
  icon?: string; // token icon URL from /chains
}

export interface StrategiesResult {
  strategies: Strategy[];
  loading: boolean;
  error: string | null;
  sourceOptions: SourceOption[];
  getDestOptions: (
    sourceChain: string,
    sourceTokenId: string,
  ) => Array<{
    chain: string;
    tokenId: string;
    displayToken: string;
    displayChain: string;
  }>;
  getStrategy: (
    sourceChain: string,
    sourceTokenId: string,
    destChain: string,
    destTokenId: string,
  ) => Strategy | undefined;
}

function assetSymbol(asset: string, displaySymbol?: string, fallback?: string): string {
  if (displaySymbol) return displaySymbol.toUpperCase();
  const part = asset.split(":").pop() ?? asset;
  // HTLC addresses are hex strings — never show them; prefer fallback symbol.
  if (/^0x[0-9a-f]{10,}/i.test(part) || /^[0-9a-f]{40,}$/i.test(part)) {
    return (fallback ?? "").toUpperCase() || part;
  }
  return part.toUpperCase();
}

function chainDisplayName(chain: string): string {
  const s = chain.replace(/_/g, " ").toLowerCase();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

interface RawAsset {
  asset: string;
  htlc_address?: string;
  token_address?: string;
  token_id: string;
  display_symbol?: string;
  decimals: number;
  version: string;
}

interface RawStrategy {
  id: string;
  source_chain: string;
  dest_chain: string;
  source_asset: RawAsset;
  dest_asset: RawAsset;
  min_amount: string;
  max_amount: string;
  fee: number;
  makers: string[];
}

// Module-level cache with 60s stale time
let cache: { strategies: Strategy[]; fetchedAt: number } | null = null;
// Maps lower-cased token contract address → {sym, decimals} for token detection in other hooks
let addressMap: Map<string, { sym: string; decimals: number }> = new Map();
// Maps "{chain}:{tokenId}" → { displayName, icon } built from /chains response
interface ChainAssetInfo { id: string; displayName: string; icon?: string; symbol: string }
let chainAssetNameMap: Map<string, ChainAssetInfo> = new Map();
// chainId (e.g. "bitcoin_testnet") → icon URL
let chainIconMap: Map<string, string> = new Map();
// upper-cased symbol (e.g. "BTC") → icon URL, first-seen wins
let tokenIconMap: Map<string, string> = new Map();

const iconSubscribers = new Set<() => void>();
function notifyIcons() { iconSubscribers.forEach((cb) => cb()); }
export function subscribeIcons(cb: () => void): () => void {
  iconSubscribers.add(cb);
  return () => iconSubscribers.delete(cb);
}
export function getChainIcon(chain: string): string | undefined {
  return chainIconMap.get(chain);
}
export function getTokenIcon(sym: string): string | undefined {
  return tokenIconMap.get(sym.toUpperCase());
}
const STALE_MS = 60_000;

interface RawChainAsset {
  id: string; // "base_sepolia:usdc"
  name: string;
  chain: string;
  icon?: string;
  token_ids?: { coingecko?: string; aggregate?: string; cmc?: string | null };
  [key: string]: unknown;
}

interface RawChain {
  chain: string;
  id: string;
  icon?: string;
  assets?: RawChainAsset[];
  [key: string]: unknown;
}

async function fetchChainAssetNames(): Promise<Map<string, ChainAssetInfo>> {
  const m = new Map<string, ChainAssetInfo>();
  try {
    const r = await fetch(`${QUOTE_API}/chains`);
    const json: { ok: boolean; data: RawChain[] } = await r.json();
    for (const chainData of json.data ?? []) {
      const chainName = chainData.chain;
      if (chainData.icon) chainIconMap.set(chainName, chainData.icon);
      for (const asset of chainData.assets ?? []) {
        // Prefer the symbol encoded in `name` (e.g. "Initia:USDC") — backend
        // `token_ids` occasionally disagrees with the actual asset.
        const nameSymbol = asset.name?.split(":").pop()?.trim();
        const idSymbol = asset.id.split(":")[1]?.toUpperCase();
        const tokenSymbol = (nameSymbol || asset.token_ids?.aggregate || idSymbol || "").toUpperCase();
        const tokenKey = asset.token_ids?.coingecko ?? asset.id.split(":")[1] ?? "";
        // Asset icon may be blank; fall back to the chain icon so the logo isn't empty.
        const icon = asset.icon || chainData.icon;
        m.set(`${chainName}:${tokenKey}`, {
          id: asset.id,
          displayName: `${chainDisplayName(chainName)} ${tokenSymbol}`,
          icon,
          symbol: tokenSymbol,
        });
        // Prefer real per-token icons over chain fallbacks: overwrite if the
        // previous entry was just the chain icon.
        if (tokenSymbol) {
          const existing = tokenIconMap.get(tokenSymbol);
          if (asset.icon) tokenIconMap.set(tokenSymbol, asset.icon);
          else if (icon && !existing) tokenIconMap.set(tokenSymbol, icon);
        }
      }
    }
  } catch {
    // non-fatal: display names will fall back to defaults
  }
  return m;
}

/** Resolve a contract address to token info using data fetched from the backend /strategies endpoint. */
export function resolveTokenByAddress(
  address: string,
): { sym: string; decimals: number } | null {
  return addressMap.get(address.toLowerCase()) ?? null;
}

function mapRaw(raw: RawStrategy, nameMap: Map<string, ChainAssetInfo>): Strategy {
  const sourceId = nameMap.get(`${raw.source_chain}:${raw.source_asset.token_id}`)?.id
    ?? `${raw.source_chain}:${raw.source_asset.token_id}`;
  const destId = nameMap.get(`${raw.dest_chain}:${raw.dest_asset.token_id}`)?.id
    ?? `${raw.dest_chain}:${raw.dest_asset.token_id}`;
  return {
    id: raw.id,
    sourceChain: raw.source_chain,
    destChain: raw.dest_chain,
    sourceTokenId: raw.source_asset.token_id,
    destTokenId: raw.dest_asset.token_id,
    sourceAsset: sourceId,
    destAsset: destId,
    sourceDisplaySymbol: raw.source_asset.display_symbol,
    destDisplaySymbol: raw.dest_asset.display_symbol,
    sourceDecimals: raw.source_asset.decimals,
    destDecimals: raw.dest_asset.decimals,
    minAmount: raw.min_amount,
    maxAmount: raw.max_amount,
    fee: raw.fee,
  };
}

function nativeSymbolForChain(chain: string): string | null {
  const c = chain.toLowerCase();
  if (c.includes('bitcoin')) return 'BTC';
  if (c === 'solana' || c.startsWith('solana_')) return 'SOL';
  if (c === 'ethereum' || c.startsWith('arbitrum') || c.startsWith('base') || c.startsWith('optimism')) return 'ETH';
  return null;
}

function buildAddressMap(
  rawMap: Record<string, RawStrategy>,
): Map<string, { sym: string; decimals: number }> {
  const m = new Map<string, { sym: string; decimals: number }>();
  for (const raw of Object.values(rawMap)) {
    for (const [asset, chain] of [
      [raw.source_asset, raw.source_chain],
      [raw.dest_asset, raw.dest_chain],
    ] as const) {
      const nativeSym = asset.token_address === 'primary' ? nativeSymbolForChain(chain) : null;
      const info = {
        sym: nativeSym ?? assetSymbol(asset.asset, asset.display_symbol),
        decimals: asset.decimals,
      };
      for (const key of [asset.token_address, asset.htlc_address, asset.asset]) {
        if (key && key.startsWith('0x')) m.set(key.toLowerCase(), info);
      }
    }
  }
  return m;
}

export function useStrategies(): StrategiesResult {
  const [strategies, setStrategies] = useState<Strategy[]>(
    cache?.strategies ?? [],
  );
  const [loading, setLoading] = useState<boolean>(
    !cache || Date.now() - cache.fetchedAt > STALE_MS,
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (cache && Date.now() - cache.fetchedAt <= STALE_MS) {
      setStrategies(cache.strategies);
      setLoading(false);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);

    Promise.all([
      fetch(`${QUOTE_API}/strategies`).then((r) => r.json()),
      fetchChainAssetNames(),
    ])
      .then(
        ([json, nameMap]: [
          { ok: boolean; data: Record<string, RawStrategy> },
          Map<string, ChainAssetInfo>,
        ]) => {
          if (cancelled) return;
          if (!json.ok)
            throw new Error("strategies endpoint returned ok=false");
          const allMapped = Object.values(json.data).map((r) => mapRaw(r, nameMap));
          const destChains = DEST_CHAINS;
          const mapped = destChains
            ? allMapped.filter((s) => destChains.includes(s.destChain))
            : allMapped;
          addressMap = buildAddressMap(json.data);
          chainAssetNameMap = nameMap;
          cache = { strategies: mapped, fetchedAt: Date.now() };
          setStrategies(mapped);
          setLoading(false);
          notifyIcons();
        },
      )
      .catch((err: unknown) => {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : String(err));
        setLoading(false);
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const sourceOptions = (() => {
    const seen = new Set<string>();
    const opts: SourceOption[] = [];
    for (const s of strategies) {
      const key = `${s.sourceChain}:${s.sourceTokenId}`;
      if (!seen.has(key)) {
        seen.add(key);
        const info = chainAssetNameMap.get(`${s.sourceChain}:${s.sourceTokenId}`);
        const displayToken = assetSymbol(s.sourceAsset, s.sourceDisplaySymbol, info?.symbol);
        const displayChain = chainDisplayName(s.sourceChain);
        const assetDisplayName = info?.displayName ?? `${displayChain} ${displayToken}`;
        opts.push({
          chain: s.sourceChain,
          tokenId: s.sourceTokenId,
          displayToken,
          displayChain,
          assetDisplayName,
          icon: info?.icon,
        });
      }
    }
    return opts;
  })();

  const getDestOptions = (sourceChain: string, sourceTokenId: string) => {
    const seen = new Set<string>();
    const opts: Array<{
      chain: string;
      tokenId: string;
      displayToken: string;
      displayChain: string;
    }> = [];
    for (const s of strategies) {
      if (s.sourceChain !== sourceChain || s.sourceTokenId !== sourceTokenId)
        continue;
      const key = `${s.destChain}:${s.destTokenId}`;
      if (!seen.has(key)) {
        seen.add(key);
        const destInfo = chainAssetNameMap.get(`${s.destChain}:${s.destTokenId}`);
        opts.push({
          chain: s.destChain,
          tokenId: s.destTokenId,
          displayToken: assetSymbol(s.destAsset, s.destDisplaySymbol, destInfo?.symbol),
          displayChain: chainDisplayName(s.destChain),
        });
      }
    }
    return opts;
  };

  const getStrategy = (
    sourceChain: string,
    sourceTokenId: string,
    destChain: string,
    destTokenId: string,
  ) =>
    strategies.find(
      (s) =>
        s.sourceChain === sourceChain &&
        s.sourceTokenId === sourceTokenId &&
        s.destChain === destChain &&
        s.destTokenId === destTokenId,
    );

  return {
    strategies,
    loading,
    error,
    sourceOptions,
    getDestOptions,
    getStrategy,
  };
}
