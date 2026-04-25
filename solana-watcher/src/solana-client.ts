import { web3 } from "@coral-xyz/anchor";
import { setTimeout } from "node:timers/promises";

import logger from "./config";

export type Transaction = {
  signature: string;
  slot: number;
  logs: string[];
  blockTime?: Date;
};

export type TxnAndConf = {
  txnSig: string;
  currentConfirmations: number;
};

// Size limit for array inputs in getSignatureStatuses() RPC call
const RPC_CHUNK_SIZE = 256;
export const RPC_WAIT_TIME_MILLIS = 1000;

export const DEFAULT_COUNT = 100;

export default class SolanaClient {
  constructor(public connection: web3.Connection) {}

  /**
   * Returns Transactions in the order of newest first [till ... from]
   */
  async getTransactions(
    programId: web3.PublicKey,
    from?: string,
    limit?: number,
  ): Promise<Transaction[]> {
    if (limit && limit <= 0) {
      throw new Error("invalid limit");
    }

    // Fetch transaction signatures for the solana program, filtering based on `options`
    let fetchedSignatures = [];
    let before = undefined;
    while (true) {
      // The solana RPC returns transactions in order of newest first.
      // It will return a list of the form [before, tx1, tx2, .., until]
      // where before is the newest and until is the oldest
      const sigs: string[] = (
        await this.connection.getSignaturesForAddress(programId, {
          before,
          until: from,
        })
      ).map((response) => response.signature);
      fetchedSignatures.push(...sigs);

      if (
        sigs.length === 0 ||
        sigs.at(-1) === from ||
        (limit && fetchedSignatures.length >= limit)
      ) {
        break;
      }

      before = sigs.at(-1);
      // 100ms should be enough as this returns 1000 transactions per call
      // as such, there wouldn't be a situation where this gets called more than once
      await setTimeout(100);
    }

    if (limit) {
      fetchedSignatures.splice(limit);
    }

    // Fetch the corresponding transactions for the signatures.
    let fetchedTransactions = [];
    for (const signature of fetchedSignatures) {
      const transaction = await this.connection.getTransaction(signature, {
        maxSupportedTransactionVersion: 0,
      });
      fetchedTransactions.push(transaction);
      await setTimeout(RPC_WAIT_TIME_MILLIS);
    }

    const transactions: Transaction[] = [];
    for (let i = 0; i < fetchedTransactions.length; i++) {
      const transaction = fetchedTransactions[i];
      const signature = fetchedSignatures[i];

      if (!transaction) {
        throw new Error(`transaction not found: ${signature}`);
      }
      if (transaction?.meta?.err) {
        logger.warn(`skipping failed transaction`, {
          signature,
          error: transaction.meta.err,
        });
        continue;
      }
      if (!transaction.meta?.logMessages) {
        throw new Error(`logs not found for transaction: ${signature}`);
      }
      const blockTime = transaction.blockTime
        ? new Date(transaction.blockTime * 1000)
        : undefined;
      transactions.push({
        signature,
        slot: transaction.slot,
        logs: transaction.meta.logMessages,
        blockTime,
      });
    }

    return transactions;
  }

  /*
   * Fetches the confirmation statuses for `txns`.
   * 0 -> processed
   * 1 -> confirmed
   * 2 -> finalized
   */
  async getConfirmations(txns: string[]): Promise<TxnAndConf[]> {
    const txnsAndConfs: TxnAndConf[] = [];
    for (let i = 0; i < txns.length; i += RPC_CHUNK_SIZE) {
      const txnsChunk = txns.slice(i, i + RPC_CHUNK_SIZE);
      const statusesChunk = (
        await this.connection.getSignatureStatuses(txnsChunk, {
          searchTransactionHistory: true,
        })
      ).value;
      await setTimeout(RPC_WAIT_TIME_MILLIS);

      for (let j = 0; j < statusesChunk.length; j++) {
        const status = statusesChunk[j];
        const txnSig = txnsChunk[j];
        if (!status) {
          logger.error("invalid txn", {
            signature: txnSig,
          });
          continue;
        }
        if (!status.confirmationStatus) {
          logger.error("could not fetch confirmation status for txn", {
            transaction: txnSig,
          });
          continue;
        }
        const currentConfirmations = enumerateConfirmationString(
          status.confirmationStatus,
        );
        txnsAndConfs.push({
          txnSig,
          currentConfirmations,
        });
      }
    }

    return txnsAndConfs;
  }

  async getBlockTime(slot: number): Promise<Date | undefined> {
    let unixTimestamp = await this.connection.getBlockTime(slot);
    return unixTimestamp ? new Date(unixTimestamp * 1000) : undefined;
  }
}

function enumerateConfirmationString(
  confirmation: web3.TransactionConfirmationStatus,
): number {
  switch (confirmation) {
    case "processed":
      return 0;
    case "confirmed":
      return 1;
    case "finalized":
      return 2;
  }
}
