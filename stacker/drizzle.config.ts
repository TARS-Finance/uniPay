import { defineConfig } from "drizzle-kit";

const databaseUrl =
  process.env.DATABASE_URL ?? "postgres://stacker:stacker@localhost:5432/stacker";

export default defineConfig({
  dialect: "postgresql",
  schema: "./packages/db/drizzle/schema.ts",
  out: "./packages/db/drizzle/migrations",
  dbCredentials: {
    url: databaseUrl
  }
});
