import { web3 } from "@coral-xyz/anchor";
import { setTimeout } from "node:timers/promises";

import SolanaClient, {
  RPC_WAIT_TIME_MILLIS,
  Transaction,
} from "./solana-client";
import { Storage } from "./storage";
import logger from "./config";
import SolanaEventParser from "./event-parser";

export default class Watcher {
  /* Watcher is a process that updates `storage` with data from on-chain events of `program`*/
  constructor(
    public eventParser: SolanaEventParser,
    public programId: web3.PublicKey,
    public solanaClient: SolanaClient,
    public storage: Storage,
  ) {}

  /*
   * Starts processing solana transactions that have occured after (not including) `startFrom`.
   */
  public async run(pollIntervalSecs: number, startFrom?: string) {
    logger.info("starting watcher", {
      program: this.programId.toString(),
      pollIntervalSecs,
    });

    let from = startFrom;
    if (from === "") {
      from = undefined;
    }

    while (true) {
      const processedTransactions = await this.processFrom(from);
      if (processedTransactions && processedTransactions.length !== 0) {
        logger.info("Processing successful", {
          fromSignature: processedTransactions.at(-1)?.signature,
          tillSignature: processedTransactions.at(0)?.signature,
          count: processedTransactions.length,
          programId: this.programId.toBase58(),
        });
        /// most recently processed transaction in this batch will be the starting point for next batch
        from = processedTransactions[0].signature;
      }
      await sleep(pollIntervalSecs);
    }
  }

  /*
   * Processes on-chain transactions, in order of newest first.
   * If `from` not provided, processes the past 100 transactions.
   * @returns Signature of the latest processed transaction
   */
  async processFrom(from?: string): Promise<Transaction[] | undefined> {
    let transactions;
    try {
      transactions = await this.solanaClient.getTransactions(
        this.programId,
        from,
        from ? undefined : 100,
      );
    } catch (error) {
      logger.error("error fetching transactions", {
        error,
        from,
        programId: this.programId,
      });
      return undefined;
    }

    const transactionsAndEvents = [];
    for (const transaction of transactions) {
      try {
        const parsedEvents = this.eventParser.parseEvents(transaction);
        const eventsAndTransactions = parsedEvents.map((event) => {
          return {
            event,
            transaction,
          };
        });
        transactionsAndEvents.push(...eventsAndTransactions);
      } catch (error) {
        logger.error("error parsing event", {
          error,
          transaction: transaction.signature,
        });
        return undefined;
      }
    }

    const parsedTransactions = [];
    for (const { transaction, event } of transactionsAndEvents) {
      const { signature, slot } = transaction;
      let blockTime = transaction.blockTime;
      // As per Solana RPC docs, the blockTime returned by getTransaction can be null
      // Fetch it using the slot and getBlockTime() if that's the case
      if (!blockTime) {
        try {
          await setTimeout(RPC_WAIT_TIME_MILLIS);
          blockTime = await this.solanaClient.getBlockTime(slot);
        } catch (error) {
          logger.error("error while fetching block time", {
            error,
            event,
            signature,
            slot,
          });
          return undefined;
        }
      }
      if (!blockTime) {
        logger.error("could not find block time for transaction", {
          event,
          signature,
          slot,
        });
        return undefined;
      }
      parsedTransactions.push({
        signature,
        slot,
        blockTime,
        event,
        programId: this.programId,
      });
    }

    try {
      await this.storage.updateEvents(parsedTransactions);
    } catch (error) {
      logger.error("database error while updating events", {
        parsedTransactions,
        error,
      });
      return undefined;
    }

    return transactions;
  }
}

/*
 * Continuous loop of:
 * - Fetching unconfirmed transactions (current_confirmations < required_confirmations)
 * - Updating them in `storage`
 * This will be repeated every `updateIntervalSecs`
 */
export async function updateConfirmations(
  solanaClient: SolanaClient,
  storage: Storage,
  updateIntervalSecs: number,
) {
  while (true) {
    await sleep(updateIntervalSecs);

    let txns;
    try {
      txns = await storage.fetchUnconfirmedTransactions();
    } catch (error) {
      logger.error("error fetching unconfirmed transactions", { error });
      continue;
    }

    let txnsAndConfs;
    try {
      txnsAndConfs = await solanaClient.getConfirmations(txns);
    } catch (error) {
      logger.error("error fetching confirmations", { error, txns });
      continue;
    }

    try {
      await storage.updateConfirmations(txnsAndConfs);
    } catch (error) {
      logger.error("error updating confirmations", {
        transactions: txns,
        error,
      });
      continue;
    }

    if (txns.length !== 0) {
      logger.info("confirmations updated successfully", { transactions: txns });
    }
  }
}

async function sleep(secs: number) {
  await setTimeout(secs * 1000);
}
