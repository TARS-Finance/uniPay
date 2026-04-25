import { drizzle } from "drizzle-orm/node-postgres";
import type { NodePgDatabase } from "drizzle-orm/node-postgres";
import { Client } from "pg";
import * as schema from "../drizzle/schema.js";

const DEFAULT_DATABASE_URL = "postgres://stacker:stacker@localhost:5432/stacker";

export type StackerDatabase = NodePgDatabase<typeof schema>;

export function getDatabaseUrl(): string {
  return process.env.DATABASE_URL ?? DEFAULT_DATABASE_URL;
}

export function openPgClient(connectionString = getDatabaseUrl()): Client {
  return new Client({ connectionString });
}

export function openDatabase(connectionString = getDatabaseUrl()): {
  client: Client;
  db: StackerDatabase;
} {
  const client = openPgClient(connectionString);
  const db = drizzle({ client, schema });

  return { client, db };
}
