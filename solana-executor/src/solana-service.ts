import * as anchor from "@coral-xyz/anchor";
import { Program, web3 } from "@coral-xyz/anchor";

import nativeIdl from "../idls/solana_native_swaps.json";
import logger from "./config";

/**
 * Thin wrapper around the solana-native-swaps Anchor program.
 *
 * Responsibilities in Phase-1:
 *   - redeem(): submit the revealed preimage on-chain to claim the source HTLC
 *   - refund(): fallback if the solver needs to unwind after timelock expiry
 *
 * Initiation is performed by the customer's wallet directly from the frontend,
 * so there is no `initiate` helper here (unlike the unipay reference which
 * initiates destination-side HTLCs).
 */
export class SolanaService {
  private provider: anchor.AnchorProvider;
  private nativeProgram: Program;
  readonly filler: web3.Keypair;
  readonly connection: web3.Connection;

  constructor(rpcUrl: string, filler: web3.Keypair, nativeProgramId: string) {
    this.filler = filler;
    this.connection = new web3.Connection(rpcUrl, "confirmed");
    const wallet = new anchor.Wallet(filler);
    this.provider = new anchor.AnchorProvider(this.connection, wallet, {
      commitment: "confirmed",
    });
    anchor.setProvider(this.provider);

    (nativeIdl as { address: string }).address = nativeProgramId;
    this.nativeProgram = new Program(nativeIdl as anchor.Idl, this.provider);
  }

  programId(): web3.PublicKey {
    return this.nativeProgram.programId;
  }

  fillerPubkey(): web3.PublicKey {
    return this.filler.publicKey;
  }

  private swapAccountPda(initiator: web3.PublicKey, secretHash: Buffer): web3.PublicKey {
    const [pda] = web3.PublicKey.findProgramAddressSync(
      [Buffer.from("swap_account"), initiator.toBuffer(), secretHash],
      this.nativeProgram.programId,
    );
    return pda;
  }

  /**
   * Submit `redeem(secret)` on the native-swaps program.
   *
   * The on-chain program derives the SwapAccount PDA from the initiator and the
   * secret hash, so we only need the initiator pubkey and the 32-byte secret.
   */
  async nativeRedeem(
    secret: Uint8Array,
    secretHash: Uint8Array,
    initiator: web3.PublicKey,
    redeemer: web3.PublicKey,
  ): Promise<string> {
    if (secret.length !== 32) throw new Error("secret must be 32 bytes");
    if (secretHash.length !== 32) throw new Error("secretHash must be 32 bytes");

    const swapAccount = this.swapAccountPda(initiator, Buffer.from(secretHash));

    const sig = await (this.nativeProgram.methods as any)
      .redeem(Array.from(secret))
      .accounts({ swapAccount, initiator, redeemer })
      .rpc({ commitment: "confirmed" });

    logger.info("native redeem submitted", { sig, initiator: initiator.toBase58() });
    return sig;
  }

  async nativeRefund(secretHash: Uint8Array, initiator: web3.PublicKey): Promise<string> {
    if (secretHash.length !== 32) throw new Error("secretHash must be 32 bytes");
    const swapAccount = this.swapAccountPda(initiator, Buffer.from(secretHash));
    const sig = await (this.nativeProgram.methods as any)
      .refund()
      .accounts({ swapAccount, initiator })
      .rpc({ commitment: "confirmed" });
    logger.info("native refund submitted", { sig, initiator: initiator.toBase58() });
    return sig;
  }

  async getSlot(): Promise<number> {
    return this.connection.getSlot("confirmed");
  }

  async isHealthy(): Promise<boolean> {
    try {
      await this.connection.getVersion();
      return true;
    } catch {
      return false;
    }
  }
}
