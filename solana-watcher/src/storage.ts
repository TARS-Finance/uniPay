import postgres from "postgres";
import crypto from "node:crypto";
import { web3 } from "@coral-xyz/anchor";

import logger from "./config";
import { TxnAndConf } from "./solana-client";
import { SwapEvent } from "./event-parser";

export type ParsedTransaction = {
  signature: string;
  slot: number;
  blockTime: Date;
  programId: web3.PublicKey;
  event: SwapEvent;
};

export interface Storage {
  updateEvents(parsedTransactions: ParsedTransaction[]): Promise<void>;
  fetchUnconfirmedTransactions(): Promise<string[]>;
  updateConfirmations(txns: TxnAndConf[]): Promise<number>;
}

/**
 * Pg storage adapted to the initia-pay `swaps` table schema.
 *
 * Unlike the unipay variant (which writes a separate `swap_events` table), this
 * project stores initiate/redeem/refund directly as columns on the `swaps` row.
 * See watchers/evm-watcher/src/swaps/store.rs for the canonical update shape.
 *
 * Matching keys:
 *   - Initiated: (chain, initiator, redeemer, asset, timelock, secret_hash)
 *   - Redeemed / Refunded: (chain, initiator, secret_hash)
 *
 * Solana addresses are base58 and case-sensitive — do NOT lower() them. Only
 * hex values (secret_hash) are normalised via TRIM + LOWER and comparison
 * tolerant of an optional "0x" prefix.
 */
export default class PgStore implements Storage {
  db: postgres.Sql;
  chain: string;

  constructor(url: string, chain: string) {
    this.db = postgres(url, { max: 20 });
    this.chain = chain;
  }

  async updateEvents(parsedTransactions: ParsedTransaction[]) {
    for (const t of parsedTransactions) {
      try {
        if (t.event.name === "Initiated") {
          await this.handleInitiated(t);
        } else if (t.event.name === "Redeemed") {
          await this.handleRedeemed(t);
        } else if (
          t.event.name === "Refunded" ||
          t.event.name === "InstantRefunded"
        ) {
          await this.handleRefunded(t);
        }
      } catch (err) {
        logger.error("error applying event to swaps table", {
          err: err instanceof Error ? err.message : String(err),
          signature: t.signature,
          event: t.event.name,
        });
      }
    }
  }

  async fetchUnconfirmedTransactions(): Promise<string[]> {
    const rows = await this.db`
      SELECT initiate_tx_hash AS tx_hash FROM swaps
      WHERE chain = ${this.chain}
        AND initiate_tx_hash <> ''
        AND redeem_tx_hash = ''
        AND refund_tx_hash = ''
        AND current_confirmations < required_confirmations
    `;
    return rows.map((r) => r.tx_hash as string);
  }

  async updateConfirmations(txns: TxnAndConf[]): Promise<number> {
    if (txns.length === 0) return 0;
    const rows = txns.map((t) => ({ tx: t.txnSig, c: t.currentConfirmations }));
    // Simple loop — confirmation counts are tiny (max ~2) and batch size is small.
    let updated = 0;
    for (const r of rows) {
      const res = await this.db`
        UPDATE swaps
        SET current_confirmations = ${r.c}
        WHERE chain = ${this.chain}
          AND initiate_tx_hash = ${r.tx}
          AND current_confirmations < ${r.c}
      `;
      updated += res.count ?? 0;
    }
    return updated;
  }

  private async handleInitiated(t: ParsedTransaction): Promise<void> {
    if (t.event.name !== "Initiated") return;
    const secretHash = Buffer.from(t.event.secret_hash).toString("hex");
    const initiator = t.event.initiator.toBase58();
    const redeemer = t.event.redeemer.toBase58();
    const asset = t.event.mint?.toBase58() ?? "primary";
    const amount = t.event.swap_amount.toString();
    const timelock = t.event.expires_in_slots.toString();

    const res = await this.db`
      UPDATE swaps SET
        filled_amount          = ${amount}::numeric,
        initiate_tx_hash       = ${t.signature},
        initiate_block_number  = ${t.slot}::bigint,
        initiate_timestamp     = ${t.blockTime.toISOString()}::timestamptz,
        current_confirmations  = 1,
        updated_at             = now()
      WHERE chain = ${this.chain}
        AND (initiator) = (${initiator})
        AND (redeemer)  = (${redeemer})
        AND (asset) = (${asset})
        AND timelock = ${timelock}::bigint
        AND LOWER(REGEXP_REPLACE(secret_hash, '^0x', '')) = ${secretHash.toLowerCase()}
        AND initiate_tx_hash = ''
    `;
    if ((res.count ?? 0) === 0) {
      logger.warn("initiated event had no matching swap row", {
        signature: t.signature,
        initiator,
        redeemer,
        asset,
        secretHash,
        amount,
        timelock,
      });
    } else {
      logger.info("applied Initiated event", {
        signature: t.signature,
        secretHash,
      });
    }
  }

  private async handleRedeemed(t: ParsedTransaction): Promise<void> {
    if (t.event.name !== "Redeemed") return;
    const secret = Buffer.from(t.event.secret).toString("hex");
    const secretHash = crypto
      .createHash("sha256")
      .update(Buffer.from(t.event.secret))
      .digest("hex");
    const initiator = t.event.initiator.toBase58();

    const res = await this.db`
      UPDATE swaps SET
        redeem_tx_hash       = ${t.signature},
        redeem_block_number  = ${t.slot}::bigint,
        redeem_timestamp     = ${t.blockTime.toISOString()}::timestamptz,
        secret               = ${secret},
        updated_at           = now()
      WHERE chain = ${this.chain}
        AND (initiator) = (${initiator})
        AND LOWER(REGEXP_REPLACE(secret_hash, '^0x', '')) = ${secretHash.toLowerCase()}
        AND initiate_tx_hash <> ''
        AND redeem_tx_hash   = ''
        AND refund_tx_hash   = ''
    `;
    if ((res.count ?? 0) === 0) {
      logger.warn("redeemed event had no matching swap row", {
        signature: t.signature,
        initiator,
        secretHash,
      });
    } else {
      logger.info("applied Redeemed event", {
        signature: t.signature,
        secretHash,
      });
    }
  }

  private async handleRefunded(t: ParsedTransaction): Promise<void> {
    if (t.event.name !== "Refunded" && t.event.name !== "InstantRefunded")
      return;
    const secretHash = Buffer.from(t.event.secret_hash).toString("hex");
    const initiator = t.event.initiator.toBase58();

    const res = await this.db`
      UPDATE swaps SET
        refund_tx_hash       = ${t.signature},
        refund_block_number  = ${t.slot}::bigint,
        refund_timestamp     = ${t.blockTime.toISOString()}::timestamptz,
        updated_at           = now()
      WHERE chain = ${this.chain}
        AND (initiator) = (${initiator})
        AND LOWER(REGEXP_REPLACE(secret_hash, '^0x', '')) = ${secretHash.toLowerCase()}
        AND initiate_tx_hash <> ''
        AND redeem_tx_hash   = ''
        AND refund_tx_hash   = ''
    `;
    if ((res.count ?? 0) === 0) {
      logger.warn("refund event had no matching swap row", {
        signature: t.signature,
        initiator,
        secretHash,
      });
    } else {
      logger.info("applied Refund event", {
        signature: t.signature,
        secretHash,
      });
    }
  }
}
