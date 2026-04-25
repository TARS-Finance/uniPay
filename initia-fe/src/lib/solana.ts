import {
  Connection,
  PublicKey,
  Transaction,
  TransactionInstruction,
  SystemProgram,
} from '@solana/web3.js';
import { Buffer } from 'buffer';

import {
  SOLANA_RPC_URL,
  SOLANA_NATIVE_HTLC_PROGRAM_ID,
  SOLANA_EXECUTOR_API,
} from './config';

const LAMPORTS_PER_SOL = 1_000_000_000n;
// Anchor account discriminator (8) + swap_amount (8) + expiry_slot (8)
// + initiator pubkey (32) + redeemer pubkey (32) + secret_hash (32).
const NATIVE_SWAP_ACCOUNT_SIZE = 120;
const SOLANA_TX_FEE_BUFFER_LAMPORTS = 5_000n;

// ── Phantom wallet typing ────────────────────────────────────────────────────

export type PhantomProvider = {
  isPhantom?: boolean;
  publicKey?: { toBase58: () => string; toBytes: () => Uint8Array };
  connect: (opts?: { onlyIfTrusted?: boolean }) => Promise<{ publicKey: { toBase58: () => string } }>;
  disconnect: () => Promise<void>;
  signAndSendTransaction: (tx: Transaction) => Promise<{ signature: string }>;
  signTransaction?: (tx: Transaction) => Promise<Transaction>;
};

type Web3Buffer = ConstructorParameters<typeof TransactionInstruction>[0]['data'];

export function getPhantom(): PhantomProvider | null {
  if (typeof window === 'undefined') return null;
  // Phantom injects at window.solana and (newer) window.phantom.solana
  const w = window as unknown as {
    solana?: PhantomProvider;
    phantom?: { solana?: PhantomProvider };
  };
  if (w.phantom?.solana?.isPhantom) return w.phantom.solana;
  if (w.solana?.isPhantom) return w.solana;
  return null;
}

export async function connectPhantom(): Promise<string> {
  const p = getPhantom();
  if (!p) throw new Error('Phantom wallet not found — install https://phantom.app');
  const res = await p.connect();
  return res.publicKey.toBase58();
}

// ── Anchor-compatible instruction encoding for `initiate` ────────────────────
//
// IDL signature for solana-native-swaps::initiate:
//   args: swap_amount: u64, expires_in_slots: u64, redeemer: Pubkey,
//         secret_hash: [u8;32], destination_data: Option<bytes>
//   accounts: swap_account (writable, PDA), initiator (signer, writable), system_program
//
// Anchor 0.30+ stores the explicit instruction discriminator in the IDL — we
// pin it directly so we never silently drift if the convention changes.

const INITIATE_DISCRIMINATOR = new Uint8Array([5, 63, 123, 113, 153, 75, 148, 14]);

function u64LeBytes(n: bigint): Uint8Array {
  const out = new Uint8Array(8);
  let value = n;
  for (let i = 0; i < 8; i++) {
    out[i] = Number(value & 0xffn);
    value >>= 8n;
  }
  return out;
}

function encodeInitiateData(
  swapAmount: bigint,
  expiresInSlots: bigint,
  redeemer: PublicKey,
  secretHash: Uint8Array,
): Uint8Array {
  if (secretHash.length !== 32) throw new Error('secret_hash must be 32 bytes');
  // 8 disc + 8 swap_amount + 8 expires + 32 redeemer + 32 secret_hash + 1 None tag
  const out = new Uint8Array(8 + 8 + 8 + 32 + 32 + 1);
  out.set(INITIATE_DISCRIMINATOR, 0);
  out.set(u64LeBytes(swapAmount), 8);
  out.set(u64LeBytes(expiresInSlots), 16);
  out.set(redeemer.toBytes(), 24);
  out.set(secretHash, 56);
  // destination_data: Option<bytes> = None → single zero byte (Borsh tag)
  out[88] = 0;
  return out;
}

function swapAccountPda(programId: PublicKey, initiator: PublicKey, secretHash: Uint8Array): PublicKey {
  const [pda] = PublicKey.findProgramAddressSync(
    [new TextEncoder().encode('swap_account'), initiator.toBytes(), secretHash],
    programId,
  );
  return pda;
}

function formatLamports(lamports: bigint): string {
  const whole = lamports / LAMPORTS_PER_SOL;
  const fractional = (lamports % LAMPORTS_PER_SOL).toString().padStart(9, '0').replace(/0+$/, '');
  return fractional ? `${whole}.${fractional}` : `${whole}`;
}

function extractErrorMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === 'string') return err;
  if (err && typeof err === 'object') {
    const obj = err as Record<string, unknown>;
    for (const key of ['message', 'errorMessage', 'reason', 'details']) {
      const value = obj[key];
      if (typeof value === 'string' && value.trim()) return value;
    }
    try {
      return JSON.stringify(err);
    } catch {
      // fall through
    }
  }
  return String(err);
}

async function ensureSolanaBalance(
  connection: Connection,
  initiator: PublicKey,
  amountLamports: string,
): Promise<void> {
  const [balanceLamports, rentLamports] = await Promise.all([
    connection.getBalance(initiator, 'confirmed'),
    connection.getMinimumBalanceForRentExemption(NATIVE_SWAP_ACCOUNT_SIZE),
  ]);

  const requiredLamports =
    BigInt(amountLamports) + BigInt(rentLamports) + SOLANA_TX_FEE_BUFFER_LAMPORTS;

  if (BigInt(balanceLamports) < requiredLamports) {
    throw new Error(
      `Insufficient SOL balance. Need about ${formatLamports(requiredLamports)} SOL ` +
      `for the swap amount, PDA rent, and tx fee; wallet has ${formatLamports(BigInt(balanceLamports))} SOL.`,
    );
  }
}

function describeInitiateSimulationFailure(logs: string[] | null, err: unknown): string {
  const entries = logs ?? [];
  for (const line of entries) {
    const insufficient = line.match(/Transfer: insufficient lamports (\d+), need (\d+)/);
    if (insufficient) {
      const [, available, needed] = insufficient;
      return `Insufficient SOL balance. Need ${formatLamports(BigInt(needed))} SOL ` +
        `for the transfer, but only ${formatLamports(BigInt(available))} SOL is available.`;
    }
  }

  const anchorMessage = entries
    .map((line) => line.match(/Error Message: (.+?)(?:\.|$)/)?.[1])
    .find(Boolean);
  if (anchorMessage) return anchorMessage;

  const lastLog = [...entries].reverse().find((line) => line.trim().length > 0);
  return lastLog
    ? `Solana initiate simulation failed. ${lastLog}`
    : `Solana initiate simulation failed. ${extractErrorMessage(err)}`;
}

/**
 * Build, sign (via Phantom), and send an `initiate` tx for the native-SOL
 * HTLC program. Returns the confirmed signature.
 */
export async function solanaInitiateHTLC(params: {
  initiator: string;          // base58 pubkey — must match Phantom's connected key
  redeemer: string;           // base58 pubkey of the solana-executor filler
  amountLamports: string;     // decimal string
  expiresInSlots: number;     // relative slot count
  secretHashHex: string;      // 64-char hex (no 0x)
}): Promise<string> {
  const phantom = getPhantom();
  if (!phantom) throw new Error('Phantom wallet not found');

  const connection = new Connection(SOLANA_RPC_URL, 'confirmed');
  const programId = new PublicKey(SOLANA_NATIVE_HTLC_PROGRAM_ID);
  const initiator = new PublicKey(params.initiator);
  const redeemer = new PublicKey(params.redeemer);
  const secretHash = hexToBytes(params.secretHashHex);

  await ensureSolanaBalance(connection, initiator, params.amountLamports);

  const data = encodeInitiateData(
    BigInt(params.amountLamports),
    BigInt(params.expiresInSlots),
    redeemer,
    secretHash,
  );
  const swapAccount = swapAccountPda(programId, initiator, secretHash);

  const ix = new TransactionInstruction({
    programId,
    // IDL order: swap_account, initiator, system_program
    keys: [
      { pubkey: swapAccount,              isSigner: false, isWritable: true },
      { pubkey: initiator,                isSigner: true,  isWritable: true },
      { pubkey: SystemProgram.programId,  isSigner: false, isWritable: false },
    ],
    data: Buffer.from(data) as unknown as Web3Buffer,
  });

  const tx = new Transaction().add(ix);
  tx.feePayer = initiator;
  const { blockhash, lastValidBlockHeight } = await connection.getLatestBlockhash('confirmed');
  tx.recentBlockhash = blockhash;

  const simulation = await connection.simulateTransaction(tx);
  if (simulation.value.err) {
    throw new Error(describeInitiateSimulationFailure(simulation.value.logs, simulation.value.err));
  }

  const { signature } = await phantom.signAndSendTransaction(tx);
  await connection.confirmTransaction({ signature, blockhash, lastValidBlockHeight }, 'confirmed');
  return signature;
}

/**
 * Fetch the Solana executor's filler pubkey (the `redeemer` for the HTLC).
 */
export async function getSolanaExecutorAddress(): Promise<string> {
  const res = await fetch(`${SOLANA_EXECUTOR_API}/accounts`);
  if (!res.ok) throw new Error(`solana executor /accounts failed: ${res.status}`);
  const json = (await res.json()) as Array<{ chain: string; address: string }>;
  const addr = json[0]?.address;
  if (!addr) throw new Error('solana executor returned no address');
  return addr;
}

// ── Utils ────────────────────────────────────────────────────────────────────

function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
  if (clean.length !== 64) throw new Error(`expected 32-byte hex, got ${clean.length} chars`);
  const out = new Uint8Array(32);
  for (let i = 0; i < 32; i++) out[i] = parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}
