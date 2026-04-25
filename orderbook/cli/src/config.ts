import fs from "node:fs/promises";
import path from "node:path";
import process from "node:process";
import { type CliArgs, type CliConfig } from "./types";

const DEFAULTS = {
  poll_interval_ms: 5000,
  poll_timeout_ms: 900000,
} as const;

function normalizeChainIdKey(chainKey: string): string {
  return chainKey.trim();
}

function isValidUrl(value: string): boolean {
  try {
    new URL(value);
    return true;
  } catch {
    return false;
  }
}

function normalizePrivateKey(raw: unknown): string {
  if (typeof raw !== "string") {
    throw new Error("private_key must be a string");
  }

  const value = raw.trim();
  if (!value) throw new Error("private key must be set");
  return value.startsWith("0x") ? value.toLowerCase() : `0x${value.toLowerCase()}`;
}

function normalizeChainConfig(entry: unknown): CliConfig["chains"][string] {
  if (!entry || typeof entry !== "object") {
    throw new Error("Invalid chain config entry");
  }

  const type = (entry as { type?: unknown }).type;
  if (type === "evm") {
    const rawChainId = (entry as { chain_id?: unknown }).chain_id;
    const chainId = typeof rawChainId === "number" ? rawChainId : Number(rawChainId);
    if (!Number.isInteger(chainId) || chainId <= 0) {
      throw new Error("evm chain_id must be a positive integer");
    }

    const rpcUrl = (entry as { rpc_url?: unknown }).rpc_url;
    if (typeof rpcUrl !== "string" || !isValidUrl(rpcUrl)) {
      throw new Error(`evm chain ${chainId} requires a valid rpc_url`);
    }

    const privateKey = normalizePrivateKey(
      (entry as { private_key?: unknown }).private_key,
    );

    return {
      type: "evm",
      chain_id: chainId,
      chain_name: typeof (entry as { chain_name?: unknown }).chain_name === "string"
        ? (entry as { chain_name: string }).chain_name
        : undefined,
      rpc_url: rpcUrl,
      private_key: privateKey,
    };
  }

  if (type === "bitcoin") {
    const network = (entry as { network?: unknown }).network;
    if (
      network !== "bitcoin" &&
      network !== "bitcoin_testnet" &&
      network !== "bitcoin_signet" &&
      network !== "bitcoin_regtest"
    ) {
      throw new Error("bitcoin network must be bitcoin|bitcoin_testnet|bitcoin_signet|bitcoin_regtest");
    }

    const esploraUrl = (entry as { esplora_url?: unknown }).esplora_url;
    if (typeof esploraUrl !== "string" || !isValidUrl(esploraUrl)) {
      throw new Error(`bitcoin network ${network} requires a valid esplora_url`);
    }

    const privateKey = normalizePrivateKey(
      (entry as { private_key?: unknown }).private_key,
    );

    return {
      type: "bitcoin",
      network,
      esplora_url: esploraUrl,
      private_key: privateKey,
    };
  }

  throw new Error(`Unknown chain type: ${String(type)}`);
}

export async function loadConfig(filePath?: string): Promise<CliConfig> {
  const defaultPath = path.resolve(process.cwd(), "cli", "config.example.json");
  const source = filePath ? path.resolve(process.cwd(), filePath) : defaultPath;
  const raw = await fs.readFile(source, "utf8");
  const parsed = JSON.parse(raw);

  if (!parsed || typeof parsed !== "object") {
    throw new Error("Invalid config file format");
  }

  const apiUrl = normalizeUrl(parsed.api_url);
  if (!apiUrl) {
    throw new Error("config.api_url is required");
  }

  const tradeInput = parsed.trade;
  if (!tradeInput || typeof tradeInput !== "object") {
    throw new Error("config.trade is required");
  }

  const trade = {
    source_asset: normalizeString(tradeInput.source_asset, "trade.source_asset"),
    target_asset: normalizeString(tradeInput.target_asset, "trade.target_asset"),
    source_amount: normalizeString(tradeInput.source_amount, "trade.source_amount"),
    slippage: normalizeSlippage(tradeInput.slippage),
    refund_address: normalizeString(
      tradeInput.refund_address,
      "trade.refund_address",
    ),
    destination_address: normalizeString(
      tradeInput.destination_address,
      "trade.destination_address",
    ),
    source_recipient:
      typeof tradeInput.source_recipient === "string"
        ? tradeInput.source_recipient
        : undefined,
    destination_recipient:
      typeof tradeInput.destination_recipient === "string"
        ? tradeInput.destination_recipient
        : undefined,
  };

  const chainsInput = parsed.chains;
  if (!chainsInput || typeof chainsInput !== "object") {
    throw new Error("config.chains is required");
  }

  const chains = Object.fromEntries(
    Object.entries(chainsInput as Record<string, unknown>).map(
      ([rawChainName, rawChainConfig]) => [
        normalizeChainIdKey(rawChainName),
        normalizeChainConfig(rawChainConfig),
      ],
    ),
  );

  return {
    api_url: apiUrl,
    poll_interval_ms:
      normalizePositiveInteger(
        parsed.poll_interval_ms,
        DEFAULTS.poll_interval_ms,
        "poll_interval_ms",
      ),
    poll_timeout_ms:
      normalizePositiveInteger(
        parsed.poll_timeout_ms,
        DEFAULTS.poll_timeout_ms,
        "poll_timeout_ms",
      ),
    chains,
    trade,
  };
}

function normalizeUrl(value: unknown): string {
  if (typeof value !== "string") return "";
  return value.endsWith("/") ? value.slice(0, -1) : value;
}

function normalizeString(value: unknown, field: string): string {
  if (typeof value !== "string" || !value.trim()) {
    throw new Error(`${field} is required`);
  }
  return value.trim();
}

function normalizeSlippage(value: unknown): "auto" | number {
  if (value === "auto") return "auto";
  if (value === undefined) return "auto";
  const parsed = typeof value === "number" ? value : Number.parseInt(String(value), 10);
  if (!Number.isFinite(parsed) || parsed < 0) {
    throw new Error("trade.slippage must be 'auto' or a non-negative integer");
  }
  return Math.trunc(parsed);
}

function normalizePositiveInteger(
  value: unknown,
  fallback: number,
  field: string,
): number {
  if (value === undefined) return fallback;
  const parsed =
    typeof value === "number" ? value : Number.parseInt(String(value), 10);
  if (!Number.isInteger(parsed) || parsed <= 0) {
    throw new Error(`${field} must be a positive integer`);
  }
  return parsed;
}

export function mergeConfigWithArgs(
  config: CliConfig,
  args: CliArgs,
): CliConfig {
  return {
    ...config,
    trade: {
      ...config.trade,
      ...(args.sourceAsset ? { source_asset: args.sourceAsset } : {}),
      ...(args.targetAsset ? { target_asset: args.targetAsset } : {}),
      ...(args.sourceAmount ? { source_amount: args.sourceAmount } : {}),
      ...(args.slippage !== undefined ? { slippage: args.slippage } : {}),
    },
  };
}
