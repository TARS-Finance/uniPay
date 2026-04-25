import {
  createPublicClient,
  createWalletClient,
  http,
  parseAbi,
  type Address,
  type Chain,
  type Hex,
  type PublicClient,
  type WalletClient,
} from "viem";
import { privateKeyToAccount } from "viem/accounts";
import type { EvmChainConfig } from "./types";

export type { EvmChainConfig } from "./types";

const ERC20_HTLC_INITIATE_ABI = parseAbi([
  "function initiate(address redeemer, uint256 timelock, uint256 amount, bytes32 secretHash) external",
]);

const NATIVE_HTLC_INITIATE_ABI = parseAbi([
  "function initiate(address redeemer, uint256 timelock, uint256 amount, bytes32 secretHash) payable",
]);

const HTLC_ACTION_ABI = parseAbi([
  "function redeem(bytes32 orderID, bytes secret) external",
]);

const ERC20_ABI = parseAbi([
  "function approve(address spender, uint256 amount) external returns (bool)",
  "function allowance(address owner, address spender) external view returns (uint256)",
]);

export interface EvmClients {
  chain: Chain;
  accountAddress: Address;
  walletClient: WalletClient;
  publicClient: PublicClient;
}

export function getChainIdFromName(chainName: string): number | null {
  const suffix = chainName.split("_").pop();
  const parsed = Number.parseInt(suffix || "", 10);
  return Number.isFinite(parsed) ? parsed : null;
}

export function createChainObject(config: EvmChainConfig): Chain {
  return {
    id: config.chain_id,
    name: config.chain_name || `chain-${config.chain_id}`,
    nativeCurrency: {
      name: "ETH",
      symbol: "ETH",
      decimals: 18,
    },
    rpcUrls: {
      default: { http: [config.rpc_url] },
      public: { http: [config.rpc_url] },
    },
  };
}

export function createEvmClients(config: EvmChainConfig): EvmClients {
  const chain = createChainObject(config);
  const account = privateKeyToAccount(config.private_key as Hex);
  const transport = http(config.rpc_url);

  const walletClient = createWalletClient({
    chain,
    account,
    transport,
  });
  const publicClient = createPublicClient({ chain, transport });

  return {
    chain,
    accountAddress: account.address,
    walletClient,
    publicClient,
  };
}

export async function approveIfNeeded(params: {
  walletClient: WalletClient;
  publicClient: PublicClient;
  tokenAddress: Hex;
  spender: Hex;
  amount: bigint;
  account: Address;
  chain: Chain;
}): Promise<Hex | null> {
  const { walletClient, publicClient, tokenAddress, spender, amount, account, chain } = params;
  const allowance = (await publicClient.readContract({
    address: tokenAddress,
    abi: ERC20_ABI,
    functionName: "allowance",
    args: [account, spender],
  })) as bigint;

  if (allowance >= amount) return null;

  const txHash = await walletClient.writeContract({
    chain,
    address: tokenAddress,
    abi: ERC20_ABI,
    functionName: "approve",
    args: [spender, amount],
    account,
  });

  await publicClient.waitForTransactionReceipt({ hash: txHash });
  return txHash;
}

export async function sendInitiateTx(params: {
  walletClient: WalletClient;
  htlcContract: Hex;
  redeemer: Hex;
  timelock: bigint;
  amount: bigint;
  value?: bigint;
  secretHash: Hex;
  account: Address;
  chain: Chain;
}): Promise<Hex> {
  const {
    walletClient,
    htlcContract,
    redeemer,
    timelock,
    amount,
    value,
    secretHash,
    account,
    chain,
  } = params;

  if (params.value !== undefined) {
    return walletClient.writeContract({
      chain,
      address: htlcContract,
      abi: NATIVE_HTLC_INITIATE_ABI,
      functionName: "initiate",
      args: [redeemer, timelock, amount, secretHash],
      value,
      account,
    });
  }

  return walletClient.writeContract({
    chain,
    address: htlcContract,
    abi: ERC20_HTLC_INITIATE_ABI,
    functionName: "initiate",
    args: [redeemer, timelock, amount, secretHash],
    account,
  });
}

export async function redeemEvm(params: {
  walletClient: WalletClient;
  publicClient: PublicClient;
  htlcContract: Hex;
  orderId: Hex;
  secret: Hex;
  account: Address;
  chain: Chain;
}): Promise<Hex> {
  const {
    walletClient,
    publicClient,
    htlcContract,
    orderId,
    secret,
    account,
    chain,
  } = params;
  const hash = await walletClient.writeContract({
    chain,
    address: htlcContract,
    abi: HTLC_ACTION_ABI,
    functionName: "redeem",
    args: [orderId, secret],
    account,
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}

export async function instantRefundEvm(params: {
  walletClient: WalletClient;
  publicClient: PublicClient;
  htlcContract: Hex;
  orderId: Hex;
  cancelSignature: Hex;
  account: Address;
  chain: Chain;
}): Promise<Hex> {
  const {
    walletClient,
    publicClient,
    htlcContract,
    orderId,
    cancelSignature,
    account,
    chain,
  } = params;

  const hash = await walletClient.writeContract({
    chain,
    address: htlcContract,
    abi: parseAbi([
      "function instantRefund(bytes32 orderID, bytes signature) external",
    ]),
    functionName: "instantRefund",
    args: [orderId, cancelSignature],
    account,
  });

  await publicClient.waitForTransactionReceipt({ hash });
  return hash;
}
