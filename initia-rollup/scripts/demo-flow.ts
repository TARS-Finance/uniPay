import { createPublicClient, defineChain, http } from 'viem';
import { execJson, need, sleep } from './shared.js';

type EthChainIdResponse = {
  result: string;
};

type BankBalanceResponse = {
  balances?: Array<{ denom: string; amount: string }>;
};

type MerchantBalanceResponse = {
  principal_available?: string;
  principal_staked?: string;
  yield_earned?: string;
  apy_bps?: number;
};

async function readEvmChainId(jsonRpcUrl: string) {
  const response = await fetch(jsonRpcUrl, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_chainId',
      params: [],
    }),
  });

  const payload = (await response.json()) as EthChainIdResponse;

  if (!payload.result) {
    throw new Error('eth_chainId did not return a result');
  }

  return Number(payload.result);
}

async function main() {
  const merchantHexAddress = need('MERCHANT_HEX_ADDRESS') as `0x${string}`;
  const merchantInitAddress = need('MERCHANT_INIT_ADDRESS');
  const usdcErc20Address = need('USDC_ERC20_ADDRESS') as `0x${string}`;
  const jsonRpcUrl = need('ROLLUP_JSON_RPC_URL');
  const stackerApiUrl = process.env.STACKER_API_URL ?? 'http://localhost:3000';
  const evmChainId = await readEvmChainId(jsonRpcUrl);

  const chain = defineChain({
    id: evmChainId,
    name: need('ROLLUP_CHAIN_ID'),
    nativeCurrency: { name: 'UTARS', symbol: 'UTARS', decimals: 18 },
    rpcUrls: {
      default: { http: [jsonRpcUrl] },
    },
  });

  const client = createPublicClient({ chain, transport: http(jsonRpcUrl) });
  const erc20Abi = [
    {
      name: 'balanceOf',
      type: 'function',
      stateMutability: 'view',
      inputs: [{ name: 'account', type: 'address' }],
      outputs: [{ type: 'uint256' }],
    },
  ] as const;

  console.log('--- DEMO FLOW ---');
  const rollupBalance = await client.readContract({
    address: usdcErc20Address,
    abi: erc20Abi,
    functionName: 'balanceOf',
    args: [merchantHexAddress],
  });

  console.log(`[1/4] rollup USDC balance: ${rollupBalance}`);

  if (rollupBalance === 0n) {
    throw new Error('merchant has no rollup USDC yet; redeem a swap or seed more funds first');
  }

  console.log(`[2/4] open the FE Earn page and click "Bridge to L1 & Earn"`);
  console.log(`      expected srcChainId=${need('ROLLUP_CHAIN_ID')} srcDenom=${need('USDC_ROLLUP_DENOM')}`);
  console.log('      press Enter here once you have signed the bridge transaction in the wallet');
  await new Promise<void>((resolveInput) => {
    process.stdin.resume();
    process.stdin.once('data', () => resolveInput());
  });

  console.log('[3/4] polling L1 uusdc balance for arrival');
  const l1Start = Date.now();

  while (Date.now() - l1Start < 10 * 60_000) {
    const balances = execJson<BankBalanceResponse>('initiad', [
      'query',
      'bank',
      'balances',
      merchantInitAddress,
      '--node',
      need('L1_RPC_URL'),
      '-o',
      'json',
    ]);
    const uusdc = balances.balances?.find((balance) => balance.denom === 'uusdc');

    if (uusdc && BigInt(uusdc.amount) > 0n) {
      console.log(`      uusdc on L1: ${uusdc.amount}`);
      break;
    }

    process.stdout.write('.');
    await sleep(10_000);
  }

  console.log('\n[4/4] polling stacker balance endpoint');
  const stackerStart = Date.now();

  while (Date.now() - stackerStart < 90_000) {
    const response = await fetch(`${stackerApiUrl}/merchants/${merchantInitAddress}/balance`);

    if (response.ok) {
      const payload = (await response.json()) as MerchantBalanceResponse;

      if (BigInt(payload.principal_staked ?? '0') > 0n) {
        console.log('      staker position:', payload);
        return;
      }
    }

    await sleep(5_000);
  }

  console.log('      no staked position observed yet; check the separate staker branch/API');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
