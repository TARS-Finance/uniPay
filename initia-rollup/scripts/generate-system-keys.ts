import { writeFileSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { generateMnemonic, mnemonicToAccount, english } from 'viem/accounts';
import { AccAddress } from '@initia/initia.js';

// Initia EVM rollups use eth_secp256k1 (cointype 60) with Ethereum-style address
// derivation: last 20 bytes of keccak256(uncompressed_pubkey). The bech32 form
// (init1...) is just that 20-byte address re-encoded with prefix "init".
//
// `@initia/initia.js`'s MnemonicKey uses cosmos-style ripemd160(sha256(...))
// even at cointype 60, which produces a DIFFERENT address than minitiad expects.
// We use viem's mnemonicToAccount (eth path m/44'/60'/0'/0/0) and convert via
// AccAddress.fromHex to get the matching init1.
function gen(): { mnemonic: string; address: string } {
  const mnemonic = generateMnemonic(english);
  const acct = mnemonicToAccount(mnemonic);
  const hex = acct.address.toLowerCase();
  const init1 = AccAddress.fromHex(hex);
  return { mnemonic, address: init1 };
}

const validator        = gen();
const bridge_executor  = gen();
const output_submitter = gen();
const batch_submitter  = gen();
const challenger       = gen();

const out = {
  system_keys: {
    validator: {
      l1_address: validator.address,
      l2_address: validator.address,
      mnemonic:   validator.mnemonic,
    },
    bridge_executor: {
      l1_address: bridge_executor.address,
      l2_address: bridge_executor.address,
      mnemonic:   bridge_executor.mnemonic,
    },
    output_submitter: {
      l1_address: output_submitter.address,
      l2_address: output_submitter.address,
      mnemonic:   output_submitter.mnemonic,
    },
    batch_submitter: {
      da_address: batch_submitter.address,
      mnemonic:   batch_submitter.mnemonic,
    },
    challenger: {
      l1_address: challenger.address,
      l2_address: challenger.address,
      mnemonic:   challenger.mnemonic,
    },
  },
};

const path = resolve(process.cwd(), 'weave/system-keys.json');
mkdirSync(dirname(path), { recursive: true });
writeFileSync(path, JSON.stringify(out, null, 2));

console.log(`>> wrote ${path}`);
console.log('   addresses (eth-derived):');
for (const [k, v] of Object.entries(out.system_keys)) {
  const a = 'l1_address' in v ? v.l1_address : v.da_address;
  console.log(`     ${k.padEnd(18)} ${a}`);
}
