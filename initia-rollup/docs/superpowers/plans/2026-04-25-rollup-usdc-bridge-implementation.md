# Rollup USDC Bridge for Auto-Earn — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Launch a fresh Initia EVM rollup, bridge `uusdc` from L1 testnet to spawn the canonical ERC20 wrapper on the rollup, redeploy the existing `HTLC.sol` to use that ERC20 as its underlying token, and add an Earn page to `initia-fe` so a merchant can click "Bridge to L1 & Earn" — the existing `stacker` backend (configured with the merchant key) auto-stakes once funds land on L1, and the FE shows the staked principal + yield.

**Architecture:** Three existing components (`HTLC.sol`, `initia-fe`, `stacker`) treated as black boxes wherever possible. New `Tars/initia-rollup/` workspace owns rollup launch config, post-launch discovery (writes the spawned ERC20 address into FE env files), and HTLC redeploy. FE additions are scoped to a new `EarnPage` plus an additive `customChains={[customChain]}` fix in `main.tsx`. Stacker stays unchanged; it's pointed at the merchant key via `.env`.

**Tech Stack:** `weave`, `minitiad`, `initiad` (Initia tooling); TypeScript + `tsx` + `viem` + `@initia/initia.js` for scripts; React 19 + Vite + `@initia/interwovenkit-react@^2.8.0` + `wagmi` for FE.

---

## File Structure

**New (`Tars/initia-rollup/`):**
- `package.json` — scripts wrapper, deps: `tsx`, `viem`, `@initia/initia.js`, `dotenv`.
- `tsconfig.json` — strict TS for the scripts.
- `weave/launch_config.json` — rollup launch config (testnet L1, EVM VM, 5min finalization).
- `scripts/preflight.sh` — verify weave/minitiad/initiad installed.
- `scripts/install-tools.sh` — reinstall weave + minitiad + initiad (we nuked them earlier).
- `scripts/launch-rollup.sh` — wrapper around `weave rollup launch`.
- `scripts/seed-bridge-deposit.ts` — submits first L1→rollup `uusdc` deposit; waits for it to land; queries `denom-erc20` and writes addresses into `.env` files.
- `scripts/redeploy-htlc.ts` — deploys `HTLC.sol` against the new rollup with the spawned USDC ERC20 address.
- `scripts/demo-flow.ts` — E2E smoke for the demo video.
- `.env.example`
- `README.md`

**Modify (`Tars/initia-fe/`):**
- `src/main.tsx` — add `customChains={[customChain]}` (skill rule).
- `src/types.ts` — extend `MerchantPage` to include `'earn'`.
- `src/lib/config.ts` — add `STACKER_API`, `USDC_ERC20_ADDRESS`, `HTLC_ADDRESS`.
- `src/lib/usdc.ts` (new) — `useRollupUsdcBalance` hook.
- `src/lib/stacker.ts` (new) — `useMerchantPosition` hook.
- `src/components/merchant/EarnPanel.tsx` (new) — UI: rollup balance + bridge button + L1 staked rows.
- `src/components/merchant/MerchantView.tsx` — render `EarnPanel` when `page === 'earn'`.
- `src/components/shell/Sidebar.tsx` — add "Earn" link.
- `.env.local` (new) — runtime env (written by `seed-bridge-deposit.ts`).

**Modify (`Tars/stacker/`):**
- `.env` — set merchant `KEEPER_ADDRESS`, `KEEPER_PRIVATE_KEY`, `TARGET_POOL_ID`, `KEEPER_MODE=live`. **No code changes.**

**Unchanged contracts (`Tars/initia/contracts/initia/HTLC.sol`):** Solidity is untouched; only the deploy script's `_token` arg changes.

---

## Chunk 1: Workspace Bootstrap & Tooling

### Phase 0: initia-rollup workspace

**Purpose:** Stand up the new `Tars/initia-rollup/` folder as a TypeScript workspace so all scripts share one `tsconfig` + deps.

**Review gate:** Folder shape exists; `npm run preflight` succeeds.

### Task 0.1: Create package.json + tsconfig

**Files:**
- Create: `Tars/initia-rollup/package.json`
- Create: `Tars/initia-rollup/tsconfig.json`

- [ ] **Step 1: Write `package.json`**

```json
{
  "name": "initia-rollup",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "preflight": "bash scripts/preflight.sh",
    "install-tools": "bash scripts/install-tools.sh",
    "launch": "bash scripts/launch-rollup.sh",
    "seed-bridge": "tsx scripts/seed-bridge-deposit.ts",
    "redeploy-htlc": "tsx scripts/redeploy-htlc.ts",
    "demo": "tsx scripts/demo-flow.ts"
  },
  "dependencies": {
    "@initia/initia.js": "^1.1.0",
    "dotenv": "^16.4.5",
    "viem": "^2.21.0"
  },
  "devDependencies": {
    "@types/node": "^22.0.0",
    "tsx": "^4.20.0",
    "typescript": "^5.5.0"
  }
}
```

- [ ] **Step 2: Write `tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ESNext",
    "moduleResolution": "Bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "resolveJsonModule": true,
    "allowImportingTsExtensions": false,
    "noEmit": true,
    "types": ["node"]
  },
  "include": ["scripts/**/*.ts"]
}
```

- [ ] **Step 3: Install deps**

Run: `cd /Users/diwakarmatsaa/Desktop/Tars/initia-rollup && npm install`
Expected: `node_modules/` populated, no peer-dep warnings about react.

- [ ] **Step 4: Commit**

```bash
git add Tars/initia-rollup/package.json Tars/initia-rollup/tsconfig.json
git commit -m "feat(rollup): bootstrap initia-rollup workspace"
```

### Task 0.2: Create .env.example + README

**Files:**
- Create: `Tars/initia-rollup/.env.example`
- Create: `Tars/initia-rollup/README.md`

- [ ] **Step 1: Write `.env.example`**

```env
# Merchant key — same key is used for L1 (init1...) and rollup (0x...)
MERCHANT_PRIVATE_KEY=0x_replace_with_64_hex_chars
MERCHANT_INIT_ADDRESS=init1replace
MERCHANT_HEX_ADDRESS=0xreplace

# L1 testnet
L1_CHAIN_ID=initiation-2
L1_RPC_URL=https://rpc.testnet.initia.xyz:443
L1_REST_URL=https://rest.testnet.initia.xyz
L1_GAS_PRICES=0.015uinit

# Rollup (filled in after `npm run launch`)
ROLLUP_CHAIN_ID=tars-1
ROLLUP_RPC_URL=http://localhost:26657
ROLLUP_REST_URL=http://localhost:1317
ROLLUP_JSON_RPC_URL=http://localhost:8545
ROLLUP_NATIVE_DENOM=utars

# Filled in after `npm run seed-bridge`
USDC_ERC20_ADDRESS=0x_filled_after_seed
USDC_ROLLUP_DENOM=l2/_filled_after_seed

# Filled in after `npm run redeploy-htlc`
HTLC_ADDRESS=0x_filled_after_redeploy

# Stacker
STACKER_API_URL=http://localhost:3000
TARGET_POOL_ID=replace_pool_id
```

- [ ] **Step 2: Write `README.md`**

```markdown
# initia-rollup

Tooling for the Tars hackathon rollup: launch, seed the USDC bridge, redeploy HTLC, run E2E demo.

## Order of operations

1. `npm run install-tools` — installs `weave`, `minitiad`, `initiad` (one-time).
2. `npm run preflight` — verifies tools.
3. Edit `.env` with merchant key and L1 endpoints.
4. `npm run launch` — launches the rollup; writes its endpoints back into `.env`.
5. `npm run seed-bridge` — seeds first uusdc deposit; captures ERC20 address.
6. `npm run redeploy-htlc` — deploys HTLC.sol with spawned USDC ERC20.
7. Update `Tars/initia-fe/.env.local` (auto-written by `seed-bridge` and `redeploy-htlc`).
8. Configure `Tars/stacker/.env` with merchant key + pool id.
9. `npm run demo` — record the demo video while this runs.

See `docs/superpowers/specs/2026-04-25-rollup-usdc-bridge-design.md` for the design.
```

- [ ] **Step 3: Commit**

```bash
git add Tars/initia-rollup/.env.example Tars/initia-rollup/README.md
git commit -m "docs(rollup): env template and operator README"
```

---

## Chunk 2: Rollup Launch

### Phase 1: Reinstall tools and verify preflight

**Purpose:** We nuked `weave`/`minitiad`/`initiad` earlier in this session. Reinstall them so launch is possible.

**Review gate:** `weave version`, `minitiad version`, `initiad version` all print non-empty values.

### Task 1.1: install-tools.sh

**Files:**
- Create: `Tars/initia-rollup/scripts/install-tools.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
set -euo pipefail

echo ">> Installing weave (Homebrew tap)…"
brew tap initia-labs/initia 2>/dev/null || true
brew install weave

echo ">> Installing initiad (L1)…"
git clone --depth 1 https://github.com/initia-labs/initia.git /tmp/initia
( cd /tmp/initia && make install )
rm -rf /tmp/initia

echo ">> Installing minitiad (minievm)…"
git clone --depth 1 https://github.com/initia-labs/minievm.git /tmp/minievm
( cd /tmp/minievm && make install )
rm -rf /tmp/minievm

echo ">> Done. Verify with: weave version && initiad version && minitiad version"
```

- [ ] **Step 2: Make executable**

Run: `chmod +x Tars/initia-rollup/scripts/install-tools.sh`

- [ ] **Step 3: Run it**

Run: `cd /Users/diwakarmatsaa/Desktop/Tars/initia-rollup && npm run install-tools`
Expected: takes 5–15 min; ends with version-verify hint.

- [ ] **Step 4: Verify versions**

Run: `weave version && initiad version && minitiad version --long | rg '^name:' | head -1`
Expected:
- `weave` prints e.g. `0.3.x`
- `initiad` prints a semver
- `minitiad` `name:` line says `minievm`

- [ ] **Step 5: Commit**

```bash
git add Tars/initia-rollup/scripts/install-tools.sh
git commit -m "chore(rollup): tool installer for weave/minitiad/initiad"
```

### Task 1.2: preflight.sh

**Files:**
- Create: `Tars/initia-rollup/scripts/preflight.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
set -euo pipefail

ok() { echo "  ✓ $1"; }
fail() { echo "  ✗ $1"; exit 1; }

echo ">> Preflight"

command -v weave    >/dev/null && ok "weave"    || fail "missing weave"
command -v initiad  >/dev/null && ok "initiad"  || fail "missing initiad"
command -v minitiad >/dev/null && ok "minitiad" || fail "missing minitiad"
command -v jq       >/dev/null && ok "jq"       || fail "missing jq (brew install jq)"
command -v curl     >/dev/null && ok "curl"     || fail "missing curl"

# Confirm minitiad is the minievm flavor
minitiad version --long 2>/dev/null | grep -q '^name: minievm' \
  && ok "minitiad is minievm" \
  || fail "minitiad is NOT minievm (rebuild from initia-labs/minievm)"

echo ">> All preflight checks passed."
```

- [ ] **Step 2: Make executable + run**

Run: `chmod +x Tars/initia-rollup/scripts/preflight.sh && npm --prefix Tars/initia-rollup run preflight`
Expected: All checks ✓, exit 0.

- [ ] **Step 3: Commit**

```bash
git add Tars/initia-rollup/scripts/preflight.sh
git commit -m "chore(rollup): preflight check script"
```

### Phase 2: Gas station + launch config

**Purpose:** Set up the L1 gas-paying account `weave` uses to fund system keys, then write a launch config with short OPInit finalization for the demo.

### Task 2.1: Set up gas station

**Files:** None (this writes to `~/.weave/config.json` via interactive prompt).

- [ ] **Step 1: Create gas station**

Run: `weave gas-station setup`
Expected: prompts for mnemonic; you accept the generated one.

- [ ] **Step 2: Confirm**

Run: `weave gas-station show`
Expected: prints the gas-station L1 address (`init1...`).

- [ ] **Step 3: Fund the gas station from testnet faucet**

Tell the user to fund the printed `init1...` address with **at least 100 INIT** + **a small amount of `uusdc`** (we'll use uusdc later for the seed deposit) from the Initia testnet faucet (https://faucet.testnet.initia.xyz or community Discord faucet).

Verify funding:
```bash
initiad query bank balances <gas-station-address> --node https://rpc.testnet.initia.xyz:443 -o json | jq
```
Expected: balances list contains `uinit` ≥ 100000000 (100 INIT in 6-decimal units) and `uusdc` ≥ 1000000 (1 USDC).

### Task 2.2: Write launch_config.json

**Files:**
- Create: `Tars/initia-rollup/weave/launch_config.json`

- [ ] **Step 1: Generate system keys**

The skill ships `scripts/generate-system-keys.py` for this.

Run:
```bash
python3 ~/.claude/skills/initia-appchain-dev/scripts/generate-system-keys.py \
  --vm evm --include-mnemonics \
  --output Tars/initia-rollup/weave/system-keys.json
```
Expected: writes `system-keys.json` with mnemonics + addresses for `validator`, `bridge_executor`, `output_submitter`, `batch_submitter`, `challenger`.

- [ ] **Step 2: Add system-keys.json to gitignore**

Edit `Tars/initia-rollup/.gitignore` (create if missing) and add:
```
weave/system-keys.json
.env
.env.local
node_modules/
```

- [ ] **Step 3: Compose launch_config.json**

Create the file by reading `system-keys.json` and embedding the keys' `l1_address`, `l2_address`, `mnemonic` fields, plus the merchant init1... in `genesis_accounts`. Write it as:

```json
{
  "l1_config": {
    "chain_id": "initiation-2",
    "rpc_url": "https://rpc.testnet.initia.xyz:443",
    "gas_prices": "0.015uinit"
  },
  "l2_config": {
    "chain_id": "tars-1",
    "denom": "utars",
    "moniker": "tars-operator"
  },
  "op_bridge": {
    "output_submission_interval": "1m",
    "output_finalization_period": "5m",
    "output_submission_start_height": 1,
    "batch_submission_target": "INITIA",
    "enable_oracle": false
  },
  "system_keys": {
    "validator":         { "l1_address": "<from system-keys.json>", "l2_address": "<...>", "mnemonic": "<...>" },
    "bridge_executor":   { "l1_address": "<...>", "l2_address": "<...>", "mnemonic": "<...>" },
    "output_submitter":  { "l1_address": "<...>", "l2_address": "<...>", "mnemonic": "<...>" },
    "batch_submitter":   { "da_address": "<init1...>", "mnemonic": "<...>" },
    "challenger":        { "l1_address": "<...>", "l2_address": "<...>", "mnemonic": "<...>" }
  },
  "genesis_accounts": [
    { "address": "<MERCHANT_INIT_ADDRESS from .env>", "coins": "100000000000000000000utars" }
  ]
}
```

Use this jq-based fill-in to materialize the placeholders programmatically:

```bash
SYS=Tars/initia-rollup/weave/system-keys.json
MERCHANT=$(grep -E '^MERCHANT_INIT_ADDRESS=' Tars/initia-rollup/.env | cut -d= -f2)

jq -n --slurpfile s "$SYS" --arg merchant "$MERCHANT" '
{
  l1_config: { chain_id: "initiation-2", rpc_url: "https://rpc.testnet.initia.xyz:443", gas_prices: "0.015uinit" },
  l2_config: { chain_id: "tars-1", denom: "utars", moniker: "tars-operator" },
  op_bridge: {
    output_submission_interval: "1m",
    output_finalization_period: "5m",
    output_submission_start_height: 1,
    batch_submission_target: "INITIA",
    enable_oracle: false
  },
  system_keys: $s[0].system_keys,
  genesis_accounts: [ { address: $merchant, coins: "100000000000000000000utars" } ]
}' > Tars/initia-rollup/weave/launch_config.json
```

- [ ] **Step 4: Validate the config**

Run: `jq . Tars/initia-rollup/weave/launch_config.json | head -40`
Expected: all five system_keys entries present with non-empty `mnemonic` and addresses; `genesis_accounts[0].address` matches your merchant init1.

- [ ] **Step 5: Commit (config only, NOT system-keys.json)**

```bash
git add Tars/initia-rollup/weave/launch_config.json Tars/initia-rollup/.gitignore
git commit -m "chore(rollup): launch config (5min finalization, tars-1 chain id)"
```

### Phase 3: Launch rollup + opinit bots

### Task 3.1: launch-rollup.sh

**Files:**
- Create: `Tars/initia-rollup/scripts/launch-rollup.sh`

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
set -euo pipefail

CONFIG="$(dirname "$0")/../weave/launch_config.json"
[ -f "$CONFIG" ] || { echo "missing $CONFIG"; exit 1; }

# Copy launch_config to where weave expects it
cp "$CONFIG" ~/.weave/launch_config.json

echo ">> Launching rollup (vm=evm) using $CONFIG"
weave rollup launch --with-config ~/.weave/launch_config.json --vm evm

echo ">> Starting rollup daemon…"
weave rollup start -d

sleep 5
weave rollup log -n 30
```

- [ ] **Step 2: Make executable + run**

Run: `chmod +x Tars/initia-rollup/scripts/launch-rollup.sh && npm --prefix Tars/initia-rollup run launch`
Expected: weave does interactive prompts (accept defaults), then daemon starts; log shows "Block committed" lines.

- [ ] **Step 3: Verify rollup health**

Run: `curl -s http://localhost:26657/status | jq -r '.result.sync_info.latest_block_height'`
Expected: a positive number that increases on repeated calls.

- [ ] **Step 4: Initialize and start OPInit bots**

```bash
weave opinit init executor
weave opinit start executor -d
weave opinit init challenger
weave opinit start challenger -d
sleep 3
weave opinit log executor | tail -20
```
Expected: executor log shows "starting executor" and no FATAL errors.

- [ ] **Step 5: Capture endpoints back into .env**

Append to `Tars/initia-rollup/.env`:
```bash
{
  echo "ROLLUP_CHAIN_ID=tars-1"
  echo "ROLLUP_RPC_URL=http://localhost:26657"
  echo "ROLLUP_REST_URL=http://localhost:1317"
  echo "ROLLUP_JSON_RPC_URL=http://localhost:8545"
  echo "ROLLUP_NATIVE_DENOM=utars"
  echo "BRIDGE_ID=$(jq -r '.l2_config.bridge_id // empty' ~/.minitia/artifacts/config.json)"
} >> Tars/initia-rollup/.env
```
Expected: `.env` now contains rollup endpoints + a numeric `BRIDGE_ID`.

- [ ] **Step 6: Commit (launch script only)**

```bash
git add Tars/initia-rollup/scripts/launch-rollup.sh
git commit -m "feat(rollup): launch script with opinit bot startup"
```

---

## Chunk 3: Seed Bridge & Capture ERC20

### Phase 4: Seed first deposit + ERC20 discovery

**Purpose:** Trigger the OPInit deposit pathway with `uusdc` from L1 → rollup. The first deposit auto-creates the ERC20 wrapper. Then query its address and write it everywhere we need it.

**Review gate:** `minitiad query evm denom-erc20 uusdc -o json | jq -r .erc20_address` returns a non-empty `0x...` address; written into `Tars/initia-fe/.env.local`.

### Task 4.1: seed-bridge-deposit.ts

**Files:**
- Create: `Tars/initia-rollup/scripts/seed-bridge-deposit.ts`

- [ ] **Step 1: Write the failing skeleton (driver loop)**

The script must: load env, sign a `MsgInitiateTokenDeposit` on L1 with the merchant private key, broadcast, poll the rollup until the bridged balance shows up, then run `denom-erc20` and write the result.

```ts
import 'dotenv/config';
import { execSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';

const ENV_PATH = resolve(process.cwd(), '.env');
const FE_ENV_PATH = resolve(process.cwd(), '../initia-fe/.env.local');

function need(key: string): string {
  const v = process.env[key];
  if (!v) throw new Error(`missing env ${key}`);
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

async function main() {
  const merchantInit = need('MERCHANT_INIT_ADDRESS');
  const merchantHex = need('MERCHANT_HEX_ADDRESS');
  const bridgeId = need('BRIDGE_ID');
  const l1Rpc = need('L1_RPC_URL');
  const l1GasPrices = need('L1_GAS_PRICES');

  // 1. Initiate deposit on L1: 1 uusdc
  console.log('>> Submitting MsgInitiateTokenDeposit on L1 (1 USDC)…');
  const txCmd = [
    'initiad', 'tx', 'opinit', 'initiate-token-deposit',
    bridgeId,
    merchantHex,                       // recipient on L2 (hex)
    '1000000uusdc',                    // 1 USDC at 6 decimals
    '--from', 'merchant',
    '--keyring-backend', 'test',
    '--node', l1Rpc,
    '--gas-prices', l1GasPrices,
    '--gas', 'auto', '--gas-adjustment', '1.6',
    '--chain-id', 'initiation-2',
    '-y', '-o', 'json',
  ].join(' ');
  const txOut = execSync(txCmd, { stdio: ['ignore', 'pipe', 'inherit'] }).toString();
  const txhash = JSON.parse(txOut).txhash;
  console.log(`   txhash=${txhash}`);

  // 2. Wait for the executor to relay; poll denom-erc20 for up to 5 min.
  console.log('>> Waiting for ERC20 spawn (up to 5 min)…');
  const start = Date.now();
  let erc20 = '';
  while (Date.now() - start < 5 * 60_000) {
    try {
      const out = execSync(
        `minitiad query evm denom-erc20 uusdc --node http://localhost:26657 -o json`,
        { stdio: ['ignore', 'pipe', 'pipe'] },
      ).toString();
      const j = JSON.parse(out);
      if (j.erc20_address && j.erc20_address.startsWith('0x')) {
        erc20 = j.erc20_address.toLowerCase();
        break;
      }
    } catch { /* not yet */ }
    process.stdout.write('.');
    await new Promise(r => setTimeout(r, 5_000));
  }
  if (!erc20) throw new Error('ERC20 wrapper did not spawn within 5min');
  console.log(`\n>> Spawned ERC20: ${erc20}`);

  // 3. Capture rollup-side denom too (for openBridge srcDenom)
  const rollupDenomOut = execSync(
    `minitiad query evm erc20-denom ${erc20} --node http://localhost:26657 -o json`,
    { stdio: ['ignore', 'pipe', 'pipe'] },
  ).toString();
  const rollupDenom = JSON.parse(rollupDenomOut).denom as string;
  console.log(`>> Rollup-side denom: ${rollupDenom}`);

  // 4. Write into .env files
  appendEnv(ENV_PATH, {
    USDC_ERC20_ADDRESS: erc20,
    USDC_ROLLUP_DENOM: rollupDenom,
  });
  appendEnv(FE_ENV_PATH, {
    VITE_USDC_ERC20: erc20,
    VITE_USDC_ROLLUP_DENOM: rollupDenom,
    VITE_CHAIN_ID: 'tars-1',
    VITE_COSMOS_RPC: 'http://localhost:26657',
    VITE_COSMOS_REST: 'http://localhost:1317',
    VITE_JSON_RPC_URL: 'http://localhost:8545',
    VITE_INDEXER_URL: 'http://localhost:8080',
  });
  console.log('>> Wrote env files. Done.');
}

main().catch(err => { console.error(err); process.exit(1); });
```

- [ ] **Step 2: Import merchant key into initiad keyring**

Run:
```bash
echo "$MERCHANT_PRIVATE_KEY" | initiad keys import-hex merchant - \
  --keyring-backend test --coin-type 60 --key-type eth_secp256k1
```
(Take MERCHANT_PRIVATE_KEY from `Tars/initia-rollup/.env`.)
Expected: `merchant` key now in `initiad keys list --keyring-backend test`.

- [ ] **Step 3: Run the seed**

Run: `cd /Users/diwakarmatsaa/Desktop/Tars/initia-rollup && npm run seed-bridge`
Expected: prints L1 txhash, then dots while polling, then "Spawned ERC20: 0x..." and "Wrote env files. Done."

- [ ] **Step 4: Verify the writes**

Run: `grep -E '^(USDC_ERC20_ADDRESS|USDC_ROLLUP_DENOM)=' Tars/initia-rollup/.env && grep -E '^VITE_USDC' Tars/initia-fe/.env.local`
Expected: both files contain the new values; ERC20 starts with `0x` and is 42 chars.

- [ ] **Step 5: Commit (script only — env files stay gitignored)**

```bash
git add Tars/initia-rollup/scripts/seed-bridge-deposit.ts
git commit -m "feat(rollup): seed first uusdc deposit and capture ERC20 wrapper"
```

---

## Chunk 4: HTLC Redeploy

### Phase 5: Replace fake-token HTLC with bridged-USDC HTLC

**Purpose:** The HTLC contract is a generic ERC20-token-locking HTLC. Re-deploy it on the new rollup with the freshly-spawned USDC wrapper as `_token`.

**Review gate:** `viem` reads `htlc.token()` and gets the bridged USDC ERC20 address.

### Task 5.1: redeploy-htlc.ts

**Files:**
- Create: `Tars/initia-rollup/scripts/redeploy-htlc.ts`

- [ ] **Step 1: Compile the contract via Foundry (one-shot)**

Run from `Tars/initia/`:
```bash
forge build --root /Users/diwakarmatsaa/Desktop/Tars/initia
```
Expected: `out/HTLC.sol/HTLC.json` written.

- [ ] **Step 2: Write the deploy script**

```ts
import 'dotenv/config';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { createPublicClient, createWalletClient, http, defineChain, getContract } from 'viem';
import { privateKeyToAccount } from 'viem/accounts';

const ARTIFACT = resolve(process.cwd(), '../initia/out/HTLC.sol/HTLC.json');
const FE_ENV   = resolve(process.cwd(), '../initia-fe/.env.local');

function need(k: string) { const v = process.env[k]; if (!v) throw new Error(`missing ${k}`); return v; }

function appendEnv(path: string, lines: Record<string, string>) {
  const existing = existsSync(path) ? readFileSync(path, 'utf8') : '';
  const filtered = existing
    .split('\n')
    .filter(l => !Object.keys(lines).some(k => l.startsWith(`${k}=`)))
    .join('\n');
  const block = Object.entries(lines).map(([k, v]) => `${k}=${v}`).join('\n');
  writeFileSync(path, `${filtered.trim()}\n${block}\n`);
}

async function main() {
  const pk        = need('MERCHANT_PRIVATE_KEY') as `0x${string}`;
  const usdc      = need('USDC_ERC20_ADDRESS')  as `0x${string}`;
  const jsonRpc   = need('ROLLUP_JSON_RPC_URL');
  const chainIdStr= need('ROLLUP_CHAIN_ID');

  const artifact = JSON.parse(readFileSync(ARTIFACT, 'utf8'));
  const abi      = artifact.abi;
  const bytecode = artifact.bytecode.object as `0x${string}`;

  // Cosmos chain id is `tars-1`; numeric EVM chain id is what viem needs. Fetch via JSON-RPC.
  const rpcRes = await fetch(jsonRpc, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method: 'eth_chainId', params: [], id: 1 }),
  });
  const { result: evmChainIdHex } = (await rpcRes.json()) as { result: string };
  const evmChainId = Number(evmChainIdHex);
  console.log(`>> rollup EVM chain id = ${evmChainId} (cosmos id ${chainIdStr})`);

  const chain = defineChain({
    id: evmChainId,
    name: chainIdStr,
    nativeCurrency: { name: 'UTARS', symbol: 'UTARS', decimals: 18 },
    rpcUrls: { default: { http: [jsonRpc] } },
  });

  const account = privateKeyToAccount(pk);
  const wallet  = createWalletClient({ account, chain, transport: http(jsonRpc) });
  const pub     = createPublicClient({ chain, transport: http(jsonRpc) });

  console.log(`>> deploying HTLC(token=${usdc}) from ${account.address}…`);
  const hash = await wallet.deployContract({ abi, bytecode, args: [usdc] });
  const rcpt = await pub.waitForTransactionReceipt({ hash });
  if (!rcpt.contractAddress) throw new Error('no contractAddress in receipt');
  const htlc = rcpt.contractAddress.toLowerCase() as `0x${string}`;
  console.log(`>> HTLC deployed at ${htlc}`);

  // Sanity: read token() — must equal usdc
  const c = getContract({ address: htlc, abi, client: pub });
  const t = (await c.read.token([])) as string;
  if (t.toLowerCase() !== usdc.toLowerCase()) throw new Error(`HTLC.token() mismatch: ${t}`);
  console.log(`>> verified HTLC.token() == ${usdc}`);

  appendEnv(FE_ENV, { VITE_HTLC_ADDRESS: htlc });
  appendEnv(resolve(process.cwd(), '.env'), { HTLC_ADDRESS: htlc });
  console.log('>> wrote VITE_HTLC_ADDRESS to FE env.');
}

main().catch(e => { console.error(e); process.exit(1); });
```

- [ ] **Step 3: Run it**

Run: `cd /Users/diwakarmatsaa/Desktop/Tars/initia-rollup && npm run redeploy-htlc`
Expected:
```
>> rollup EVM chain id = ...
>> deploying HTLC(token=0x...)
>> HTLC deployed at 0x...
>> verified HTLC.token() == 0x...
>> wrote VITE_HTLC_ADDRESS to FE env.
```

- [ ] **Step 4: Commit**

```bash
git add Tars/initia-rollup/scripts/redeploy-htlc.ts
git commit -m "feat(rollup): redeploy HTLC.sol with bridged USDC ERC20 as underlying"
```

---

## Chunk 5: Frontend Earn Page

### Phase 6: Provider fix + types

### Task 6.1: Add `customChains={[customChain]}` and `'earn'` page

**Files:**
- Modify: `Tars/initia-fe/src/main.tsx`
- Modify: `Tars/initia-fe/src/types.ts`

- [ ] **Step 1: Patch main.tsx**

In `Tars/initia-fe/src/main.tsx`, change the `InterwovenKitProvider` element:

```tsx
<InterwovenKitProvider
  {...TESTNET}
  defaultChainId="tars-1"
  customChain={customChain}
  customChains={[customChain]}
>
```

- [ ] **Step 2: Extend MerchantPage in types.ts**

Change:
```ts
export type MerchantPage = 'overview' | 'pools' | 'activity';
```
to:
```ts
export type MerchantPage = 'overview' | 'pools' | 'activity' | 'earn';
```

- [ ] **Step 3: Restart dev server (skill rule for env+config changes)**

Run: `cd Tars/initia-fe && npm run dev`
Expected: starts on http://localhost:5173 with no provider errors. Connect wallet — `tars-1` resolves without "Chain not found".

- [ ] **Step 4: Commit**

```bash
git add Tars/initia-fe/src/main.tsx Tars/initia-fe/src/types.ts
git commit -m "fix(fe): pass customChains array; add 'earn' merchant page"
```

### Phase 7: Rollup balance hook (TDD)

### Task 7.1: useRollupUsdcBalance

**Files:**
- Create: `Tars/initia-fe/src/lib/usdc.ts`
- Create: `Tars/initia-fe/src/lib/usdc.test.ts`

- [ ] **Step 1: Write the failing test**

```ts
import { describe, it, expect, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useRollupUsdcBalance } from './usdc';

vi.mock('wagmi', () => ({
  useReadContract: ({ args }: { args: readonly [`0x${string}`] }) => ({
    data: args[0] === '0x1111111111111111111111111111111111111111' ? 5_000_000n : 0n,
    isLoading: false,
    error: null,
    refetch: vi.fn(),
  }),
}));

describe('useRollupUsdcBalance', () => {
  it('returns balance for the supplied address', () => {
    const { result } = renderHook(() =>
      useRollupUsdcBalance('0x1111111111111111111111111111111111111111'),
    );
    expect(result.current.balance).toBe(5_000_000n);
    expect(result.current.formatted).toBe('5');
  });

  it('returns 0 for unfunded address', () => {
    const { result } = renderHook(() =>
      useRollupUsdcBalance('0x0000000000000000000000000000000000000000'),
    );
    expect(result.current.balance).toBe(0n);
    expect(result.current.formatted).toBe('0');
  });
});
```

- [ ] **Step 2: Add vitest + RTL to FE if missing**

Run: `cd Tars/initia-fe && npm install --save-dev vitest @testing-library/react @testing-library/jest-dom jsdom`
Then add to `package.json` scripts: `"test": "vitest run"`.

Create `Tars/initia-fe/vitest.setup.ts` with:
```ts
import '@testing-library/jest-dom/vitest';
```

In `vite.config.ts`, add to `defineConfig`:
```ts
test: { environment: 'jsdom', globals: false, setupFiles: ['./vitest.setup.ts'] },
```
And add a triple-slash reference at the top of `vite.config.ts` so TS picks up `vitest`'s `test` field:
```ts
/// <reference types="vitest" />
```

- [ ] **Step 3: Run test — expect FAIL**

Run: `cd Tars/initia-fe && npx vitest run src/lib/usdc.test.ts`
Expected: FAIL with `Cannot find module './usdc'`.

- [ ] **Step 4: Implement `usdc.ts`**

```ts
import { useReadContract } from 'wagmi';
import { formatUnits } from 'viem';

const ERC20_ABI = [
  {
    name: 'balanceOf',
    type: 'function',
    stateMutability: 'view',
    inputs: [{ name: 'account', type: 'address' }],
    outputs: [{ type: 'uint256' }],
  },
] as const;

const USDC = (import.meta.env.VITE_USDC_ERC20 ?? '0x0000000000000000000000000000000000000000') as `0x${string}`;
const DECIMALS = 6;

export function useRollupUsdcBalance(address: `0x${string}` | undefined) {
  const { data, isLoading, error, refetch } = useReadContract({
    address: USDC,
    abi: ERC20_ABI,
    functionName: 'balanceOf',
    args: [address ?? '0x0000000000000000000000000000000000000000'] as const,
    query: { enabled: !!address && USDC !== '0x0000000000000000000000000000000000000000' },
  });
  const balance = (data ?? 0n) as bigint;
  return {
    balance,
    formatted: formatUnits(balance, DECIMALS).replace(/\.?0+$/, '') || '0',
    isLoading,
    error,
    refetch,
  };
}
```

- [ ] **Step 5: Run test — expect PASS**

Run: `npx vitest run src/lib/usdc.test.ts`
Expected: 2 tests passed.

- [ ] **Step 6: Commit**

```bash
git add Tars/initia-fe/src/lib/usdc.ts Tars/initia-fe/src/lib/usdc.test.ts \
        Tars/initia-fe/package.json Tars/initia-fe/vite.config.ts
git commit -m "feat(fe): useRollupUsdcBalance hook with vitest setup"
```

### Phase 8: Stacker position hook (TDD)

### Task 8.1: useMerchantPosition

**Files:**
- Create: `Tars/initia-fe/src/lib/stacker.ts`
- Create: `Tars/initia-fe/src/lib/stacker.test.ts`

- [ ] **Step 1: Add `STACKER_API` to config.ts**

In `Tars/initia-fe/src/lib/config.ts`, add at the top alongside `EXECUTOR_API`:

```ts
export const STACKER_API = import.meta.env.VITE_STACKER_API_URL ?? 'http://localhost:3000';
```

- [ ] **Step 2: Write the failing test**

```ts
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import React from 'react';
import { useMerchantPosition } from './stacker';

const wrapper = ({ children }: { children: React.ReactNode }) => {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
};

beforeEach(() => {
  vi.stubGlobal('fetch', vi.fn());
});

describe('useMerchantPosition', () => {
  it('returns position fields when stacker responds', async () => {
    (fetch as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      ok: true,
      json: async () => ({
        principal_available: '1000000',
        principal_staked: '5000000',
        yield_earned: '12345',
        apy_bps: 2480,
      }),
    });

    const { result } = renderHook(
      () => useMerchantPosition('init1abc'),
      { wrapper },
    );

    await waitFor(() => expect(result.current.data).toBeDefined());
    expect(result.current.data?.principal_staked).toBe('5000000');
    expect(result.current.data?.apy_bps).toBe(2480);
  });

  it('exposes error on non-ok response', async () => {
    (fetch as unknown as ReturnType<typeof vi.fn>).mockResolvedValue({
      ok: false, status: 500, json: async () => ({}),
    });

    const { result } = renderHook(
      () => useMerchantPosition('init1abc'),
      { wrapper },
    );

    await waitFor(() => expect(result.current.error).toBeDefined());
  });
});
```

- [ ] **Step 3: Run test — expect FAIL**

Run: `npx vitest run src/lib/stacker.test.ts`
Expected: FAIL — `Cannot find module './stacker'`.

- [ ] **Step 4: Implement `stacker.ts`**

```ts
import { useQuery } from '@tanstack/react-query';
import { STACKER_API } from './config';

export interface MerchantPosition {
  principal_available: string;
  principal_staked: string;
  yield_earned: string;
  apy_bps: number;
}

async function fetchPosition(merchantId: string): Promise<MerchantPosition> {
  const r = await fetch(`${STACKER_API}/merchants/${merchantId}/balance`);
  if (!r.ok) throw new Error(`stacker returned ${r.status}`);
  return r.json();
}

export function useMerchantPosition(merchantId: string | undefined) {
  return useQuery({
    queryKey: ['stacker', 'merchant-position', merchantId],
    queryFn: () => fetchPosition(merchantId!),
    enabled: !!merchantId,
    refetchInterval: 5_000,
  });
}
```

- [ ] **Step 5: Run test — expect PASS**

Run: `npx vitest run src/lib/stacker.test.ts`
Expected: 2 tests passed.

- [ ] **Step 6: Commit**

```bash
git add Tars/initia-fe/src/lib/stacker.ts Tars/initia-fe/src/lib/stacker.test.ts Tars/initia-fe/src/lib/config.ts
git commit -m "feat(fe): useMerchantPosition hook for stacker /merchants/:id/balance"
```

### Phase 9: EarnPanel UI

### Task 9.1: EarnPanel component (TDD core, then layout)

**Files:**
- Create: `Tars/initia-fe/src/components/merchant/EarnPanel.tsx`
- Create: `Tars/initia-fe/src/components/merchant/EarnPanel.test.tsx`

- [ ] **Step 1: Write failing tests**

```tsx
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { EarnPanel } from './EarnPanel';

const openBridge = vi.fn();

vi.mock('@initia/interwovenkit-react', () => ({
  useInterwovenKit: () => ({
    initiaAddress: 'init1merchant',
    address: '0x1111111111111111111111111111111111111111',
    openBridge,
  }),
}));

vi.mock('../../lib/usdc', () => ({
  useRollupUsdcBalance: (addr: string) => ({
    balance: addr === '0x1111111111111111111111111111111111111111' ? 5_000_000n : 0n,
    formatted: addr === '0x1111111111111111111111111111111111111111' ? '5' : '0',
    isLoading: false,
    error: null,
    refetch: vi.fn(),
  }),
}));

vi.mock('../../lib/stacker', () => ({
  useMerchantPosition: () => ({
    data: { principal_available: '0', principal_staked: '5000000', yield_earned: '12345', apy_bps: 2480 },
    isLoading: false,
    error: null,
  }),
}));

describe('EarnPanel', () => {
  it('shows rollup USDC balance and enabled bridge button when balance > 0', () => {
    render(<EarnPanel />);
    expect(screen.getByText(/Rollup balance/i)).toBeInTheDocument();
    expect(screen.getByText('5 USDC')).toBeInTheDocument();
    const btn = screen.getByRole('button', { name: /Bridge to L1 & Earn/i });
    expect(btn).not.toBeDisabled();
  });

  it('clicking bridge button calls openBridge with rollup → L1 args', () => {
    render(<EarnPanel />);
    fireEvent.click(screen.getByRole('button', { name: /Bridge to L1 & Earn/i }));
    expect(openBridge).toHaveBeenCalledWith({
      srcChainId: 'tars-1',
      srcDenom: expect.any(String),
    });
  });

  it('renders staked principal and yield from stacker', () => {
    render(<EarnPanel />);
    expect(screen.getByText(/Staked on L1/i)).toBeInTheDocument();
    expect(screen.getByText('5 USDC')).toBeInTheDocument(); // staked too
    expect(screen.getByText(/24.80% APY/)).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run test — expect FAIL**

Run: `npx vitest run src/components/merchant/EarnPanel.test.tsx`
Expected: FAIL on missing module.

- [ ] **Step 3: Implement EarnPanel**

```tsx
import { useInterwovenKit } from '@initia/interwovenkit-react';
import { formatUnits } from 'viem';
import { useRollupUsdcBalance } from '../../lib/usdc';
import { useMerchantPosition } from '../../lib/stacker';

const ROLLUP_CHAIN_ID = (import.meta.env.VITE_CHAIN_ID ?? 'tars-1') as string;
const ROLLUP_USDC_DENOM = (import.meta.env.VITE_USDC_ROLLUP_DENOM ?? 'uusdc') as string;
const DECIMALS = 6;

function fmt(microUsdc: string | bigint): string {
  return formatUnits(BigInt(microUsdc), DECIMALS).replace(/\.?0+$/, '') || '0';
}

export function EarnPanel() {
  const { initiaAddress, address, openBridge } = useInterwovenKit();
  const { balance, formatted, isLoading: balLoading } = useRollupUsdcBalance(address as `0x${string}` | undefined);
  const { data: pos, isLoading: posLoading, error: posErr } = useMerchantPosition(initiaAddress);

  const handleBridge = () => {
    if (!initiaAddress) return;
    openBridge({ srcChainId: ROLLUP_CHAIN_ID, srcDenom: ROLLUP_USDC_DENOM });
  };

  const hasRollupUsdc = balance > 0n;
  const apyPct = pos ? (pos.apy_bps / 100).toFixed(2) : '—';

  return (
    <section className="earn-panel" style={{ padding: 24, display: 'grid', gap: 24, maxWidth: 720 }}>
      <header>
        <h2 style={{ margin: 0 }}>Earn</h2>
        <p style={{ opacity: 0.7, marginTop: 4 }}>
          Move settled USDC from the rollup to L1, where it’s auto-staked into the USDC/INIT pool for yield.
        </p>
      </header>

      <div className="card" style={{ padding: 16, border: '1px solid #e2e8f0', borderRadius: 16 }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'baseline' }}>
          <span style={{ opacity: 0.7 }}>Rollup balance</span>
          <strong>{balLoading ? '…' : `${formatted} USDC`}</strong>
        </div>
        <button
          onClick={handleBridge}
          disabled={!hasRollupUsdc}
          style={{ marginTop: 12, padding: '12px 16px', borderRadius: 12, width: '100%' }}
        >
          {hasRollupUsdc ? 'Bridge to L1 & Earn' : 'No USDC to bridge yet'}
        </button>
      </div>

      <div className="card" style={{ padding: 16, border: '1px solid #e2e8f0', borderRadius: 16 }}>
        <h3 style={{ marginTop: 0 }}>Staked on L1</h3>
        {posErr && <p>Stacker offline — check the keeper.</p>}
        {!posErr && (
          <ul style={{ listStyle: 'none', padding: 0, display: 'grid', gap: 8 }}>
            <li>Principal: <strong>{posLoading || !pos ? '…' : `${fmt(pos.principal_staked)} USDC`}</strong></li>
            <li>Available (idle): {posLoading || !pos ? '…' : `${fmt(pos.principal_available)} USDC`}</li>
            <li>Yield earned: {posLoading || !pos ? '…' : `${fmt(pos.yield_earned)} USDC`}</li>
            <li>APY: <strong>{apyPct}% APY</strong></li>
          </ul>
        )}
      </div>
    </section>
  );
}
```

- [ ] **Step 4: Run test — expect PASS**

Run: `npx vitest run src/components/merchant/EarnPanel.test.tsx`
Expected: 3 tests passed.

- [ ] **Step 5: Wire into MerchantView**

Open `Tars/initia-fe/src/components/merchant/MerchantView.tsx` and:
- import `EarnPanel`,
- in the `switch (page)` (or equivalent ladder), add a case `'earn': return <EarnPanel />;`.

(Read the current MerchantView.tsx first — the exact switch shape may differ; just route the `'earn'` page to `<EarnPanel />`.)

- [ ] **Step 6: Add "Earn" link in Sidebar**

Open `Tars/initia-fe/src/components/shell/Sidebar.tsx` and append an "Earn" entry to the merchant nav array — same pattern as existing 'overview' / 'pools' / 'activity' entries.

- [ ] **Step 7: Manual smoke**

Run: `npm run dev` — visit http://localhost:5173, switch to merchant persona, click "Earn".
Expected: panel renders. With merchant key holding USDC on rollup, "Bridge to L1 & Earn" is enabled; clicking opens the InterwovenKit bridge modal preset to `tars-1 → initiation-2`.

- [ ] **Step 8: Commit**

```bash
git add Tars/initia-fe/src/components/merchant/EarnPanel.tsx \
        Tars/initia-fe/src/components/merchant/EarnPanel.test.tsx \
        Tars/initia-fe/src/components/merchant/MerchantView.tsx \
        Tars/initia-fe/src/components/shell/Sidebar.tsx
git commit -m "feat(fe): EarnPanel — rollup balance, bridge button, L1 staked view"
```

---

## Chunk 6: Stacker Wiring & E2E

### Phase 10: Configure stacker for live merchant

**Purpose:** Stacker code is unchanged. Point it at the merchant's L1 key and live pool, then verify it stakes once funds arrive.

### Task 10.1: Configure and run stacker

**Files:**
- Modify: `Tars/stacker/.env` (NOT committed — secrets)

- [ ] **Step 1: Discover the live USDC/INIT pool id on testnet**

Run:
```bash
initiad query move view 0x1 dex pools_by_pair \
  --args "address:0x1::denom::denom_module" \
  --node https://rpc.testnet.initia.xyz:443 -o json
```
Or simpler: ask the user to provide `TARGET_POOL_ID` from the Initia testnet DEX UI (the USDC/INIT pool detail page). Save the value.

- [ ] **Step 2: Write `.env` for stacker**

```env
DATABASE_URL=postgres://stacker:stacker@localhost:5432/stacker
KEEPER_PRIVATE_KEY=<MERCHANT_PRIVATE_KEY>
KEEPER_ADDRESS=<MERCHANT_INIT_ADDRESS>
INITIA_LCD_URL=https://rest.testnet.initia.xyz
INITIA_RPC_URL=https://rpc.testnet.initia.xyz
INITIA_GAS_PRICES=0.015uinit
INITIA_GAS_ADJUSTMENT=1.75
TARGET_POOL_ID=<resolved in step 1>
DEX_MODULE_ADDRESS=0x1
DEX_MODULE_NAME=dex
LOCK_STAKING_MODULE_ADDRESS=0x81c3ea419d2fd3a27971021d9dd3cc708def05e5d6a09d39b2f1f9ba18312264
LOCK_STAKING_MODULE_NAME=lock_staking
LOCKUP_SECONDS=86400
KEEPER_MODE=dry-run
LP_DENOM=ulp
```

- [ ] **Step 3: Bring up stacker locally**

Run from `Tars/stacker/`:
```bash
docker compose up -d                 # postgres
pnpm install
pnpm db:migrate
pnpm --filter ./apps/api dev &       # API on :3000
pnpm --filter ./apps/keeper dev &    # keeper loop
```
Expected: API starts on http://localhost:3000; keeper logs "tick" every interval.

- [ ] **Step 4: Verify the API endpoint works**

Run: `curl -s http://localhost:3000/merchants/$MERCHANT_INIT_ADDRESS/balance | jq`
Expected: JSON with `principal_available`, `principal_staked`, `yield_earned`, `apy_bps` fields. (If the endpoint isn't built yet in stacker, surface this to the user — it's a stacker-side gap that blocks integration; don't proceed until it lands.)

- [ ] **Step 5: Switch keeper to live + verify it stakes**

Set `KEEPER_MODE=live` in `Tars/stacker/.env`, restart keeper.
On L1 testnet, transfer 1 USDC to merchant `init1...` (e.g., from your already-funded gas station):
```bash
initiad tx bank send gas-station <MERCHANT_INIT_ADDRESS> 1000000uusdc \
  --keyring-backend test --node https://rpc.testnet.initia.xyz:443 \
  --gas-prices 0.015uinit --gas auto --gas-adjustment 1.6 \
  --chain-id initiation-2 -y
```
Wait one keeper tick (~30s by default), then re-curl the balance endpoint:
```bash
curl -s http://localhost:3000/merchants/$MERCHANT_INIT_ADDRESS/balance | jq
```
Expected: `principal_staked` increases above 0.

### Phase 11: Demo flow

### Task 11.1: demo-flow.ts

**Files:**
- Create: `Tars/initia-rollup/scripts/demo-flow.ts`

- [ ] **Step 1: Write the script**

```ts
import 'dotenv/config';
import { execSync } from 'node:child_process';
import { createPublicClient, http, defineChain } from 'viem';

function need(k: string) { const v = process.env[k]; if (!v) throw new Error(`missing ${k}`); return v; }

async function main() {
  const merchantHex   = need('MERCHANT_HEX_ADDRESS') as `0x${string}`;
  const merchantInit  = need('MERCHANT_INIT_ADDRESS');
  const usdc          = need('USDC_ERC20_ADDRESS')  as `0x${string}`;
  const jsonRpc       = need('ROLLUP_JSON_RPC_URL');
  const stackerApi    = need('STACKER_API_URL');

  const chainIdHex = await fetch(jsonRpc, {
    method: 'POST', headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ jsonrpc: '2.0', method: 'eth_chainId', params: [], id: 1 }),
  }).then(r => r.json()).then(j => j.result as string);
  const chain = defineChain({
    id: Number(chainIdHex), name: 'rollup',
    nativeCurrency: { name: 'UTARS', symbol: 'UTARS', decimals: 18 },
    rpcUrls: { default: { http: [jsonRpc] } },
  });
  const pub = createPublicClient({ chain, transport: http(jsonRpc) });

  const erc20Abi = [{
    name: 'balanceOf', type: 'function', stateMutability: 'view',
    inputs: [{ name: 'a', type: 'address' }], outputs: [{ type: 'uint256' }],
  }] as const;

  console.log('--- DEMO FLOW: rollup USDC → L1 → stacker stake ---');

  const before = await pub.readContract({ address: usdc, abi: erc20Abi, functionName: 'balanceOf', args: [merchantHex] }) as bigint;
  console.log(`[1/4] rollup USDC balance: ${before}`);
  if (before === 0n) throw new Error('merchant has no rollup USDC; complete an HTLC redeem first');

  console.log(`[2/4] open the FE Earn page and click "Bridge to L1 & Earn".`);
  console.log(`      The bridge widget should pre-fill srcChainId=tars-1, srcDenom=${process.env.USDC_ROLLUP_DENOM}.`);
  console.log(`      Sign in your wallet. Press Enter here once the rollup tx is signed…`);
  await new Promise<void>(r => process.stdin.once('data', () => r()));

  console.log('[3/4] polling L1 balance for arrival (up to 10 min)…');
  const start = Date.now();
  while (Date.now() - start < 10 * 60_000) {
    const out = execSync(
      `initiad query bank balances ${merchantInit} --node https://rpc.testnet.initia.xyz:443 -o json`,
      { stdio: ['ignore', 'pipe', 'pipe'] },
    ).toString();
    const ub = (JSON.parse(out).balances || []).find((b: { denom: string }) => b.denom === 'uusdc');
    if (ub && BigInt(ub.amount) > 0n) {
      console.log(`      → ${ub.amount} uusdc minted on L1.`);
      break;
    }
    process.stdout.write('.');
    await new Promise(r => setTimeout(r, 10_000));
  }

  console.log('[4/4] waiting for stacker to stake (max 90s)…');
  const stakeStart = Date.now();
  while (Date.now() - stakeStart < 90_000) {
    const r = await fetch(`${stackerApi}/merchants/${merchantInit}/balance`);
    if (r.ok) {
      const j = await r.json();
      if (BigInt(j.principal_staked) > 0n) {
        console.log('      → staked:', j);
        return;
      }
    }
    await new Promise(r => setTimeout(r, 5_000));
  }
  console.log('      no stake observed; check stacker logs.');
}

main().catch(e => { console.error(e); process.exit(1); });
```

- [ ] **Step 2: Dry run with real funds**

Run: `npm run demo`
Expected: walks through the four stages and ends with a non-zero `principal_staked`. This is the demo video.

- [ ] **Step 3: Commit**

```bash
git add Tars/initia-rollup/scripts/demo-flow.ts
git commit -m "feat(rollup): end-to-end demo flow script"
```

---

## Final Verification

- [ ] Merchant connects wallet in FE.
- [ ] FE shows rollup USDC balance > 0 from `useRollupUsdcBalance`.
- [ ] Click "Bridge to L1 & Earn" → InterwovenKit bridge widget opens with `tars-1 → initiation-2`.
- [ ] Withdrawal tx hash recorded; ~5min later L1 `uusdc` balance increases.
- [ ] Stacker stakes within one keeper tick.
- [ ] FE Earn panel shows non-zero `principal_staked` and APY.
- [ ] Demo video captured.

## Risk Notes

- **Secrets in `.env`:** never commit `Tars/initia-rollup/.env`, `Tars/initia-rollup/weave/system-keys.json`, or `Tars/stacker/.env`. The `.gitignore` from Task 0.2 covers this for `initia-rollup`.
- **OPInit finalization period:** set to 5 min in launch config — short enough for the demo, much shorter than mainnet defaults. **Do NOT use this config for mainnet.**
- **Single-merchant assumption:** stacker is configured with one merchant key. Multi-merchant support is out of scope.
- **`stacker/merchants/:id/balance` shape:** assumed in Task 8.1; if the live response shape differs, update the `MerchantPosition` interface to match before the FE will render.
