import * as btc from "@scure/btc-signer";
import { sha256 } from "@noble/hashes/sha256";
import { Script } from "@scure/btc-signer";
import { pubSchnorr } from "@scure/btc-signer/utils.js";
import { type BTC_NETWORK } from "@scure/btc-signer/utils.js";
import { schnorr } from "@noble/curves/secp256k1.js";
import { type HtlcParams } from "./types";
import { type BitcoinChainConfig } from "./types";

const UNIPAY_H_POINT =
  "0250929b74c1a04954b78b4b6035e97a5e078a5a0f28ec96d547bfee9ace803ac0";
const DUST_LIMIT_SATS = 330n;
const TAP_SIGHASH_SINGLE_ANYONECANPAY = 0x83;
const TAP_SIGHASH_DEFAULT = 0x00;

export const REDEEM_SATOSHI_MULTIPLIER = 1_000_000_000;

export type { BitcoinChainConfig } from "./types";

interface EsploraUtxo {
  txid: string;
  vout: number;
  value: number;
}

interface EsploraTxVout {
  value: number;
  scriptpubkey: string;
}

interface EsploraTransaction {
  status?: {
    confirmed: boolean;
    block_height?: number;
    block_hash?: string;
    block_time?: number;
  };
  vout: EsploraTxVout[];
}

export function getBitcoinNetwork(config: BitcoinChainConfig): BTC_NETWORK {
  switch (config.network) {
    case "bitcoin_testnet":
    case "bitcoin_signet":
      return btc.TEST_NETWORK;
    case "bitcoin_regtest":
      return {
        bech32: "bcrt",
        pubKeyHash: 0x6f,
        scriptHash: 0xc4,
        wif: 0xef,
      } as const;
    default:
      return btc.NETWORK;
  }
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}

export function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) {
    throw new Error("Invalid hex value");
  }
  const bytes = new Uint8Array(clean.length / 2);
  for (let i = 0; i < bytes.length; i += 1) {
    bytes[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

export function bitcoinPrivateKeyToBytes(privateKeyHex: string): Uint8Array {
  return hexToBytes(privateKeyHex);
}

function equalBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  return a.every((value, index) => value === b[index]);
}

function filterTapLeafScript(
  payment: ReturnType<typeof buildPayment>["payment"],
  script: Uint8Array,
) {
  return (payment.tapLeafScript ?? []).filter(([, candidate]) =>
    equalBytes(candidate.subarray(0, -1), script),
  );
}

function unipayNumsInternalKey(): Uint8Array {
  const r = sha256(new TextEncoder().encode("UnipayHTLC"));
  const hPoint = schnorr.Point.fromHex(UNIPAY_H_POINT);
  const rPoint = schnorr.Point.BASE.multiply(BigInt(`0x${bytesToHex(r)}`));
  return hPoint.add(rPoint).toBytes(true).slice(1);
}

function requireHtlcParams(
  params?: HtlcParams | null,
): asserts params is HtlcParams {
  if (
    !params?.initiator_address ||
    !params.redeemer_address ||
    !params.timelock ||
    !params.secret_hash
  ) {
    throw new Error("HTLC params are incomplete");
  }
}

function redeemLeaf(secretHash: Uint8Array, redeemerPubkey: Uint8Array): Uint8Array {
  return Script.encode(["SHA256", secretHash, "EQUALVERIFY", redeemerPubkey, "CHECKSIG"]);
}

function refundLeaf(timelock: number, initiatorPubkey: Uint8Array): Uint8Array {
  return Script.encode([
    timelock,
    "CHECKSEQUENCEVERIFY",
    "DROP",
    initiatorPubkey,
    "CHECKSIG",
  ]);
}

function instantRefundLeaf(
  initiatorPubkey: Uint8Array,
  redeemerPubkey: Uint8Array,
): Uint8Array {
  return Script.encode([
    initiatorPubkey,
    "CHECKSIG",
    redeemerPubkey,
    "CHECKSIGADD",
    2,
    "NUMEQUAL",
  ]);
}

function buildPayment(params: HtlcParams) {
  requireHtlcParams(params);
  const secretHash = hexToBytes(params.secret_hash!);
  const initiatorPubkey = hexToBytes(params.initiator_address!);
  const redeemerPubkey = hexToBytes(params.redeemer_address!);
  const redeem = redeemLeaf(secretHash, redeemerPubkey);
  const refund = refundLeaf(params.timelock!, initiatorPubkey);
  const instantRefund = instantRefundLeaf(initiatorPubkey, redeemerPubkey);
  const payment = btc.p2tr(
    unipayNumsInternalKey(),
    [{ script: redeem }, [{ script: refund }, { script: instantRefund }]],
    undefined,
    true,
  );
  return { payment, redeem, instantRefund };
}

export function deriveBitcoinHtlcAddress(params: {
  network: BTC_NETWORK;
  initiatorPubkeyHex: string;
  redeemerPubkeyHex: string;
  secretHashHex: string;
  timelock: number;
  }): string {
  const payment = buildBitcoinPayment(
    params.network,
    {
      initiator_address: params.initiatorPubkeyHex,
      redeemer_address: params.redeemerPubkeyHex,
      timelock: params.timelock,
      secret_hash: params.secretHashHex,
      recipient_address: null,
    },
  );

  if (!payment.address) {
    throw new Error("Failed to derive Bitcoin HTLC address");
  }

  return payment.address;
}

function buildBitcoinPayment(
  network: BTC_NETWORK,
  params: HtlcParams,
): ReturnType<typeof buildPayment>["payment"] {
  const { payment } = buildPaymentInternal(network, params);
  return payment;
}

function buildPaymentInternal(
  network: BTC_NETWORK,
  params: HtlcParams,
): { payment: ReturnType<typeof btc.p2tr>; redeem: Uint8Array; instantRefund: Uint8Array } {
  requireHtlcParams(params);
  const secretHash = hexToBytes(params.secret_hash!);
  const initiatorPubkey = hexToBytes(params.initiator_address!);
  const redeemerPubkey = hexToBytes(params.redeemer_address!);
  const redeem = redeemLeaf(secretHash, redeemerPubkey);
  const refund = refundLeaf(params.timelock!, initiatorPubkey);
  const instantRefund = instantRefundLeaf(initiatorPubkey, redeemerPubkey);
  const payment = btc.p2tr(
    unipayNumsInternalKey(),
    [{ script: redeem }, [{ script: refund }, { script: instantRefund }]],
    network,
    true,
  );
  return { payment, redeem, instantRefund };
}

function sumUtxos(utxos: Array<{ value: number }>): bigint {
  return utxos.reduce((sum, utxo) => sum + BigInt(utxo.value), 0n);
}

function isPlaceholderSignature(bytes: Uint8Array): boolean {
  return bytes.length === 65 && bytes.every((value) => value === 0);
}

function getSingleInstantRefundPlaceholderIndex(witness: Uint8Array[]): number {
  const indexes = witness.reduce<number[]>((acc, item, idx) => {
    if (isPlaceholderSignature(item)) acc.push(idx);
    return acc;
  }, []);

  if (indexes.length === 0) {
    throw new Error("Bitcoin cancel transaction is missing the local signature placeholder");
  }
  if (indexes.length > 1) {
    throw new Error("Bitcoin cancel transaction has multiple local signature placeholders");
  }
  return indexes[0] ?? 0;
}

function buildBitcoinRedeemTransaction(params: {
  utxos: Array<{ txid: string; vout: number; value: number }>;
  paymentScript: Uint8Array;
  recipientAddress: string;
  spendAmount: bigint;
  redeemLeaf: Uint8Array;
  controlBlock: Uint8Array;
  secret: Uint8Array;
  privateKey: Uint8Array;
  sign: boolean;
  network: BTC_NETWORK;
}) {
  const {
    utxos,
    paymentScript,
    recipientAddress,
    spendAmount,
    redeemLeaf: leafScript,
    controlBlock,
    secret,
    privateKey,
    sign,
    network,
  } = params;

  const tx = new btc.Transaction({ allowUnknownInputs: true });
  const prevoutScripts = utxos.map(() => paymentScript);
  const prevoutAmounts = utxos.map((utxo) => BigInt(utxo.value));

  for (const utxo of utxos) {
    tx.addInput({
      txid: utxo.txid,
      index: utxo.vout,
      witnessUtxo: {
        script: paymentScript,
        amount: BigInt(utxo.value),
      },
    });
  }

  tx.addOutputAddress(recipientAddress, spendAmount, network);

  for (let i = 0; i < utxos.length; i += 1) {
    let signature = new Uint8Array(64);
    if (sign) {
      const digest = tx.preimageWitnessV1(
        i,
        prevoutScripts,
        TAP_SIGHASH_DEFAULT,
        prevoutAmounts,
        -1,
        leafScript,
      );
      signature = new Uint8Array(schnorr.sign(digest, privateKey));
    }

    tx.updateInput(
      i,
      {
        finalScriptWitness: [signature, secret, leafScript, controlBlock],
      },
      true,
    );
  }

  return tx;
}

async function getAddressUtxos(config: BitcoinChainConfig, address: string): Promise<EsploraUtxo[]> {
  const response = await fetch(
    `${config.esplora_url}/address/${encodeURIComponent(address)}/utxo`,
  );
  if (!response.ok) {
    throw new Error("Failed to load Bitcoin UTXOs");
  }
  return (await response.json()) as EsploraUtxo[];
}

export async function getFastFeeRate(config: BitcoinChainConfig): Promise<number> {
  const response = await fetch(`${config.esplora_url}/fee-estimates`);
  if (!response.ok) return 10;
  const data = (await response.json()) as Record<string, number>;
  return Math.max(2, Math.ceil(data["1"] ?? data["2"] ?? 10));
}

async function getBitcoinTransaction(
  config: BitcoinChainConfig,
  txid: string,
): Promise<EsploraTransaction> {
  const response = await fetch(`${config.esplora_url}/tx/${encodeURIComponent(txid)}`);
  if (!response.ok) {
    const error = new Error(`Failed to load Bitcoin transaction ${txid}`);
    Object.assign(error, { status: response.status });
    throw error;
  }
  return (await response.json()) as EsploraTransaction;
}

export async function broadcastBitcoinTx(
  config: BitcoinChainConfig,
  rawTxHex: string,
): Promise<string> {
  const response = await fetch(`${config.esplora_url}/tx`, {
    method: "POST",
    headers: { "Content-Type": "text/plain" },
    body: rawTxHex,
  });
  if (!response.ok) {
    const detail = await response.text();
    throw new Error(
      detail
        ? `Failed to broadcast Bitcoin transaction: ${detail}`
        : "Failed to broadcast Bitcoin transaction",
    );
  }

  return response.text();
}

export async function redeemBitcoinHtlc(params: {
  config: BitcoinChainConfig;
  privateKeyHex: string;
  htlcAddress: string;
  recipientAddress: string;
  secret: Uint8Array;
  htlcParams?: HtlcParams | null;
}): Promise<string> {
  const { config, privateKeyHex, htlcAddress, recipientAddress, secret, htlcParams } = params;
  requireHtlcParams(htlcParams);

  const network = getBitcoinNetwork(config);
  const utxos = await getAddressUtxos(config, htlcAddress);
  if (!utxos.length) {
    throw new Error("No Bitcoin HTLC UTXOs available for redeem");
  }

  const { payment, redeem } = buildPaymentInternal(network, htlcParams);
  const privateKey = bitcoinPrivateKeyToBytes(privateKeyHex);
  const total = sumUtxos(utxos);
  const feeRate = BigInt(await getFastFeeRate(config));

  const redeemTapLeafScript = filterTapLeafScript(payment, redeem);
  const controlBlock = redeemTapLeafScript[0]
    ? btc.TaprootControlBlock.encode(redeemTapLeafScript[0][0])
    : null;

  if (!controlBlock) {
    throw new Error("Bitcoin redeem control block is unavailable");
  }

  const tempTx = buildBitcoinRedeemTransaction({
    utxos,
    paymentScript: payment.script,
    recipientAddress,
    spendAmount: total,
    redeemLeaf: redeem,
    controlBlock,
    secret,
    privateKey,
    sign: false,
    network,
  });

  const fee = BigInt(tempTx.vsize) * feeRate;
  const spendAmount = total - fee;

  if (spendAmount <= DUST_LIMIT_SATS) {
    throw new Error("Bitcoin redeem amount is below dust after fees");
  }

  const signedTx = buildBitcoinRedeemTransaction({
    utxos,
    paymentScript: payment.script,
    recipientAddress,
    spendAmount,
    redeemLeaf: redeem,
    controlBlock,
    secret,
    privateKey,
    sign: true,
    network,
  });

  return broadcastBitcoinTx(config, signedTx.hex);
}

export function deriveBitcoinXOnlyPublicKey(privateKeyHex: string): string {
  return bytesToHex(pubSchnorr(bitcoinPrivateKeyToBytes(privateKeyHex)));
}

export function buildBitcoinVaultAddress(
  config: BitcoinChainConfig,
  privateKeyHex: string,
): string {
  const privateKey = bitcoinPrivateKeyToBytes(privateKeyHex);
  const network = getBitcoinNetwork(config);
  const address = btc.getAddress("tr", privateKey, network);
  if (!address) throw new Error("Failed to derive Bitcoin vault address");
  return address;
}

export async function completeBitcoinInstantRefund(params: {
  config: BitcoinChainConfig;
  privateKeyHex: string;
  cancelTxHex: string;
}): Promise<string> {
  const { config, privateKeyHex, cancelTxHex } = params;
  const network = getBitcoinNetwork(config);

  const tx = btc.Transaction.fromRaw(hexToBytes(cancelTxHex), {
    allowUnknownInputs: true,
    allowUnknownOutputs: true,
  });
  const privateKey = bitcoinPrivateKeyToBytes(privateKeyHex);

  const prevoutScripts: Uint8Array[] = [];
  const prevoutAmounts: bigint[] = [];

  for (let i = 0; i < tx.inputsLength; i += 1) {
    const input = tx.getInput(i);
    if (!input.txid || input.index === undefined) {
      throw new Error("Bitcoin cancel transaction input is incomplete");
    }

    const prevTx = await getBitcoinTransaction(config, bytesToHex(input.txid));
    const prevout = prevTx.vout[input.index];
    if (!prevout) {
      throw new Error(
        `Missing Bitcoin prevout ${bytesToHex(input.txid)}:${input.index}`,
      );
    }

    prevoutScripts.push(hexToBytes(prevout.scriptpubkey));
    prevoutAmounts.push(BigInt(prevout.value));
  }

  for (let i = 0; i < tx.inputsLength; i += 1) {
    const input = tx.getInput(i);
    const witness = input.finalScriptWitness;
    if (!witness || witness.length !== 4) {
      throw new Error("Bitcoin cancel transaction witness is malformed");
    }

    const leafScript = witness[witness.length - 2];
    const digest = tx.preimageWitnessV1(
      i,
      prevoutScripts,
      TAP_SIGHASH_SINGLE_ANYONECANPAY,
      prevoutAmounts,
      -1,
      leafScript,
    );

    const signature = schnorr.sign(digest, privateKey);
    const encodedSignature = new Uint8Array(signature.length + 1);
    encodedSignature.set(signature);
    encodedSignature[signature.length] = TAP_SIGHASH_SINGLE_ANYONECANPAY;

    const placeholderIndex = getSingleInstantRefundPlaceholderIndex(witness);
    const updatedWitness = witness.map((item, index) =>
      index === placeholderIndex ? encodedSignature : item,
    );

    tx.updateInput(i, { finalScriptWitness: updatedWitness }, true);
  }

  return broadcastBitcoinTx(config, tx.hex);
}
