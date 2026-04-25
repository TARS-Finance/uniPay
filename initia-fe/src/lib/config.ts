import { defineChain } from 'viem';

// ── Executor ──────────────────────────────────────────────────────────────────
export const EXECUTOR_API         = import.meta.env.VITE_EXECUTOR_API_URL         ?? 'http://localhost:7777';
export const OPINIT_EXECUTOR_API  = import.meta.env.VITE_OPINIT_EXECUTOR_API_URL  ?? 'http://localhost:3000';
export const QUOTE_API            = import.meta.env.VITE_QUOTE_API_URL            ?? 'http://localhost:6969';
export const STACKER_API          = import.meta.env.VITE_STACKER_API_URL          ?? 'http://localhost:3010';
export const SOLANA_EXECUTOR_API  = import.meta.env.VITE_SOLANA_EXECUTOR_API_URL  ?? 'http://localhost:7778';
export const BITCOIN_EXECUTOR_API = import.meta.env.VITE_BITCOIN_EXECUTOR_API_URL ?? 'http://localhost:7779';
export const L1_REST_URL          = import.meta.env.VITE_L1_REST_URL              ?? 'https://rest.testnet.initia.xyz';
export const L1_RPC_URL           = import.meta.env.VITE_L1_RPC_URL               ?? 'https://rpc.testnet.initia.xyz';
export const L1_CHAIN_ID          = import.meta.env.VITE_L1_CHAIN_ID              ?? 'initiation-2';
export const L1_TX_EXPLORER_URL   = import.meta.env.VITE_L1_TX_EXPLORER_URL       ?? `https://scan.testnet.initia.xyz/${import.meta.env.VITE_L1_CHAIN_ID ?? 'initiation-2'}/txs`;
export const ROLLUP_REST_URL      = import.meta.env.VITE_COSMOS_REST              ?? 'http://localhost:1317';
export const USDC_ERC20_ADDRESS = (import.meta.env.VITE_USDC_ERC20
  ?? '0x0000000000000000000000000000000000000000') as `0x${string}`;
export const HTLC_ADDRESS = (import.meta.env.VITE_HTLC_ADDRESS
  ?? '0x0000000000000000000000000000000000000000') as `0x${string}`;

// Solana devnet defaults — overridable via env.
export const SOLANA_RPC_URL = import.meta.env.VITE_SOLANA_RPC_URL ?? 'https://api.devnet.solana.com';
export const SOLANA_NATIVE_HTLC_PROGRAM_ID =
  import.meta.env.VITE_SOLANA_NATIVE_HTLC_PROGRAM_ID ?? '5GtLBHmNBGEzqEDi6SGgKYwbbZru5sm7Sf7Vys8bYqyn';
export const SOLANA_CHAIN_NAME = import.meta.env.VITE_SOLANA_CHAIN_NAME ?? 'solana_devnet';

// Comma-separated allowlist of chain identifiers (e.g. "utars_chain_1,initia") that
// are permitted as the *destination* side of a payment. When unset, no filtering applies.
export const DEST_CHAINS: string[] | null = (() => {
  const raw = import.meta.env.VITE_DEST_CHAINS;
  if (!raw || typeof raw !== 'string') return null;
  const list = raw.split(',').map((s) => s.trim()).filter(Boolean);
  return list.length ? list : null;
})();

// ── Source chain config shape (populated at runtime from GET /chains) ─────────
export interface SourceChainConfig {
  chainId: string;
  chainIdHex: string;
  chainName: string;
  htlcAddress: string;
  tokenAddress: string;
  tokenDecimals: number;
  solverAddress: string;
  sourceTimelock: number;
  nativeCurrency: { name: string; symbol: string; decimals: number };
  rpcUrls: string[];
}

// ── Viem chain definition (EVM JSON-RPC) ─────────────────────────────────────
export const ROLLUP_EVM_CHAIN_ID = 604686810448826n;
export const INITIA_EVM_CHAIN_ID_HEX = '0x' + ROLLUP_EVM_CHAIN_ID.toString(16);

export const initiaEvmChain = defineChain({
  id: Number(ROLLUP_EVM_CHAIN_ID),
  name: 'Universal Pay Rollup',
  nativeCurrency: { name: 'UTARS', symbol: 'UTARS', decimals: 18 },
  rpcUrls: {
    default: { http: [import.meta.env.VITE_JSON_RPC_URL ?? 'http://localhost:8545'] },
  },
  testnet: true,
});

export const INITIA_EVM_CHAIN = {
  chainId: INITIA_EVM_CHAIN_ID_HEX,
  chainName: 'Universal Pay Rollup',
  nativeCurrency: { name: 'UTARS', symbol: 'UTARS', decimals: 18 },
  rpcUrls: [import.meta.env.VITE_JSON_RPC_URL ?? 'http://localhost:8545'],
};

// ── InterwovenKit appchain config ─────────────────────────────────────────────
export const customChain = {
  chain_id:     import.meta.env.VITE_CHAIN_ID ?? 'tars-1',
  chain_name:   'Universal Pay',
  network_type: 'testnet' as const,
  bech32_prefix: 'init' as const,
  apis: {
    rpc:        [{ address: import.meta.env.VITE_COSMOS_RPC  ?? 'http://localhost:26657' }],
    rest:       [{ address: import.meta.env.VITE_COSMOS_REST ?? 'http://localhost:1317'  }],
    indexer:    [{ address: import.meta.env.VITE_INDEXER_URL ?? 'http://localhost:8080'  }],
    'json-rpc': [{ address: import.meta.env.VITE_JSON_RPC_URL ?? 'http://localhost:8545' }],
  },
  fees: {
    fee_tokens: [{
      denom: 'utars',
      fixed_min_gas_price: 0,
      low_gas_price:       0,
      average_gas_price:   0,
      high_gas_price:      0,
    }],
  },
  staking: {
    staking_tokens: [{ denom: 'utars' }],
  },
  native_assets: [{
    denom:    'utars',
    name:     'UTARS',
    symbol:   'UTARS',
    decimals: 18,
  }],
  metadata: {
    is_l1: false,
    minitia: { type: 'minievm' as const, version: '1.2.15' },
  },
};
