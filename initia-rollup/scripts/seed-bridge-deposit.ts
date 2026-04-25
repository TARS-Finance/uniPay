import { execJson, execText, FE_ENV_PATH, need, sleep, upsertEnv, ENV_PATH } from './shared.js';

type KeyInfo = {
  name: string;
  address: string;
};

type BroadcastResponse = {
  txhash?: string;
  code?: number;
  raw_log?: string;
  tx_response?: {
    txhash?: string;
    code?: number;
    raw_log?: string;
  };
};

type ContractAddrByDenomResponse = {
  address?: string;
};

type TokenPairByL1DenomResponse = {
  token_pair?: {
    l2_denom?: string;
  };
};

type DenomByContractResponse = {
  denom?: string;
};

function ensureMerchantKey() {
  const privateKey = need('MERCHANT_PRIVATE_KEY').replace(/^0x/, '');
  const keys = execJson<KeyInfo[]>('initiad', ['keys', 'list', '--keyring-backend', 'test', '--output', 'json']);

  if (keys.some((key) => key.name === 'merchant')) {
    return;
  }

  execText('initiad', [
    'keys',
    'import-hex',
    'merchant',
    privateKey,
    '--keyring-backend',
    'test',
    '--key-type',
    'eth_secp256k1',
  ]);
}

function readErc20AddressOnce(bridgeId: string) {
  const rpcUrl = need('ROLLUP_RPC_URL');
  const l1RpcUrl = need('L1_RPC_URL');

  const tokenPair = execJson<TokenPairByL1DenomResponse>('initiad', [
    'query',
    'ophost',
    'token-pair-by-l1-denom',
    '--bridge-id',
    bridgeId,
    '--l1-denom',
    'uusdc',
    '--node',
    l1RpcUrl,
    '-o',
    'json',
  ]);
  const l2Denom = tokenPair.token_pair?.l2_denom;

  if (!l2Denom) {
    return null;
  }

  const response = execJson<ContractAddrByDenomResponse>('minitiad', [
    'query',
    'evm',
    'contract-addr-by-denom',
    l2Denom,
    '--node',
    rpcUrl,
    '-o',
    'json',
  ]);

  if (!response.address?.startsWith('0x')) {
    return null;
  }

  return {
    erc20Address: response.address.toLowerCase(),
    rollupUsdcDenom: l2Denom,
  };
}

async function waitForErc20Address(bridgeId: string) {
  const startedAt = Date.now();

  while (Date.now() - startedAt < 10 * 60_000) {
    try {
      const resolved = readErc20AddressOnce(bridgeId);

      if (resolved) {
        return resolved;
      }
    } catch {
      // Wait for the executor to finalize the first deposit on the child chain.
    }

    process.stdout.write('.');
    await sleep(5_000);
  }

  throw new Error('ERC20 wrapper did not appear on the rollup within 10 minutes');
}

function readRollupDenom(erc20Address: string) {
  const rpcUrl = need('ROLLUP_RPC_URL');
  const response = execJson<DenomByContractResponse>('minitiad', [
    'query',
    'evm',
    'denom',
    erc20Address,
    '--node',
    rpcUrl,
    '-o',
    'json',
  ]);

  if (!response.denom) {
    throw new Error(`missing rollup denom for ERC20 ${erc20Address}`);
  }

  return response.denom;
}

async function main() {
  ensureMerchantKey();

  const merchantInitAddress = need('MERCHANT_INIT_ADDRESS');
  const bridgeId = need('BRIDGE_ID');
  const l1RpcUrl = need('L1_RPC_URL');
  const l1GasPrices = need('L1_GAS_PRICES');
  const rollupChainId = need('ROLLUP_CHAIN_ID');
  const rollupRpcUrl = need('ROLLUP_RPC_URL');
  const rollupRestUrl = need('ROLLUP_REST_URL');
  const rollupJsonRpcUrl = need('ROLLUP_JSON_RPC_URL');
  const stackerApiUrl = process.env.STACKER_API_URL ?? 'http://localhost:3000';

  let resolved = null;

  try {
    resolved = readErc20AddressOnce(bridgeId);
  } catch {
    resolved = null;
  }

  if (!resolved) {
    console.log('>> submitting first L1 -> L2 uusdc deposit from merchant wallet');
    const tx = execJson<BroadcastResponse>('initiad', [
      'tx',
      'ophost',
      'initiate-token-deposit',
      bridgeId,
      merchantInitAddress,
      '1000000uusdc',
      '00',
      '--from',
      'merchant',
      '--keyring-backend',
      'test',
      '--node',
      l1RpcUrl,
      '--gas-prices',
      l1GasPrices,
      '--gas',
      'auto',
      '--gas-adjustment',
      '1.6',
      '--chain-id',
      need('L1_CHAIN_ID'),
      '-y',
      '-o',
      'json',
    ]);

    const txhash = tx.txhash ?? tx.tx_response?.txhash;
    const code = tx.code ?? tx.tx_response?.code ?? 0;

    if (code !== 0) {
      throw new Error(`deposit broadcast failed: ${tx.raw_log ?? tx.tx_response?.raw_log ?? 'unknown error'}`);
    }

    console.log(`   txhash=${txhash ?? 'unknown'}`);
    console.log('>> waiting for the rollup ERC20 wrapper to appear');
    resolved = await waitForErc20Address(bridgeId);
  } else {
    console.log('>> existing uusdc wrapper already present on the rollup; skipping another seed deposit');
  }

  const { erc20Address, rollupUsdcDenom } = resolved;
  const resolvedDenom = readRollupDenom(erc20Address);

  console.log(`\n>> bridged uusdc wrapper = ${erc20Address}`);
  console.log(`>> rollup denom          = ${resolvedDenom}`);

  upsertEnv(ENV_PATH, {
    USDC_ERC20_ADDRESS: erc20Address,
    USDC_ROLLUP_DENOM: resolvedDenom,
  });

  upsertEnv(FE_ENV_PATH, {
    VITE_USDC_ERC20: erc20Address,
    VITE_USDC_ROLLUP_DENOM: resolvedDenom,
    VITE_CHAIN_ID: rollupChainId,
    VITE_COSMOS_RPC: rollupRpcUrl,
    VITE_COSMOS_REST: rollupRestUrl,
    VITE_JSON_RPC_URL: rollupJsonRpcUrl,
    VITE_INDEXER_URL: process.env.VITE_INDEXER_URL ?? 'http://localhost:8080',
    VITE_STACKER_API_URL: stackerApiUrl,
  });

  console.log('>> wrote rollup USDC addresses into initia-rollup/.env and initia-fe/.env.local');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
