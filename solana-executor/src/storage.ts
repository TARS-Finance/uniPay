import postgres from "postgres";
import logger from "./config";

export type SwapRow = {
  swap_id: string;
  chain: string;
  asset: string;
  initiator: string;
  redeemer: string;
  timelock: string;
  amount: string;
  filled_amount: string;
  secret_hash: string;
  secret: string;
  initiate_tx_hash: string;
  redeem_tx_hash: string;
  refund_tx_hash: string;
};

export type PendingRedeemRow = SwapRow & {
  destination_swap_id: string;
  destination_secret: string;
  destination_initiate_tx_hash: string;
  destination_redeem_tx_hash: string;
};

/**
 * Postgres helper for the Solana executor.
 *
 * Only reads and writes the shared `swaps` table — no migrations are applied
 * from this service; the canonical schema is owned by the watchers / orderbook
 * crate (see Tars-rs/crates/orderbook).
 */
export default class Storage {
  db: postgres.Sql;
  chain: string;

  constructor(url: string, chain: string) {
    this.db = postgres(url, { max: 10 });
    this.chain = chain;
  }

  /**
   * Locate a Solana source swap by the create-order id (swap_id).
   *
   * In the shared schema the source swap is the row keyed by
   * matched_orders.source_swap_id. We look it up via the join on create_orders
   * so a caller only needs the `create_id` / `order_id` that the frontend
   * received when the invoice was generated.
   */
  async getSourceSwapByOrderId(orderId: string): Promise<SwapRow | null> {
    const rows = await this.db<SwapRow[]>`
      SELECT s.swap_id, s.chain, s.asset, s.initiator, s.redeemer,
             s.timelock::text AS timelock,
             s.amount::text AS amount,
             s.filled_amount::text AS filled_amount,
             s.secret_hash, s.secret,
             s.initiate_tx_hash, s.redeem_tx_hash, s.refund_tx_hash
      FROM swaps s
      JOIN matched_orders mo ON mo.source_swap_id = s.swap_id
      WHERE (mo.create_order_id = ${orderId} OR s.swap_id = ${orderId})
        AND s.chain = ${this.chain}
      LIMIT 1
    `;
    return rows[0] ?? null;
  }

  /**
   * Return Solana source swaps whose matched destination swap was redeemed and
   * exposed the preimage, but where the Solana source redeem has not yet been
   * submitted.
   */
  async getPendingRedeems(redeemer: string): Promise<PendingRedeemRow[]> {
    return this.db<PendingRedeemRow[]>`
      SELECT ss.swap_id, ss.chain, ss.asset, ss.initiator, ss.redeemer,
             ss.timelock::text AS timelock,
             ss.amount::text AS amount,
             ss.filled_amount::text AS filled_amount,
             ss.secret_hash, ss.secret,
             ss.initiate_tx_hash, ss.redeem_tx_hash, ss.refund_tx_hash,
             ds.swap_id          AS destination_swap_id,
             ds.secret           AS destination_secret,
             ds.initiate_tx_hash AS destination_initiate_tx_hash,
             ds.redeem_tx_hash   AS destination_redeem_tx_hash
      FROM matched_orders mo
      JOIN swaps ss ON mo.source_swap_id = ss.swap_id
      JOIN swaps ds ON mo.destination_swap_id = ds.swap_id
      WHERE ss.chain            = ${this.chain}
        AND ss.redeemer         = ${redeemer}
        AND ss.initiate_tx_hash IS NOT NULL AND ss.initiate_tx_hash != ''
        AND ds.initiate_tx_hash IS NOT NULL AND ds.initiate_tx_hash != ''
        AND ds.secret           IS NOT NULL AND ds.secret           != ''
        AND ds.redeem_tx_hash   IS NOT NULL AND ds.redeem_tx_hash   != ''
        AND (ss.redeem_tx_hash  IS NULL     OR  ss.redeem_tx_hash   = '')
        AND (ss.refund_tx_hash  IS NULL     OR  ss.refund_tx_hash   = '')
        AND (ds.refund_tx_hash  IS NULL     OR  ds.refund_tx_hash   = '')
      ORDER BY mo.created_at ASC
    `;
  }

  /**
   * Return Solana source swaps whose timelock has expired and that have not yet
   * been redeemed or refunded.  The on-chain program enforces the slot-based
   * expiry; we check the stored timelock value here as an early filter.
   */
  async getPendingRefunds(redeemer: string): Promise<SwapRow[]> {
    return this.db<SwapRow[]>`
      SELECT ss.swap_id, ss.chain, ss.asset, ss.initiator, ss.redeemer,
             ss.timelock::text AS timelock,
             ss.amount::text AS amount,
             ss.filled_amount::text AS filled_amount,
             ss.secret_hash, ss.secret,
             ss.initiate_tx_hash, ss.redeem_tx_hash, ss.refund_tx_hash
      FROM matched_orders mo
      JOIN swaps ss ON mo.source_swap_id = ss.swap_id
      WHERE ss.chain            = ${this.chain}
        AND ss.redeemer         = ${redeemer}
        AND ss.initiate_tx_hash IS NOT NULL AND ss.initiate_tx_hash != ''
        AND (ss.redeem_tx_hash  IS NULL     OR  ss.redeem_tx_hash   = '')
        AND (ss.refund_tx_hash  IS NULL     OR  ss.refund_tx_hash   = '')
        AND ss.timelock::bigint < EXTRACT(EPOCH FROM now())::bigint
      ORDER BY mo.created_at ASC
    `;
  }

  async recordRefund(swapId: string, txHash: string, slot: number): Promise<void> {
    await this.db`
      UPDATE swaps SET
        refund_tx_hash      = ${txHash},
        refund_block_number = ${slot}::bigint,
        refund_timestamp    = now(),
        updated_at          = now()
      WHERE swap_id = ${swapId}
        AND chain   = ${this.chain}
        AND refund_tx_hash = ''
    `;
    logger.info("recorded refund in swaps table", { swapId, txHash });
  }

  async recordRedeem(
    swapId: string,
    txHash: string,
    secretHex: string,
    slot: number,
  ): Promise<void> {
    await this.db`
      UPDATE swaps SET
        redeem_tx_hash      = ${txHash},
        redeem_block_number = ${slot}::bigint,
        redeem_timestamp    = now(),
        secret              = ${secretHex},
        updated_at          = now()
      WHERE swap_id = ${swapId}
        AND chain   = ${this.chain}
        AND redeem_tx_hash = ''
    `;
    logger.info("recorded redeem in swaps table", { swapId, txHash });
  }
}
