import 'dotenv/config';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { privateKeyToAccount } from 'viem/accounts';
import { AccAddress } from '@initia/initia.js';

const ENV_PATH = resolve(process.cwd(), '.env');

function need(k: string): string {
  const v = process.env[k];
  if (!v) throw new Error(`missing ${k} in .env`);
  return v;
}

function appendEnv(path: string, lines: Record<string, string>) {
  const existing = existsSync(path) ? readFileSync(path, 'utf8') : '';
  const filtered = existing
    .split('\n')
    .filter(line => !Object.keys(lines).some(k => line.startsWith(`${k}=`)))
    .join('\n');
  const block = Object.entries(lines).map(([k, v]) => `${k}=${v}`).join('\n');
  writeFileSync(path, `${filtered.trim()}\n${block}\n`);
}

const pk = need('MERCHANT_PRIVATE_KEY') as `0x${string}`;
if (pk.length !== 66) throw new Error(`MERCHANT_PRIVATE_KEY must be 0x + 64 hex chars (got length ${pk.length})`);

const account = privateKeyToAccount(pk);
const hex = account.address.toLowerCase();
const init1 = AccAddress.fromHex(hex);

appendEnv(ENV_PATH, {
  MERCHANT_HEX_ADDRESS: hex,
  MERCHANT_INIT_ADDRESS: init1,
});

console.log('>> derived from MERCHANT_PRIVATE_KEY:');
console.log(`   MERCHANT_HEX_ADDRESS  = ${hex}`);
console.log(`   MERCHANT_INIT_ADDRESS = ${init1}`);
console.log('>> wrote both into .env');
