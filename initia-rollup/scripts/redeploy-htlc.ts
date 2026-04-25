import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import solc from 'solc';
import { createPublicClient, createWalletClient, defineChain, getContract, http } from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { ENV_PATH, FE_ENV_PATH, need, upsertEnv } from './shared.js';

type SolcImportResponse = {
  contents?: string;
  error?: string;
};

type SolcOutput = {
  contracts?: Record<string, Record<string, { abi: unknown[]; evm: { bytecode: { object: string } } }>>;
  errors?: Array<{ severity: 'error' | 'warning'; formattedMessage: string }>;
};

type EthChainIdResponse = {
  result: string;
};

const CONTRACT_PATH = resolve(process.cwd(), '../initia/contracts/initia/HTLC.sol');

function findImports(importPath: string): SolcImportResponse {
  const localPaths = [
    resolve(process.cwd(), '../initia/contracts/initia', importPath),
    resolve(process.cwd(), 'node_modules', importPath),
  ];

  for (const path of localPaths) {
    try {
      return { contents: readFileSync(path, 'utf8') };
    } catch {
      // Try the next candidate.
    }
  }

  return { error: `File not found: ${importPath}` };
}

function compileHtlc() {
  const input = {
    language: 'Solidity',
    sources: {
      'HTLC.sol': {
        content: readFileSync(CONTRACT_PATH, 'utf8'),
      },
    },
    settings: {
      optimizer: {
        enabled: true,
        runs: 200,
      },
      outputSelection: {
        '*': {
          '*': ['abi', 'evm.bytecode.object'],
        },
      },
    },
  };

  const output = JSON.parse(solc.compile(JSON.stringify(input), { import: findImports })) as SolcOutput;
  const errors = output.errors?.filter((entry) => entry.severity === 'error') ?? [];

  if (errors.length > 0) {
    throw new Error(errors.map((entry) => entry.formattedMessage).join('\n\n'));
  }

  const contract = output.contracts?.['HTLC.sol']?.HTLC;

  if (!contract?.evm.bytecode.object) {
    throw new Error('HTLC.sol compilation did not produce deployable bytecode');
  }

  return {
    abi: contract.abi,
    bytecode: `0x${contract.evm.bytecode.object}` as `0x${string}`,
  };
}

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
  const privateKey = need('MERCHANT_PRIVATE_KEY') as `0x${string}`;
  const usdcErc20Address = need('USDC_ERC20_ADDRESS') as `0x${string}`;
  const jsonRpcUrl = need('ROLLUP_JSON_RPC_URL');
  const chainName = need('ROLLUP_CHAIN_ID');
  const { abi, bytecode } = compileHtlc();
  const evmChainId = await readEvmChainId(jsonRpcUrl);

  const chain = defineChain({
    id: evmChainId,
    name: chainName,
    nativeCurrency: { name: 'UTARS', symbol: 'UTARS', decimals: 18 },
    rpcUrls: {
      default: { http: [jsonRpcUrl] },
    },
  });

  const account = privateKeyToAccount(privateKey);
  const publicClient = createPublicClient({ chain, transport: http(jsonRpcUrl) });
  const walletClient = createWalletClient({ account, chain, transport: http(jsonRpcUrl) });

  console.log(`>> deploying HTLC with token ${usdcErc20Address}`);
  const hash = await walletClient.deployContract({
    abi,
    bytecode,
    args: [usdcErc20Address],
    account,
  });

  const receipt = await publicClient.waitForTransactionReceipt({ hash });
  const contractAddress = receipt.contractAddress?.toLowerCase() as `0x${string}` | undefined;

  if (!contractAddress) {
    throw new Error('deployment receipt did not include a contract address');
  }

  const contract = getContract({
    address: contractAddress,
    abi,
    client: publicClient,
  });
  const tokenAddress = (await contract.read.token()) as string;

  if (tokenAddress.toLowerCase() !== usdcErc20Address.toLowerCase()) {
    throw new Error(`HTLC.token() mismatch: expected ${usdcErc20Address}, got ${tokenAddress}`);
  }

  upsertEnv(ENV_PATH, { HTLC_ADDRESS: contractAddress });
  upsertEnv(FE_ENV_PATH, { VITE_HTLC_ADDRESS: contractAddress });

  console.log(`>> deployed HTLC at ${contractAddress}`);
  console.log(`>> verified token() = ${tokenAddress}`);
  console.log('>> wrote HTLC address into initia-rollup/.env and initia-fe/.env.local');
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
