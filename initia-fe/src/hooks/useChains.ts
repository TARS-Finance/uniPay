import { useState, useEffect } from 'react';
import { QUOTE_API, type SourceChainConfig } from '../lib/config';

// Types matching the Rust RawChain / RawAsset shape from GET /chains
interface RawContractInfo {
  address: string;
  schema?: string;
}

interface RawAsset {
  id: string;
  chain: string;
  htlc?: RawContractInfo;
  token?: RawContractInfo;
  decimals: number;
  solver: string;
  chain_id?: string;
  chain_type: string;
}

interface RawChain {
  chain: string;
  supported_htlc_schemas: string[];
  source_timelock: string;
  assets: RawAsset[];
}

// MetaMask needs nativeCurrency + rpcUrls to add unknown chains.
// Arbitrum Sepolia and Base Sepolia are already known to MetaMask so these
// only matter as a fallback for truly novel chains.
const CHAIN_METAMASK_DEFAULTS: Record<string, { nativeCurrency: { name: string; symbol: string; decimals: number }; rpcUrls: string[] }> = {
  '421614': { nativeCurrency: { name: 'Ethereum', symbol: 'ETH', decimals: 18 }, rpcUrls: ['https://sepolia-rollup.arbitrum.io/rpc'] },
  '84532':  { nativeCurrency: { name: 'Ethereum', symbol: 'ETH', decimals: 18 }, rpcUrls: ['https://sepolia.base.org'] },
};

function toChainName(chain: string): string {
  return chain.replace(/_/g, ' ').replace(/\b\w/g, (c) => c.toUpperCase());
}

function buildConfig(chain: RawChain, asset: RawAsset): SourceChainConfig | null {
  if (!asset.chain_id || !asset.htlc?.address || !asset.token?.address) return null;
  const chainIdNum = parseInt(asset.chain_id, 10);
  if (isNaN(chainIdNum)) return null;
  const chainIdHex = '0x' + chainIdNum.toString(16).toUpperCase();
  const defaults = CHAIN_METAMASK_DEFAULTS[asset.chain_id] ?? {
    nativeCurrency: { name: 'Ethereum', symbol: 'ETH', decimals: 18 },
    rpcUrls: [],
  };
  return {
    chainId: asset.chain_id,
    chainIdHex,
    chainName: toChainName(chain.chain),
    htlcAddress: asset.htlc.address,
    tokenAddress: asset.token.address,
    tokenDecimals: asset.decimals,
    solverAddress: asset.solver,
    sourceTimelock: parseInt(chain.source_timelock, 10),
    nativeCurrency: defaults.nativeCurrency,
    rpcUrls: defaults.rpcUrls,
  };
}

// Module-level cache
let cache: { map: Record<string, Record<string, SourceChainConfig>>; fetchedAt: number } | null = null;
const STALE_MS = 60_000;

export interface ChainsResult {
  // chainsMap[chainName][tokenSymbol] → SourceChainConfig
  // e.g. chainsMap['arbitrum_sepolia']['usdc']
  chainsMap: Record<string, Record<string, SourceChainConfig>>;
  loading: boolean;
  error: string | null;
  getChainConfig: (chain: string, tokenSymbol: string) => SourceChainConfig | undefined;
}

export function useChains(): ChainsResult {
  const [chainsMap, setChainsMap] = useState<Record<string, Record<string, SourceChainConfig>>>(cache?.map ?? {});
  const [loading, setLoading] = useState(!cache || Date.now() - cache.fetchedAt > STALE_MS);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (cache && Date.now() - cache.fetchedAt <= STALE_MS) {
      setChainsMap(cache.map);
      setLoading(false);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setError(null);

    fetch(`${QUOTE_API}/chains`)
      .then((r) => r.json())
      .then((json: { ok: boolean; data: RawChain[] }) => {
        if (cancelled) return;
        if (!json.ok) throw new Error('chains endpoint returned ok=false');

        const map: Record<string, Record<string, SourceChainConfig>> = {};
        for (const chain of json.data) {
          // Only EVM chains with ERC20 HTLC support are valid payment sources
          if (!chain.supported_htlc_schemas.includes('evm:htlc_erc20')) continue;

          for (const asset of chain.assets) {
            if (asset.token?.schema !== 'evm:erc20') continue;
            const cfg = buildConfig(chain, asset);
            if (!cfg) continue;

            // Key by the token symbol derived from the asset id (e.g. "utars_chain_1:usdc" → "usdc")
            const tokenSymbol = asset.id.split(':')[1] ?? asset.id;
            if (!map[chain.chain]) map[chain.chain] = {};
            map[chain.chain][tokenSymbol] = cfg;
          }
        }

        cache = { map, fetchedAt: Date.now() };
        setChainsMap(map);
        setLoading(false);
      })
      .catch((e: unknown) => {
        if (cancelled) return;
        setError(e instanceof Error ? e.message : String(e));
        setLoading(false);
      });

    return () => { cancelled = true; };
  }, []);

  const getChainConfig = (chain: string, tokenSymbol: string) =>
    chainsMap[chain]?.[tokenSymbol.toLowerCase()];

  return { chainsMap, loading, error, getChainConfig };
}
