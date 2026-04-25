import { BN, BorshCoder, EventParser, Idl, web3 } from "@coral-xyz/anchor";
import { z } from "zod/v4";

import { Transaction } from "./solana-client";

export const Initiated = z.object({
  name: z.literal("Initiated"),
  swap_amount: z.instanceof(BN),
  expires_in_slots: z.instanceof(BN),
  initiator: z.instanceof(web3.PublicKey),
  redeemer: z.instanceof(web3.PublicKey),
  secret_hash: z.array(z.number()).length(32),
  // Mint will only be emitted by the SPL program
  mint: z.optional(z.instanceof(web3.PublicKey)),
});

export const Redeemed = z.object({
  name: z.literal("Redeemed"),
  initiator: z.instanceof(web3.PublicKey),
  secret: z.array(z.number()).length(32),
});

export const Refunded = z.object({
  name: z.enum(["Refunded", "InstantRefunded"]),
  initiator: z.instanceof(web3.PublicKey),
  secret_hash: z.array(z.number()).length(32),
});

export const SwapEvent = z.discriminatedUnion("name", [
  Initiated,
  Redeemed,
  Refunded,
]);

export type Initiated = z.infer<typeof Initiated>;
export type Redeemed = z.infer<typeof Redeemed>;
export type Refunded = z.infer<typeof Refunded>;
export type SwapEvent = z.infer<typeof SwapEvent>;

export default class SolanaEventParser {
  parser: EventParser;

  constructor(idl: Idl) {
    const programId = new web3.PublicKey(idl.address);
    const coder = new BorshCoder(idl);
    this.parser = new EventParser(programId, coder);
  }

  parseEvents(transaction: Transaction): SwapEvent[] {
    const events = Array.from(this.parser.parseLogs(transaction.logs));

    return events.map((event) =>
      SwapEvent.parse({ name: event.name, ...event.data }),
    );
  }
}
