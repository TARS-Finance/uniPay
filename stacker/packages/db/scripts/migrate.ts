import { migrate } from "drizzle-orm/node-postgres/migrator";
import { openDatabase } from "../src/client.js";

const { client, db } = openDatabase();

await client.connect();

try {
  await migrate(db, {
    migrationsFolder: "./packages/db/drizzle/migrations"
  });
} finally {
  await client.end();
}
