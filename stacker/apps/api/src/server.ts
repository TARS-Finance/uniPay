import { createApp } from "./app.js";

const app = await createApp({
  logger: true
});

await app.listen({
  host: "0.0.0.0",
  port: app.stackerConfig.port
});
