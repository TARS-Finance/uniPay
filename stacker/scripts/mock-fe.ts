import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

type MockFeConfig = {
  walletAddress: string;
  strategy: {
    inputDenom: "usdc" | "iusdc" | "uusdc";
    targetPoolId: string;
    validatorAddress: string;
    minBalanceAmount: string;
    maxAmountPerRun: string;
    maxSlippageBps: number;
    cooldownSeconds: number;
  };
};

function readArgument(name: string): string {
  const index = process.argv.indexOf(name);
  const value = process.argv[index + 1];

  if (index === -1 || !value) {
    throw new Error(`Missing required argument: ${name}`);
  }

  return value;
}

async function requestJson<T>(input: string, init?: RequestInit): Promise<T> {
  const response = await fetch(input, init);

  if (!response.ok) {
    throw new Error(
      `Request failed: ${response.status} ${response.statusText} for ${input}`
    );
  }

  return response.json() as Promise<T>;
}

async function loadConfig(path: string): Promise<MockFeConfig> {
  const contents = await readFile(resolve(path), "utf8");
  return JSON.parse(contents) as MockFeConfig;
}

const apiBaseUrl = readArgument("--api-base-url");
const configPath = readArgument("--config");
const config = await loadConfig(configPath);

const registerUser = await requestJson<{
  userId: string;
  initiaAddress: string;
}>(`${apiBaseUrl}/users/register`, {
  method: "POST",
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({
    initiaAddress: config.walletAddress
  })
});

console.log(`created user id: ${registerUser.userId}`);

const createStrategy = await requestJson<{
  strategyId: string;
  status: string;
}>(`${apiBaseUrl}/strategies`, {
  method: "POST",
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({
    userId: registerUser.userId,
    ...config.strategy
  })
});

console.log(`created strategy id: ${createStrategy.strategyId}`);

const prepareGrants = await requestJson<{
  keeperAddress: string;
  grants: {
    move: { "@type": string };
    staking: { "@type": string } | null;
    feegrant: { "@type": string };
  };
}>(`${apiBaseUrl}/grants/prepare`, {
  method: "POST",
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({
    userId: registerUser.userId,
    strategyId: createStrategy.strategyId
  })
});

console.log(`keeper address: ${prepareGrants.keeperAddress}`);
console.log(
  `grant payload summary: ${prepareGrants.grants.move["@type"]}, ${prepareGrants.grants.staking?.["@type"] ?? "not-required"}, ${prepareGrants.grants.feegrant["@type"]}`
);

await requestJson<{
  strategyId: string;
  strategyStatus: string;
}>(`${apiBaseUrl}/grants/confirm`, {
  method: "POST",
  headers: {
    "content-type": "application/json"
  },
  body: JSON.stringify({
    userId: registerUser.userId,
    strategyId: createStrategy.strategyId
  })
});

const strategyStatus = await requestJson<{
  strategyId: string;
  status: string;
  executionMode: "single-asset-provide-delegate";
  balances: {
    input: string;
    lp: string;
    delegatedLp: string;
    delegatedLpKind: "delegated" | "bonded-locked";
  };
  rewardLock: {
    releaseTime: string;
    releaseTimeIso: string;
    stakingAccount: string;
  } | null;
}>(`${apiBaseUrl}/strategies/${createStrategy.strategyId}`);

console.log(`resulting strategy status: ${strategyStatus.status}`);
console.log(`execution mode: ${strategyStatus.executionMode}`);
console.log(`delegated lp kind: ${strategyStatus.balances.delegatedLpKind}`);

if (strategyStatus.rewardLock) {
  console.log(`reward lock release time: ${strategyStatus.rewardLock.releaseTimeIso}`);
  console.log(`reward lock staking account: ${strategyStatus.rewardLock.stakingAccount}`);
}
