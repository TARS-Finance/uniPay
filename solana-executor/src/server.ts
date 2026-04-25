import express, { Request, Response } from "express";
import crypto from "node:crypto";
import { web3 } from "@coral-xyz/anchor";

import logger from "./config";
import { SolanaService } from "./solana-service";
import Storage from "./storage";

export type ServerDeps = {
  chain: string;
  port: number;
  solana: SolanaService;
  storage: Storage;
};

export type SecretRequest = {
  order_id: string;
  secret: string;
};

/**
 * Strip an optional 0x prefix and decode hex. Accepts 32-byte payloads.
 */
function decodeHex32(s: string, field: string): Buffer {
  const clean = s.startsWith("0x") ? s.slice(2) : s;
  if (!/^[0-9a-fA-F]{64}$/.test(clean)) {
    throw new Error(`${field} must be 32-byte hex (got ${s.length} chars)`);
  }
  return Buffer.from(clean, "hex");
}

export function buildApp(deps: ServerDeps): express.Express {
  const app = express();
  app.use(express.json({ limit: "1mb" }));

  // Permissive CORS — the frontend runs on a different origin (vite :5173/:5174)
  // and calls /accounts directly. Keep /secret available for manual recovery.
  app.use((req, res, next) => {
    res.setHeader("Access-Control-Allow-Origin", "*");
    res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
    res.setHeader("Access-Control-Allow-Headers", "Content-Type");
    if (req.method === "OPTIONS") { res.status(204).end(); return; }
    next();
  });

  app.get("/health", async (_req, res) => {
    const healthy = await deps.solana.isHealthy();
    res.status(healthy ? 200 : 503).json({
      status: healthy ? "ok" : "degraded",
      service: "solana-executor",
      chain: deps.chain,
    });
  });

  app.get("/accounts", (_req, res) => {
    res.json([
      {
        chain: deps.chain,
        address: deps.solana.fillerPubkey().toBase58(),
      },
    ]);
  });

  // POST /secret
  //   Body: { order_id, secret }
  //   Manual fallback for redeeming a Solana source HTLC with a known preimage.
  //   The background poller is the primary path; this endpoint remains useful
  //   for retries/backfills when the secret is already known out of band.
  app.post("/secret", async (req: Request, res: Response) => {
    try {
      const { order_id, secret } = req.body as Partial<SecretRequest>;
      if (!order_id || typeof order_id !== "string") {
        return res.status(400).json({ error: "order_id required" });
      }
      if (!secret || typeof secret !== "string") {
        return res.status(400).json({ error: "secret required" });
      }

      const secretBytes = decodeHex32(secret, "secret");
      const swap = await deps.storage.getSourceSwapByOrderId(order_id);
      if (!swap) return res.status(404).json({ error: `order ${order_id} not found` });
      if (swap.initiate_tx_hash === "") {
        return res.status(409).json({ error: "source HTLC not yet initiated" });
      }
      if (swap.redeem_tx_hash !== "") {
        return res.status(409).json({ error: "already redeemed" });
      }
      if (swap.refund_tx_hash !== "") {
        return res.status(409).json({ error: "already refunded" });
      }

      const storedHash = decodeHex32(swap.secret_hash, "stored secret_hash");
      const computed = crypto.createHash("sha256").update(secretBytes).digest();
      if (!computed.equals(storedHash)) {
        return res.status(400).json({ error: "secret does not match secret_hash" });
      }

      let initiator: web3.PublicKey;
      let redeemer: web3.PublicKey;
      try {
        initiator = new web3.PublicKey(swap.initiator);
        redeemer = new web3.PublicKey(swap.redeemer);
      } catch (e) {
        return res.status(500).json({
          error: `stored addresses are not valid Solana pubkeys: ${(e as Error).message}`,
        });
      }

      if (!redeemer.equals(deps.solana.fillerPubkey())) {
        logger.warn("redeemer on swap row does not match this executor's filler key", {
          swap: swap.redeemer,
          filler: deps.solana.fillerPubkey().toBase58(),
        });
      }

      const sig = await deps.solana.nativeRedeem(
        Uint8Array.from(secretBytes),
        Uint8Array.from(storedHash),
        initiator,
        redeemer,
      );

      const slot = await deps.solana.getSlot();
      await deps.storage.recordRedeem(swap.swap_id, sig, secretBytes.toString("hex"), slot);

      res.json({ tx_hash: sig });
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      logger.error("/secret failed", { err: msg });
      res.status(500).json({ error: msg });
    }
  });

  return app;
}

export function startServer(deps: ServerDeps): Promise<void> {
  return new Promise((resolve) => {
    const app = buildApp(deps);
    app.listen(deps.port, () => {
      logger.info("solana-executor HTTP listening", {
        port: deps.port,
        filler: deps.solana.fillerPubkey().toBase58(),
      });
      resolve();
    });
  });
}
