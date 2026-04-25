import crypto from "node:crypto";
import { web3 } from "@coral-xyz/anchor";
import logger, { loadConfig, loadKeypair } from "./config";
import { SolanaService } from "./solana-service";
import Storage from "./storage";
import { startServer } from "./server";

const POLL_INTERVAL_MS = 10_000;

function decodeHex32(s: string, field: string): Buffer {
  const clean = s.startsWith("0x") ? s.slice(2) : s;
  if (!/^[0-9a-fA-F]{64}$/.test(clean)) {
    throw new Error(`${field} must be 32-byte hex (got ${s.length} chars)`);
  }
  return Buffer.from(clean, "hex");
}

async function runRedeemPoller(solana: SolanaService, storage: Storage): Promise<void> {
  while (true) {
    try {
      const pending = await storage.getPendingRedeems(solana.fillerPubkey().toBase58());
      for (const swap of pending) {
        try {
          const secretBytes = decodeHex32(swap.destination_secret, "destination secret");
          const storedHash = decodeHex32(swap.secret_hash, "source secret_hash");
          const computed = crypto.createHash("sha256").update(secretBytes).digest();
          if (!computed.equals(storedHash)) {
            logger.warn("destination secret does not match source secret_hash — skipping", {
              swapId: swap.swap_id,
              destinationSwapId: swap.destination_swap_id,
              destinationRedeemTxHash: swap.destination_redeem_tx_hash,
            });
            continue;
          }

          const initiator = new web3.PublicKey(swap.initiator);
          const redeemer = new web3.PublicKey(swap.redeemer);
          if (!redeemer.equals(solana.fillerPubkey())) {
            logger.warn("source swap redeemer does not match this executor's filler key", {
              swapId: swap.swap_id,
              destinationSwapId: swap.destination_swap_id,
              swapRedeemer: swap.redeemer,
              filler: solana.fillerPubkey().toBase58(),
            });
            continue;
          }

          logger.info("auto-redeeming source swap from destination secret", {
            swapId: swap.swap_id,
            destinationSwapId: swap.destination_swap_id,
            destinationRedeemTxHash: swap.destination_redeem_tx_hash,
          });
          const sig = await solana.nativeRedeem(
            Uint8Array.from(secretBytes),
            Uint8Array.from(storedHash),
            initiator,
            redeemer,
          );
          const slot = await solana.getSlot();
          await storage.recordRedeem(swap.swap_id, sig, secretBytes.toString("hex"), slot);
          logger.info("source swap redeemed", { swapId: swap.swap_id, sig });
        } catch (err) {
          logger.error("failed to redeem swap", {
            swapId: swap.swap_id,
            err: err instanceof Error ? err.message : String(err),
          });
        }
      }
    } catch (err) {
      logger.error("poller error", { err: err instanceof Error ? err.message : String(err) });
    }
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
}

async function runRefundPoller(solana: SolanaService, storage: Storage): Promise<void> {
  while (true) {
    try {
      const pending = await storage.getPendingRefunds(solana.fillerPubkey().toBase58());
      for (const swap of pending) {
        try {
          const initiator = new web3.PublicKey(swap.initiator);
          const secretHash = decodeHex32(swap.secret_hash, "source secret_hash");

          logger.info("auto-refunding expired source swap", {
            swapId: swap.swap_id,
            initiator: swap.initiator,
            timelock: swap.timelock,
          });
          const sig = await solana.nativeRefund(Uint8Array.from(secretHash), initiator);
          const slot = await solana.getSlot();
          await storage.recordRefund(swap.swap_id, sig, slot);
          logger.info("source swap refunded", { swapId: swap.swap_id, sig });
        } catch (err) {
          logger.error("failed to refund swap", {
            swapId: swap.swap_id,
            err: err instanceof Error ? err.message : String(err),
          });
        }
      }
    } catch (err) {
      logger.error("refund poller error", { err: err instanceof Error ? err.message : String(err) });
    }
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }
}

async function main() {
  const config = await loadConfig();
  const keypair = await loadKeypair(config.keypairPath);
  const solana = new SolanaService(config.rpcUrl, keypair, config.nativeProgramId);
  const storage = new Storage(config.databaseUrl, config.chainName);

  logger.info("solana-executor starting", {
    chain: config.chainName,
    rpc: config.rpcUrl,
    filler: keypair.publicKey.toBase58(),
    nativeProgram: config.nativeProgramId,
  });

  // Fire-and-forget: poll matched orders where the destination swap on Initia
  // has already been redeemed and exposed the secret, then use that secret to
  // redeem the Solana source swap.
  runRedeemPoller(solana, storage).catch((err) => {
    logger.error("redeem poller crashed", { err: err instanceof Error ? err.stack : String(err) });
  });

  // Fire-and-forget: poll for expired source swaps that were never redeemed and
  // submit refund transactions to return funds to the initiator.
  runRefundPoller(solana, storage).catch((err) => {
    logger.error("refund poller crashed", { err: err instanceof Error ? err.stack : String(err) });
  });

  await startServer({
    chain: config.chainName,
    port: config.serverPort,
    solana,
    storage,
  });
}

if (require.main === module) {
  main().catch((err) => {
    logger.error("solana-executor crashed", {
      err: err instanceof Error ? err.stack : String(err),
    });
    process.exit(1);
  });
}
