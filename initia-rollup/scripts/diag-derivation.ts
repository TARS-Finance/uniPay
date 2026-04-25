import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { mnemonicToSeedSync } from '@scure/bip39';
import { HDKey } from '@scure/bip32';
import { sha256 } from '@noble/hashes/sha2.js';
import { ripemd160 } from '@noble/hashes/legacy.js';
import { keccak_256 } from '@noble/hashes/sha3.js';
import { bech32 } from 'bech32';

// Read validator mnemonic from system-keys.json
const path = resolve(process.cwd(), 'weave/system-keys.json');
const sys = JSON.parse(readFileSync(path, 'utf8'));
const mnemonic = sys.system_keys.validator.mnemonic as string;
const writtenAddr = sys.system_keys.validator.l1_address as string;

console.log('mnemonic:', mnemonic.split(' ').slice(0, 3).join(' '), '...(redacted)');
console.log('what we wrote in launch_config:', writtenAddr);
console.log('what minitiad derived:           init1xl7d3dhpts7tp8d85ez8f7m6ydn3dvgrljmnxg');
console.log();

const seed = mnemonicToSeedSync(mnemonic);

function bech32Init(hash20: Uint8Array): string {
  return bech32.encode('init', bech32.toWords(hash20));
}

const candidates: Array<{ label: string; path: string; algo: 'cosmos' | 'eth' }> = [
  { label: 'cosmos cointype 118 + ripemd', path: "m/44'/118'/0'/0/0", algo: 'cosmos' },
  { label: 'cosmos cointype 60  + ripemd', path: "m/44'/60'/0'/0/0",  algo: 'cosmos' },
  { label: 'eth    cointype 60  + keccak', path: "m/44'/60'/0'/0/0",  algo: 'eth' },
  { label: 'eth    cointype 118 + keccak', path: "m/44'/118'/0'/0/0", algo: 'eth' },
];

for (const c of candidates) {
  const node = HDKey.fromMasterSeed(seed).derive(c.path);
  if (!node.publicKey || !node.privateKey) { console.log(c.label, '— derive failed'); continue; }
  let addr: string;
  if (c.algo === 'cosmos') {
    const pubCompressed = node.publicKey;
    const r = ripemd160(sha256(pubCompressed));
    addr = bech32Init(r);
  } else {
    // For ETH: need uncompressed pubkey (65 bytes, drop 0x04 prefix → 64 bytes)
    const pubFull = HDKey.fromMasterSeed(seed).derive(c.path).publicKey!;
    // @scure/bip32 returns 33-byte compressed; need to decompress.
    // Use noble/secp256k1 to get uncompressed.
    // (lazy import to keep top-level clean)
    const noble = require('@noble/secp256k1');
    const uncompressed = noble.ProjectivePoint.fromHex(pubFull).toRawBytes(false); // 65 bytes
    const ethBytes = keccak_256(uncompressed.slice(1)).slice(-20);
    addr = bech32Init(ethBytes);
  }
  const match = addr === 'init1xl7d3dhpts7tp8d85ez8f7m6ydn3dvgrljmnxg' ? '  ← MATCH minitiad' : '';
  const matchInput = addr === writtenAddr ? '  ← matches our launch_config' : '';
  console.log(`${c.label.padEnd(34)} ${addr}${match}${matchInput}`);
}
