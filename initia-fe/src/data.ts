import type { Token, Chain, HistoryTx, Pool, Settlement } from './types';

export const TOKENS: Record<string, Token> = {
  ETH:  { sym: 'ETH',  name: 'Ether',    klass: 'lt-eth',  price: 3420.18 },
  USDC: { sym: 'USDC', name: 'USD Coin', klass: 'lt-usdc', price: 1.0004 },
  SOL:  { sym: 'SOL',  name: 'Solana',   klass: 'lt-sol',  price: 168.42 },
  BTC:  { sym: 'BTC',  name: 'Bitcoin',  klass: 'lt-btc',  price: 72140.00 },
  INIT: { sym: 'INIT', name: 'Initia',   klass: 'lt-init', price: 2.81 },
};

export const CHAINS: Record<string, Chain> = {
  ethereum:        { id: 'ethereum',        name: 'Ethereum',         klass: 'lt-eth',    short: 'ETH',  explorer: 'https://etherscan.io/tx/',            addrExplorer: 'https://etherscan.io/address/' },
  base:            { id: 'base',            name: 'Base',             klass: 'lt-base',   short: 'BASE', explorer: 'https://basescan.org/tx/',            addrExplorer: 'https://basescan.org/address/' },
  base_sepolia:    { id: 'base_sepolia',    name: 'Base Sepolia',     klass: 'lt-base',   short: 'BASE', explorer: 'https://sepolia.basescan.org/tx/',    addrExplorer: 'https://sepolia.basescan.org/address/' },
  arbitrum:        { id: 'arbitrum',        name: 'Arbitrum',         klass: 'lt-arb',    short: 'ARB',  explorer: 'https://arbiscan.io/tx/',             addrExplorer: 'https://arbiscan.io/address/' },
  arbitrum_sepolia:{ id: 'arbitrum_sepolia',name: 'Arbitrum Sepolia', klass: 'lt-arb',    short: 'ARB',  explorer: 'https://sepolia.arbiscan.io/tx/',     addrExplorer: 'https://sepolia.arbiscan.io/address/' },
  optimism:        { id: 'optimism',        name: 'Optimism',         klass: 'lt-op',     short: 'OP',   explorer: 'https://optimistic.etherscan.io/tx/', addrExplorer: 'https://optimistic.etherscan.io/address/' },
  solana:          { id: 'solana',          name: 'Solana',           klass: 'lt-sol',    short: 'SOL',  explorer: 'https://solscan.io/tx/',              addrExplorer: 'https://solscan.io/account/' },
  solana_devnet:   { id: 'solana_devnet',   name: 'Solana Devnet',    klass: 'lt-sol',    short: 'SOL',  explorer: 'https://explorer.solana.com/tx/',     addrExplorer: 'https://explorer.solana.com/address/' },
  bitcoin:         { id: 'bitcoin',         name: 'Bitcoin',          klass: 'lt-btc',    short: 'BTC',  explorer: 'https://mempool.space/tx/',           addrExplorer: 'https://mempool.space/address/' },
  bitcoin_testnet: { id: 'bitcoin_testnet', name: 'Bitcoin Testnet',  klass: 'lt-btc',    short: 'BTC',  explorer: 'https://mempool.space/testnet4/tx/',  addrExplorer: 'https://mempool.space/testnet4/address/' },
  initia:          { id: 'initia',          name: 'Initia',           klass: 'lt-initia', short: 'INIT', explorer: 'https://scan.initia.xyz/initiation-2/txs/', addrExplorer: 'https://scan.initia.xyz/initiation-2/accounts/' },
  utars_chain_1:   { id: 'utars_chain_1',   name: 'Initia',           klass: 'lt-initia', short: 'INIT', explorer: 'https://scan.testnet.initia.xyz/utars-chain-1/txs/', addrExplorer: 'https://scan.testnet.initia.xyz/utars-chain-1/accounts/' },
};

export const CHAIN_TOKENS: Record<string, string[]> = {
  ethereum: ['ETH', 'USDC'],
  base:     ['ETH', 'USDC'],
  arbitrum: ['ETH', 'USDC'],
  optimism: ['ETH', 'USDC'],
  solana:   ['SOL', 'USDC'],
};

export const shortAddr = (a: string) => a ? `${a.slice(0, 6)}…${a.slice(-4)}` : '';

export const mkHash = () =>
  '0x' + Array.from({ length: 64 }, () => '0123456789abcdef'[Math.floor(Math.random() * 16)]).join('');

export const fmtUSD = (n: number, d = 2) =>
  '$' + n.toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });

export const fmtNum = (n: number, d = 4) =>
  Number(n).toLocaleString('en-US', { minimumFractionDigits: 0, maximumFractionDigits: d });

export function relTime(ms: number): string {
  const diff = Date.now() - ms;
  const s = Math.floor(diff / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  return `${d}d ago`;
}

export const MOCK_HISTORY: HistoryTx[] = [
  { id: 'tx1', amount: 24.50,  token: 'USDC', chain: 'base',     destToken: 'INIT', status: 'Settled',  ts: Date.now() - 1000*60*6,    initAmount: 8.718,  srcHash: mkHash(), initHash: mkHash(), swapId: '' },
  { id: 'tx2', amount: 0.042,  token: 'ETH',  chain: 'arbitrum', destToken: 'INIT', status: 'Settled',  ts: Date.now() - 1000*60*42,   initAmount: 51.08,  srcHash: mkHash(), initHash: mkHash(), swapId: '' },
  { id: 'tx3', amount: 120.00, token: 'USDC', chain: 'solana',   destToken: 'INIT', status: 'Pending',  ts: Date.now() - 1000*60*2,    initAmount: 42.71,  srcHash: mkHash(), initHash: null, swapId: '' },
  { id: 'tx4', amount: 15.00,  token: 'USDC', chain: 'ethereum', destToken: 'INIT', status: 'Refunded', ts: Date.now() - 1000*60*60*3, initAmount: 0,      srcHash: mkHash(), initHash: null, swapId: '' },
  { id: 'tx5', amount: 0.25,   token: 'SOL',  chain: 'solana',   destToken: 'INIT', status: 'Settled',  ts: Date.now() - 1000*60*60*19, initAmount: 14.98, srcHash: mkHash(), initHash: mkHash(), swapId: '' },
  { id: 'tx6', amount: 80.00,  token: 'USDC', chain: 'base',     destToken: 'INIT', status: 'Settled',  ts: Date.now() - 1000*60*60*26, initAmount: 28.47, srcHash: mkHash(), initHash: mkHash(), swapId: '' },
];

export const MOCK_POOLS: Pool[] = [
  { id: 'init-usdc', name: 'INIT / USDC', tokens: ['INIT', 'USDC'], staked: 6240.11, apy: 24.8, earned: 412.08, chain: 'initia', tvl: '4.2M' },
  { id: 'init-eth',  name: 'INIT / ETH',  tokens: ['INIT', 'ETH'],  staked: 4820.53, apy: 18.3, earned: 286.44, chain: 'initia', tvl: '8.7M' },
  { id: 'init-atom', name: 'INIT / ATOM', tokens: ['INIT', 'SOL'],  staked: 3580.21, apy: 21.1, earned: 192.77, chain: 'initia', tvl: '2.1M' },
  { id: 'init-tia',  name: 'INIT / TIA',  tokens: ['INIT', 'BTC'],  staked: 3600.07, apy: 15.6, earned: 50.88,  chain: 'initia', tvl: '1.8M' },
];

export const MOCK_SETTLEMENTS: Settlement[] = [
  { id: 's1', amount: 28.47,  token: 'INIT', srcChain: 'base',     ts: Date.now() - 1000*30,     staked: true },
  { id: 's2', amount: 14.08,  token: 'INIT', srcChain: 'solana',   ts: Date.now() - 1000*60*4,   staked: true },
  { id: 's3', amount: 51.20,  token: 'INIT', srcChain: 'arbitrum', ts: Date.now() - 1000*60*12,  staked: true },
  { id: 's4', amount: 9.55,   token: 'INIT', srcChain: 'base',     ts: Date.now() - 1000*60*22,  staked: false },
  { id: 's5', amount: 102.80, token: 'INIT', srcChain: 'ethereum', ts: Date.now() - 1000*60*45,  staked: true },
  { id: 's6', amount: 6.30,   token: 'INIT', srcChain: 'solana',   ts: Date.now() - 1000*60*60,  staked: false },
  { id: 's7', amount: 42.71,  token: 'INIT', srcChain: 'base',     ts: Date.now() - 1000*60*90,  staked: true },
];
