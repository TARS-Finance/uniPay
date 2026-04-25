import { Idl } from "@coral-xyz/anchor";
import fs from "node:fs/promises";
import path from "node:path";
import * as toml from "smol-toml";
import winston from "winston";
import { z } from "zod";

import nativeIdl from "../idls/solana_native_swaps.json";
import splIdl from "../idls/solana_spl_swaps.json";

const ProgramSchema = z.object({
  program_id: z.string(),
  start_after_transaction: z.string().default(""),
});

const ConfigSchema = z.object({
  chain_name: z.string().default("solana_devnet"),
  confirmations_poll_interval_secs: z.number().int().min(1),
  database_url: z.string(),
  rpc_url: z.string(),
  watcher_poll_interval_secs: z.number().int().min(1),
  native_program: ProgramSchema,
  spl_program: ProgramSchema,
});

type ConfigData = z.infer<typeof ConfigSchema>;

export class Config {
  chainName: string;
  confirmationPollIntervalSecs: number;
  databaseUrl: string;
  rpcUrl: string;
  watcherPollIntervalSecs: number;
  nativeProgram: { idl: Idl; startAfterTransaction: string };
  splProgram: { idl: Idl; startAfterTransaction: string };

  constructor(data: ConfigData) {
    const parsed = ConfigSchema.parse(data);

    // Patch program IDs into IDL objects so the EventParser uses the right address.
    (nativeIdl as { address: string }).address = parsed.native_program.program_id;
    (splIdl as { address: string }).address = parsed.spl_program.program_id;

    this.chainName = parsed.chain_name;
    this.confirmationPollIntervalSecs = parsed.confirmations_poll_interval_secs;
    this.databaseUrl = parsed.database_url;
    this.rpcUrl = parsed.rpc_url;
    this.watcherPollIntervalSecs = parsed.watcher_poll_interval_secs;
    this.nativeProgram = {
      idl: nativeIdl as Idl,
      startAfterTransaction: parsed.native_program.start_after_transaction,
    };
    this.splProgram = {
      idl: splIdl as Idl,
      startAfterTransaction: parsed.spl_program.start_after_transaction,
    };
  }
}

export async function loadConfig(configPath?: string): Promise<Config> {
  const file = configPath ?? process.env.CONFIG_FILE ?? "Settings.toml";
  const resolved = path.resolve(file);
  const raw = await fs.readFile(resolved, "utf8");
  const data = toml.parse(raw) as unknown as ConfigData;
  return new Config(data);
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
