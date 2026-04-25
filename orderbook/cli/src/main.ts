import { randomBytes, randomUUID } from "node:crypto";
import readline from "node:readline/promises";
import process from "node:process";
import {
  DEFAULT_TERMINAL_STATUSES,
  type ApiAcceptanceTerms,
  type ApiTradeStatus,
  type CliConfig,
  type ChainConfig,
  type ChainKind,
  type CliArgs,
} from "./types";
import { MungerApi } from "./api";
import { loadConfig, mergeConfigWithArgs } from "./config";
import {
  approveIfNeeded,
  createEvmClients,
  redeemEvm,
  sendInitiateTx,
  type EvmChainConfig,
} from "./evm";
import {
  buildBitcoinVaultAddress,
  deriveBitcoinHtlcAddress,
  deriveBitcoinXOnlyPublicKey,
  getBitcoinNetwork,
  redeemBitcoinHtlc,
  type BitcoinChainConfig,
} from "./bitcoin";
import { sha256 } from "@noble/hashes/sha256";

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

function now(): string {
  return new Date().toISOString();
}

function shortHash(hash: string): string {
  if (hash.length <= 18) return hash;
  return `${hash.slice(0, 10)}...${hash.slice(-8)}`;
}

function parseArgs(argv: string[]): CliArgs {
  const args: CliArgs = { config: "./cli/config.json" };

  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--config" && argv[i + 1]) {
      args.config = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--source" && argv[i + 1]) {
      args.sourceAsset = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--target" && argv[i + 1]) {
      args.targetAsset = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--amount" && argv[i + 1]) {
      args.sourceAmount = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--slippage" && argv[i + 1]) {
      const raw = argv[i + 1];
      args.slippage = raw.toLowerCase() === "auto" ? "auto" : Number.parseInt(raw, 10);
      i += 1;
      continue;
    }
  }

  return args;
}

function getChainType(chainName: string, chainConfig?: ChainConfig): ChainKind {
  if (chainConfig) return chainConfig.type;
  const lowered = chainName.toLowerCase();
  if (
    lowered.startsWith("evm") ||
    lowered.startsWith("base") ||
    lowered.startsWith("ethereum") ||
    lowered === "arbitrum" ||
    lowered === "matic" ||
    lowered.startsWith("op_")
  ) {
    return "evm";
  }
  return "bitcoin";
}

function findChainConfig(config: CliConfig, chainName: string): ChainConfig | undefined {
  const exact = config.chains[chainName];
  if (exact) return exact;

  const parsedSuffix = chainName.split("_").pop() || "";
  const chainId = Number.parseInt(parsedSuffix, 10);
  if (Number.isFinite(chainId)) {
    return Object.values(config.chains).find(
      (item) => item.type === "evm" && item.chain_id === chainId,
    );
  }

  return undefined;
}

async function promptInput(question: string, valueDefault?: string): Promise<string> {
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const answer = await rl.question(`${question}${valueDefault ? ` [${valueDefault}]` : ""}: `);
  await rl.close();
  return (answer || valueDefault || "").trim();
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function asHex(value: string): `0x${string}` {
  if (/^0x[0-9a-fA-F]{2,}$/.test(value)) {
    return value.toLowerCase() as `0x${string}`;
  }
  return `0x${value}` as `0x${string}`;
}

function isTxHash(value: string): boolean {
  return /^(0x)?[0-9a-fA-F]{64}$/.test(value);
}

async function pollTrade(
  api: MungerApi,
  tradeId: string,
  timeoutMs: number,
  intervalMs: number,
  requireDestination: boolean,
): Promise<ApiTradeStatus> {
  const deadline = Date.now() + timeoutMs;
  let lastStatus = "";

  while (Date.now() < deadline) {
    const status = await api.getTradeStatus(tradeId);
    if (status.status !== lastStatus) {
      console.log(`[${now()}] trade ${tradeId} status: ${status.status}`);
      lastStatus = status.status;
    }

    const destinationTx = status.outbound_htlc?.tx_hash;
    if (requireDestination && destinationTx) {
      console.log(`[${now()}] destination htlc ready: ${destinationTx}`);
      return status;
    }

    if (DEFAULT_TERMINAL_STATUSES.has(status.status)) {
      return status;
    }

    await sleep(intervalMs);
  }

  throw new Error("Trade poll timed out before destination was deployed");
}

async function waitForManualSourceTx(message: string): Promise<string> {
  while (true) {
    const tx = await promptInput(message);
    if (!isTxHash(tx)) {
      console.log("Please provide a valid 32-byte hex tx hash.");
      continue;
    }
    return asHex(tx);
  }
}

async function initSourceEvm(params: {
  terms: ApiAcceptanceTerms;
  sourceAmount: bigint;
  secretHashHex: string;
  chainConfig: EvmChainConfig;
}): Promise<string> {
  const clients = createEvmClients(params.chainConfig);
  const isNative = !params.terms.source_asset_contract;

  if (!isNative) {
    const tokenAddress = params.terms.source_asset_contract as `0x${string}`;
    const spender = params.terms.htlc_contract_address as `0x${string}`;
    const existingApproval = await approveIfNeeded({
      walletClient: clients.walletClient,
      publicClient: clients.publicClient,
      tokenAddress,
      spender,
      amount: params.sourceAmount,
      account: clients.accountAddress,
      chain: clients.chain,
    });

    if (existingApproval) {
      console.log(`[${now()}] approval tx sent: ${existingApproval}`);
    } else {
      console.log(`[${now()}] approval already sufficient`);
    }
  }

  const txHash = await sendInitiateTx({
    walletClient: clients.walletClient,
    htlcContract: params.terms.htlc_contract_address as `0x${string}`,
    redeemer: params.terms.redeemer_address as `0x${string}`,
    timelock: BigInt(params.terms.timelock),
    amount: params.sourceAmount,
    value: isNative ? params.sourceAmount : undefined,
    secretHash: asHex(params.secretHashHex),
    account: clients.accountAddress,
    chain: clients.chain,
  });

  const receipt = await clients.publicClient.waitForTransactionReceipt({ hash: txHash });
  if (receipt.status !== "success") {
    throw new Error(`Source initiate transaction failed: ${txHash}`);
  }

  console.log(`[${now()}] source init confirmed: ${txHash}`);
  return txHash;
}

function initSourceBitcoin(params: {
  terms: ApiAcceptanceTerms;
  sourceAmount: bigint;
  secretHashHex: string;
  chainName: string;
  chainConfig: BitcoinChainConfig;
}): Promise<string> {
  const htlcAddress = deriveBitcoinHtlcAddress({
    network: getBitcoinNetwork(params.chainConfig),
    initiatorPubkeyHex: deriveBitcoinXOnlyPublicKey(params.chainConfig.private_key),
    redeemerPubkeyHex: params.terms.redeemer_address,
    secretHashHex: params.secretHashHex,
    timelock: params.terms.timelock,
  });

  const vaultAddress = buildBitcoinVaultAddress(
    params.chainConfig,
    params.chainConfig.private_key,
  );

  console.log(`\nBitcoin init required on chain ${params.chainName}`);
  console.log(`Source vault (refund receiver): ${vaultAddress}`);
  console.log(`Send ${params.sourceAmount.toString()} sats-equivalent units to:`);
  console.log(`   ${htlcAddress}`);
  console.log(`Redeemer x-only key: ${params.terms.redeemer_address}`);
  console.log(`Timelock: ${params.terms.timelock}`);

  return waitForManualSourceTx("Paste source funding tx hash after funding the above HTLC address");
}

function getDestinationRecipient(config: CliConfig): string {
  return (config.trade.destination_recipient || config.trade.destination_address || "").trim();
}

async function redeemDestination(params: {
  trade: ApiTradeStatus;
  config: CliConfig;
  secret: Uint8Array;
}): Promise<string> {
  const outbound = params.trade.outbound_htlc;
  if (!outbound) {
    throw new Error("Trade is missing outbound htlc");
  }

  const chainKey = outbound.chain;
  const chainConfig = findChainConfig(params.config, chainKey);
  if (!chainConfig) {
    throw new Error(`Missing chain config for destination ${chainKey}`);
  }

  const chainType = getChainType(chainKey, chainConfig);
  const secretHex = asHex(bytesToHex(params.secret));

  if (chainType === "evm") {
    const htlcId = outbound.htlc_id;
    const htlcContract = outbound.htlc_contract_address;
    if (!htlcId || !htlcContract) {
      throw new Error("Destination EVM status is missing htlc_id or contract");
    }
    if (chainConfig.type !== "evm") {
      throw new Error(`Destination chain config mismatch for ${chainKey}`);
    }

    const clients = createEvmClients(chainConfig);
    const redeemHash = await redeemEvm({
      walletClient: clients.walletClient,
      publicClient: clients.publicClient,
      htlcContract: htlcContract as `0x${string}`,
      orderId: htlcId as `0x${string}`,
      secret: secretHex,
      account: clients.accountAddress,
      chain: clients.chain,
    });

    const receipt = await clients.publicClient.waitForTransactionReceipt({
      hash: redeemHash,
    });
    if (receipt.status !== "success") {
      throw new Error(`Destination EVM redeem failed: ${redeemHash}`);
    }
    return redeemHash;
  }

  if (chainType === "bitcoin") {
    if (chainConfig.type !== "bitcoin") {
      throw new Error(`Destination chain config mismatch for ${chainKey}`);
    }

    const htlcAddress = outbound.htlc_contract_address;
    if (!htlcAddress) {
      throw new Error("Destination BTC status is missing htlc address");
    }

    const recipient = getDestinationRecipient(params.config);
    if (!recipient) {
      throw new Error("config.trade.destination_recipient or config.trade.destination_address is required for BTC destination redeem");
    }

    return redeemBitcoinHtlc({
      config: chainConfig,
      privateKeyHex: chainConfig.private_key,
      htlcAddress,
      recipientAddress: recipient,
      secret: params.secret,
      htlcParams: outbound.htlc_params,
    });
  }

  throw new Error(`Unsupported destination chain type ${chainType}`);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const baseConfig = await loadConfig(args.config);
  const config = mergeConfigWithArgs(baseConfig, args);

  const sourceChainName = config.trade.source_asset.split(":")[0];
  const targetChainName = config.trade.target_asset.split(":")[0];

  const sourceChainConfig = findChainConfig(config, sourceChainName);
  const targetChainConfig = findChainConfig(config, targetChainName);

  if (!sourceChainConfig) {
    throw new Error(`Missing chain config for source chain ${sourceChainName}`);
  }
  if (!targetChainConfig) {
    throw new Error(`Missing chain config for destination chain ${targetChainName}`);
  }

  const sourceChainType = getChainType(sourceChainName, sourceChainConfig);
  const targetChainType = getChainType(targetChainName, targetChainConfig);

  console.log(`[${now()}] source: ${sourceChainName} (${sourceChainType})`);
  console.log(`[${now()}] destination: ${targetChainName} (${targetChainType})`);

  const api = new MungerApi(config.api_url);
  const secret = randomBytes(32);
  const secretHashHex = bytesToHex(sha256(secret));

  console.log(`\nGenerated secret hash: 0x${secretHashHex}`);
  console.log(`Source amount requested: ${config.trade.source_amount}`);

  const quote = await api.createRfq({
    idempotency_key: randomUUID(),
    source_asset: config.trade.source_asset,
    target_asset: config.trade.target_asset,
    source_amount: config.trade.source_amount,
    slippage: config.trade.slippage,
    refund_address: config.trade.refund_address,
    destination_address: config.trade.destination_address,
  });

  if (quote.status !== "accepted") {
    throw new Error(
      `RFQ rejected${"reason" in quote ? `: ${(quote as { reason?: string }).reason}` : ""}`,
    );
  }

  console.log(`[${now()}] quote id: ${quote.quote_id}`);
  console.log(`Execution path: ${quote.execution_path}`);
  console.log(`Estimated target: ${quote.estimated_target_amount}`);

  const selectedInit = await promptInput(
    `Init on chain now [evm|bitcoin] (defaults to source: ${sourceChainType})`,
  );

  const normalizedInit = selectedInit.trim().toLowerCase();
  const sourceInitMode =
    normalizedInit === "evm" || normalizedInit === "bitcoin"
      ? normalizedInit
      : sourceChainType;

  let sourceTxHash: string;

  if (sourceInitMode === "bitcoin") {
    if (sourceChainConfig.type !== "bitcoin") {
      throw new Error(`Source chain ${sourceChainName} not configured as Bitcoin`);
    }
    sourceTxHash = await initSourceBitcoin({
      chainName: sourceChainName,
      chainConfig: sourceChainConfig,
      terms: quote.acceptance_terms,
      secretHashHex,
      sourceAmount: BigInt(quote.acceptance_terms.source_amount),
    });
  } else {
    if (sourceChainConfig.type !== "evm") {
      throw new Error(`Source chain ${sourceChainName} not configured as EVM`);
    }
    sourceTxHash = await initSourceEvm({
      chainConfig: sourceChainConfig,
      terms: quote.acceptance_terms,
      secretHashHex,
      sourceAmount: BigInt(quote.acceptance_terms.source_amount),
    });
  }

  const accept = await api.acceptQuote({
    idempotency_key: randomUUID(),
    quote_id: quote.quote_id,
    source_tx_hash: sourceTxHash,
    source_recipient: config.trade.refund_address,
    destination_recipient: config.trade.destination_address,
    secret_hash: secretHashHex,
  });

  console.log(`[${now()}] trade created: ${accept.trade_id}`);

  const preRedeem = await pollTrade(
    api,
    accept.trade_id,
    config.poll_timeout_ms,
    config.poll_interval_ms,
    true,
  );

  if (!preRedeem.outbound_htlc || !preRedeem.outbound_htlc.tx_hash) {
    if (DEFAULT_TERMINAL_STATUSES.has(preRedeem.status)) {
      throw new Error(`Trade stopped before destination was deployed: ${preRedeem.status}`);
    }
    throw new Error("No outbound destination htlc found in trade status");
  }

  const redeemTx = await redeemDestination({
    trade: preRedeem,
    config,
    secret,
  });
  console.log(`[${now()}] destination redeem submitted: ${shortHash(redeemTx)}`);

  const final = await pollTrade(
    api,
    accept.trade_id,
    config.poll_timeout_ms,
    config.poll_interval_ms,
    false,
  );

  console.log(`[${now()}] final trade status: ${final.status}`);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`Error: ${message}`);
  process.exit(1);
});
