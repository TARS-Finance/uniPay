// Generate a fresh Solana ed25519 keypair and write:
//   - keypair.json     : 64-byte array (Solana CLI format)  [gitignored]
//   - .env             : SOLANA_PRIVATE_KEY=<base58>         [gitignored]
// Prints ONLY the public key (safe). The secret is never printed.
//
// Uses Node's built-in crypto — no npm deps required.

import { generateKeyPairSync, createPrivateKey } from 'node:crypto';
import { writeFileSync, existsSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(__dirname, '..');

// Base58 (Bitcoin alphabet — same as Solana)
const ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz';
function base58encode(bytes) {
  let num = 0n;
  for (const b of bytes) num = (num << 8n) | BigInt(b);
  let out = '';
  while (num > 0n) { out = ALPHABET[Number(num % 58n)] + out; num /= 58n; }
  for (const b of bytes) { if (b === 0) out = '1' + out; else break; }
  return out;
}

const { publicKey, privateKey } = generateKeyPairSync('ed25519');

// Extract raw 32-byte private seed from DER. Node's ed25519 PKCS#8 DER is:
//   30 2e 02 01 00 30 05 06 03 2b 65 70 04 22 04 20 <32-byte seed>
const privDer = privateKey.export({ format: 'der', type: 'pkcs8' });
const seed = privDer.subarray(privDer.length - 32);

// Extract raw 32-byte public key from SPKI DER:
//   30 2a 30 05 06 03 2b 65 70 03 21 00 <32-byte pubkey>
const pubDer = publicKey.export({ format: 'der', type: 'spki' });
const pub = pubDer.subarray(pubDer.length - 32);

// Solana secretKey = seed (32) || pubkey (32)
const secret64 = Buffer.concat([seed, pub]);

const keypairPath = resolve(ROOT, 'keypair.json');
const envPath = resolve(ROOT, '.env');

if (existsSync(keypairPath) || existsSync(envPath)) {
  console.error('[gen-keypair] refuse to overwrite existing keypair.json or .env — delete first if you really want to regenerate');
  process.exit(1);
}

writeFileSync(keypairPath, JSON.stringify(Array.from(secret64)), { mode: 0o600 });
writeFileSync(
  envPath,
  `# Solana executor filler key. Generated $(date). Do not commit.\nSOLANA_PRIVATE_KEY=${base58encode(secret64)}\n`,
  { mode: 0o600 },
);

console.log(`wrote ${keypairPath}`);
console.log(`wrote ${envPath}`);
console.log(`PUBLIC KEY: ${base58encode(pub)}`);
console.log('Fund this address on devnet before running the executor.');
