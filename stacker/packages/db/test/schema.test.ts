import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { getDatabaseUrl, openPgClient } from "../src/client.js";
import { users } from "../drizzle/schema.js";

type ConstraintRow = {
  table_name: string;
  column_name: string;
  foreign_table_name: string;
  foreign_column_name: string;
};

describe("database schema", () => {
  const client = openPgClient(getDatabaseUrl());

  beforeAll(async () => {
    await client.connect();
  });

  afterAll(async () => {
    await client.end();
  });

  it("exposes the users table schema object", () => {
    expect(users).toBeDefined();
  });

  it("creates the expected tables", async () => {
    const result = await client.query<{ tablename: string }>(`
      select tablename
      from pg_tables
      where schemaname = 'public'
        and tablename in ('users', 'strategies', 'grants', 'executions', 'positions')
      order by tablename;
    `);

    expect(result.rows.map((row) => row.tablename)).toEqual([
      "executions",
      "grants",
      "positions",
      "strategies",
      "users"
    ]);
  });

  it("creates a unique index on users.initia_address", async () => {
    const result = await client.query<{ indexdef: string }>(`
      select indexdef
      from pg_indexes
      where schemaname = 'public'
        and tablename = 'users'
    `);

    expect(
      result.rows.some((row) =>
        row.indexdef.includes("UNIQUE INDEX")
          && row.indexdef.includes("initia_address")
      )
    ).toBe(true);
  });

  it("ties strategies and grants back to users through foreign keys", async () => {
    const result = await client.query<ConstraintRow>(`
      select
        tc.table_name,
        kcu.column_name,
        ccu.table_name as foreign_table_name,
        ccu.column_name as foreign_column_name
      from information_schema.table_constraints tc
      join information_schema.key_column_usage kcu
        on tc.constraint_name = kcu.constraint_name
       and tc.table_schema = kcu.table_schema
      join information_schema.constraint_column_usage ccu
        on tc.constraint_name = ccu.constraint_name
       and tc.table_schema = ccu.table_schema
      where tc.constraint_type = 'FOREIGN KEY'
        and tc.table_schema = 'public'
        and tc.table_name in ('strategies', 'grants')
      order by tc.table_name, kcu.column_name;
    `);

    expect(result.rows).toEqual([
      {
        table_name: "grants",
        column_name: "user_id",
        foreign_table_name: "users",
        foreign_column_name: "id"
      },
      {
        table_name: "strategies",
        column_name: "user_id",
        foreign_table_name: "users",
        foreign_column_name: "id"
      }
    ]);
  });
});
