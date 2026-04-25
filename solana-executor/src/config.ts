import fs from "node:fs/promises";
import path from "node:path";
import * as toml from "smol-toml";
import winston from "winston";
import { z } from "zod";
import { web3 } from "@coral-xyz/anchor";
import bs58 from "bs58";
import "dotenv/config";

const ConfigSchema = z.object({
  chain_name: z.string().default("solana_devnet"),
  server_port: z.number().int().min(1).default(7778),
  database_url: z.string(),
  rpc_url: z.string(),
  keypair_path: z.string(),
  native_program_id: z.string(),
  spl_program_id: z.string().optional().default(""),
});

export type ConfigData = z.infer<typeof ConfigSchema>;

export class Config {
  chainName: string;
  serverPort: number;
  databaseUrl: string;
  rpcUrl: string;
  keypairPath: string;
  nativeProgramId: string;
  splProgramId: string;

  constructor(data: ConfigData) {
    const parsed = ConfigSchema.parse(data);
    this.chainName = parsed.chain_name;
    this.serverPort = parsed.server_port;
    this.databaseUrl = parsed.database_url;
    this.rpcUrl = parsed.rpc_url;
    this.keypairPath = process.env.SOLANA_KEYPAIR_PATH || parsed.keypair_path;
    this.nativeProgramId = parsed.native_program_id;
    this.splProgramId = parsed.spl_program_id;
  }
}

export async function loadConfig(configPath?: string): Promise<Config> {
  const file = configPath ?? process.env.CONFIG_FILE ?? "Settings.toml";
  const raw = await fs.readFile(path.resolve(file), "utf8");
  return new Config(toml.parse(raw) as unknown as ConfigData);
}

export async function loadKeypair(keypairPath: string): Promise<web3.Keypair> {
  // Prefer SOLANA_PRIVATE_KEY (base58) from env — this matches the `.env` file
  // produced by scripts/gen-keypair.mjs and keeps the secret out of the repo.
  const envKey = process.env.SOLANA_PRIVATE_KEY;
  if (envKey && envKey.trim().length > 0) {
    const decoded = bs58.decode(envKey.trim());
    if (decoded.length !== 64) {
      throw new Error(
        `SOLANA_PRIVATE_KEY decoded to ${decoded.length} bytes — expected 64`,
      );
    }
    return web3.Keypair.fromSecretKey(decoded);
  }

  const resolved = path.resolve(keypairPath);
  const raw = await fs.readFile(resolved, "utf8");
  const bytes = JSON.parse(raw) as number[];
  if (!Array.isArray(bytes) || bytes.length !== 64) {
    throw new Error(`Invalid keypair file at ${resolved} — expected 64-byte array`);
  }
  return web3.Keypair.fromSecretKey(Uint8Array.from(bytes));
}

const logger = winston.createLogger({
  level: process.env.LOG_LEVEL || "info",
  format: winston.format.combine(
    winston.format.timestamp(),
    winston.format.printf(({ level, message, timestamp, ...rest }) => {
      const meta = Object.keys(rest).length ? ` ${JSON.stringify(rest)}` : "";
      return `${timestamp} [${level}] ${message}${meta}`;
    }),
  ),
  transports: [new winston.transports.Console()],
});
export default logger;
