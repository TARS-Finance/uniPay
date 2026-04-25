import process from "node:process";
import {
  randomBytes,
  createHash,
} from "node:crypto";
import readline from "node:readline/promises";
import {
  ApiMatchedOrderResponse,
  ApiQuoteResponse,
  type ApiCreateOrderRequest,
  type ApiQuoteRoute,
} from "./types";
import { loadConfig, mergeConfigWithArgs } from "./config";
import { MungerApi } from "./api";

interface OrderbookArgs {
  config: string;
  sourceAsset?: string;
  targetAsset?: string;
  fromAmount?: string;
  toAmount?: string;
  strategyId?: string;
  slippage?: number;
  affiliateFee?: number;
  sourceRecipient?: string;
  destinationRecipient?: string;
  sourceDelegator?: string;
  createOrder: boolean;
  waitDestinationTx: boolean;
  pollIntervalMs?: number;
  pollTimeoutMs?: number;
  secret?: string;
  secretHash?: string;
}

function now(): string {
  return new Date().toISOString();
}

function usage(): void {
  console.log(`Usage:
  npx tsx src/orderbook.ts [options]

Options:
  --config <path>                 Config path (default: ./cli/config.json)
  --source <asset-id>              Source asset id (default: config.trade.source_asset)
  --target <asset-id>              Target asset id (default: config.trade.target_asset)
  --amount <amount>                Source amount (defaults to config.trade.source_amount)
  --to-amount <amount>             Destination amount (exact-out quote mode; ignored if --amount set)
  --strategy-id <id>               Force a strategy for quote/create
  --slippage <bps>                 Max slippage in bps
  --affiliate-fee <bps>            Optional affiliate fee in bps
  --source-recipient <address>      Source chain initiator address
  --destination-recipient <address> Destination chain initiator address
  --source-delegator <address>      Optional source delegator
  --secret <hex32>                 Secret preimage to keep for redeem
  --secret-hash <hex32>            Secret hash if you already manage the preimage elsewhere
  --watch                          Poll order endpoint until destination initiate tx hash appears
  --no-create                      Do not create order after quote
  --poll-interval-ms <ms>          Poll interval for --watch
  --poll-timeout-ms <ms>           Poll timeout for --watch`);
}

function isHex64(value: string): boolean {
  return /^[0-9a-fA-F]{64}$/.test(value);
}

function normalizeHex64(value: string, label: string): string {
  const normalized = value.trim().replace(/^0x/i, "").toLowerCase();
  if (!isHex64(normalized)) {
    throw new Error(`${label} must be a 64-character hex string`);
  }
  return normalized;
}

function parsePositiveInt(raw: string, label: string): number {
  const value = Number.parseInt(raw, 10);
  if (!Number.isInteger(value) || value <= 0) {
    throw new Error(`Invalid ${label}: must be a positive integer`);
  }
  return value;
}

function parseArgs(argv: string[]): OrderbookArgs {
  const args: OrderbookArgs = { config: "./cli/config.json", createOrder: true, waitDestinationTx: false };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--help" || arg === "-h") {
      usage();
      process.exit(0);
    }
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
      args.fromAmount = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--to-amount" && argv[i + 1]) {
      args.toAmount = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--strategy-id" && argv[i + 1]) {
      args.strategyId = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--slippage" && argv[i + 1]) {
      args.slippage = parsePositiveInt(argv[i + 1], "slippage bps");
      i += 1;
      continue;
    }
    if (arg === "--affiliate-fee" && argv[i + 1]) {
      args.affiliateFee = parsePositiveInt(argv[i + 1], "affiliate fee");
      i += 1;
      continue;
    }
    if (arg === "--source-recipient" && argv[i + 1]) {
      args.sourceRecipient = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--destination-recipient" && argv[i + 1]) {
      args.destinationRecipient = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--source-delegator" && argv[i + 1]) {
      args.sourceDelegator = argv[i + 1];
      i += 1;
      continue;
    }
    if (arg === "--secret" && argv[i + 1]) {
      args.secret = argv[i + 1].trim();
      i += 1;
      continue;
    }
    if (arg === "--secret-hash" && argv[i + 1]) {
      args.secretHash = argv[i + 1].trim();
      i += 1;
      continue;
    }
    if (arg === "--watch") {
      args.waitDestinationTx = true;
      continue;
    }
    if (arg === "--poll-interval-ms" && argv[i + 1]) {
      args.pollIntervalMs = parsePositiveInt(argv[i + 1], "poll-interval-ms");
      i += 1;
      continue;
    }
    if (arg === "--poll-timeout-ms" && argv[i + 1]) {
      args.pollTimeoutMs = parsePositiveInt(argv[i + 1], "poll-timeout-ms");
      i += 1;
      continue;
    }
    if (arg === "--no-create") {
      args.createOrder = false;
      continue;
    }

    throw new Error(`Unknown argument: ${arg}`);
  }

  return args;
}

async function promptInput(question: string, valueDefault?: string): Promise<string> {
  const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
  const answer = await rl.question(`${question}${valueDefault ? ` [${valueDefault}]` : ""}: `);
  await rl.close();
  return (answer || valueDefault || "").trim();
}

async function pollOrderDestination(
  api: MungerApi,
  orderId: string,
  timeoutMs: number,
  intervalMs: number,
): Promise<ApiMatchedOrderResponse> {
  const deadline = Date.now() + timeoutMs;
  let lastSourceTx: string | null = null;
  let lastDestinationTx: string | null = null;

  while (Date.now() < deadline) {
    const order = await api.getOrder(orderId);
    const sourceTx = maybeString(order.source_swap.initiate_tx_hash);
    const destinationTx = maybeString(order.destination_swap.initiate_tx_hash);

    if (sourceTx !== lastSourceTx || destinationTx !== lastDestinationTx) {
      console.log(`[${now()}] source-init=${sourceTx || "pending"} destination-init=${destinationTx || "pending"}`);
      if (sourceTx) {
        console.log(`[${now()}] source swap id: ${order.source_swap.swap_id}`);
      }
      if (destinationTx) {
        console.log(`[${now()}] destination swap id: ${order.destination_swap.swap_id}`);
      }
      lastSourceTx = sourceTx;
      lastDestinationTx = destinationTx;
    }

    if (destinationTx) {
      return order;
    }
    await sleep(intervalMs);
  }

  throw new Error("Timed out while waiting for destination initiate tx hash");
}

function maybeString(value: string | null | undefined): string {
  if (typeof value === "string") return value;
  return "";
}

function printRoute(route: ApiQuoteRoute, index: number): void {
  console.log(
    `  [${index}] strategy=${route.strategy_id} solver=${route.solver_id} time=${route.estimated_time}s slippage=${route.slippage} fee=${route.fee} fixedFee=${route.fixed_fee} source=${route.source.amount} ${route.source.asset} destination=${route.destination.amount} ${route.destination.asset}`,
  );
}

function printOrder(order: ApiMatchedOrderResponse): void {
  const sourceAmount = order.source_swap.amount;
  const destinationAmount = order.destination_swap.amount;
  console.log(`[${now()}] order created`);
  console.log(`create_id: ${order.create_order.create_id}`);
  console.log(`pair: ${order.create_order.source_chain} -> ${order.create_order.destination_chain}`);
  console.log(`amounts: ${sourceAmount} -> ${destinationAmount}`);
  console.log(`source swap id: ${order.source_swap.swap_id}`);
  console.log(`destination swap id: ${order.destination_swap.swap_id}`);
  console.log(`source htlc: ${order.source_swap.htlc_address}`);
  console.log(`destination htlc: ${order.destination_swap.htlc_address}`);
}

function printQuote(quote: ApiQuoteResponse): void {
  console.log(`[${now()}] quote`);
  console.log(`input token price: ${quote.input_token_price}`);
  console.log(`output token price: ${quote.output_token_price}`);
  console.log(`routes: ${quote.routes.length}`);
  if (quote.best) {
    console.log(`best: ${quote.best.strategy_id} ${quote.best.source.amount} ${quote.best.source.asset} -> ${quote.best.destination.amount} ${quote.best.destination.asset}`);
  } else {
    console.log("best: none");
  }
  quote.routes.forEach((route, index) => printRoute(route, index + 1));
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function normalizeString(value: string | undefined, fallback: string): string {
  return (value ?? fallback).trim();
}

async function requestSecretHash(): Promise<string> {
  const response = await promptInput("Proceed with random secret hash [Y/n]", "Y");
  if (response.toLowerCase().startsWith("n")) {
    throw new Error("Aborted");
  }
  const secret = randomBytes(32);
  return createHash("sha256").update(secret).digest("hex");
}

function resolveSecretMaterial(args: OrderbookArgs): {
  secretHex: string | null;
  secretHash: string;
  generated: boolean;
} {
  if (args.secret && args.secretHash) {
    throw new Error("Provide either --secret or --secret-hash, not both");
  }

  if (args.secret) {
    const secretHex = normalizeHex64(args.secret, "secret");
    const secretHash = createHash("sha256")
      .update(Buffer.from(secretHex, "hex"))
      .digest("hex");
    return { secretHex, secretHash, generated: false };
  }

  if (args.secretHash) {
    return {
      secretHex: null,
      secretHash: normalizeHex64(args.secretHash, "secret hash"),
      generated: false,
    };
  }

  const secretHex = randomBytes(32).toString("hex");
  const secretHash = createHash("sha256")
    .update(Buffer.from(secretHex, "hex"))
    .digest("hex");
  return { secretHex, secretHash, generated: true };
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const baseConfig = await loadConfig(args.config);
  const config = mergeConfigWithArgs(baseConfig, {
    config: args.config,
    sourceAsset: args.sourceAsset,
    targetAsset: args.targetAsset,
    sourceAmount: args.fromAmount,
    slippage: args.slippage,
  });

  const sourceChainName = config.trade.source_asset.split(":")[0];
  const targetChainName = config.trade.target_asset.split(":")[0];
  const quoteAmountSource = (args.fromAmount || "").trim() || undefined;
  const quoteAmountTo = args.toAmount?.trim();
  const hasFromAmount = Boolean(quoteAmountSource);
  const configuredSourceAmount = config.trade.source_amount.trim();
  const finalFromAmount = hasFromAmount
    ? quoteAmountSource
    : !quoteAmountTo
      ? configuredSourceAmount
      : undefined;
  const finalToAmount = !hasFromAmount ? quoteAmountTo : undefined;

  const sourceRecipient = normalizeString(args.sourceRecipient ?? config.trade.source_recipient, config.trade.refund_address);
  const destinationRecipient = normalizeString(args.destinationRecipient ?? config.trade.destination_recipient, config.trade.destination_address);
  const effectiveSlippage =
    args.slippage ?? (typeof config.trade.slippage === "number" ? config.trade.slippage : undefined);

  const api = new MungerApi(config.api_url);
  const quote = await api.quote({
    from: config.trade.source_asset,
    to: config.trade.target_asset,
    from_amount: finalFromAmount,
    to_amount: finalToAmount,
    strategy_id: args.strategyId,
    slippage: effectiveSlippage,
    affiliate_fee: args.affiliateFee,
  });

  printQuote(quote);

  if (quote.routes.length === 0) {
    throw new Error("No routes returned for the given quote request");
  }

  const strategyId = args.strategyId ?? quote.best?.strategy_id;
  if (!strategyId) {
    throw new Error("No strategy available to create an order");
  }

  if (!args.createOrder) {
    console.log(`[${now()}] --no-create set; skipping order creation`);
    return;
  }

  const response = await promptInput("Proceed with order creation [Y/n]", "Y");
  if (response.toLowerCase().startsWith("n")) {
    throw new Error("Aborted");
  }

  const secretMaterial = resolveSecretMaterial(args);
  if (secretMaterial.secretHex) {
    console.log(`secret: ${secretMaterial.secretHex}`);
  } else {
    console.log("secret: not shown because only --secret-hash was provided");
  }
  console.log(`secret_hash: ${secretMaterial.secretHash}`);

  if (secretMaterial.generated) {
    console.log("Keep this secret safe. You will need it later to redeem the destination HTLC.");
  }

  const createFromAmount = finalFromAmount;
  const createToAmount = finalToAmount;

  const createPayload: ApiCreateOrderRequest = {
    from: config.trade.source_asset,
    to: config.trade.target_asset,
    from_amount: createFromAmount,
    to_amount: createToAmount,
    initiator_source_address: sourceRecipient,
    initiator_destination_address: destinationRecipient,
    secret_hash: secretMaterial.secretHash,
    strategy_id: strategyId,
    affiliate_fee: args.affiliateFee,
    slippage: effectiveSlippage,
    bitcoin_optional_recipient: destinationRecipient,
  };

  if (args.sourceDelegator) {
    createPayload.source_delegator = args.sourceDelegator;
  }

  const header = `[${now()}] creating order`;
  console.log(header);

  const createOrder = await api.createOrder(createPayload);
  printOrder(createOrder);
  console.log(`[${now()}] source chain: ${sourceChainName} target chain: ${targetChainName}`);

  if (!args.waitDestinationTx) {
    return;
  }

  const polled = await pollOrderDestination(
    api,
    createOrder.create_order.create_id,
    args.pollTimeoutMs ?? config.poll_timeout_ms,
    args.pollIntervalMs ?? config.poll_interval_ms,
  );
  console.log(`[${now()}] destination initiate tx hash: ${maybeString(polled.destination_swap.initiate_tx_hash)}`);
}

main().catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(`Error: ${message}`);
  process.exit(1);
});
