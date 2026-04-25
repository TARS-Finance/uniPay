import {
  ExecutionsRepository,
  GrantsRepository,
  openDatabase,
  PositionsRepository,
  StrategiesRepository,
  UsersRepository
} from "@stacker/db";
import {
  createDryRunKeeperChainClient,
  createLiveKeeperChainClient,
  type KeeperChainClient
} from "@stacker/chain";
import { loadKeeperConfig } from "./config.js";
import { runTickJob } from "./jobs/run-tick.js";
import { createKeeperLogger } from "./logger.js";
import { createKeeperRunner } from "./runner/keeper-runner.js";
import { StrategyLocks } from "./runner/locks.js";

function createChainClient(config: ReturnType<typeof loadKeeperConfig>): KeeperChainClient {
  if (config.mode === "dry-run") {
    return createDryRunKeeperChainClient({
      keeperAddress: config.keeperAddress,
      lpDenom: config.lpDenom,
      defaultInputBalance: config.dryRunInputBalance
    });
  }

  return createLiveKeeperChainClient({
    lcdUrl: config.initiaLcdUrl,
    privateKey: config.keeperPrivateKey,
    keeperAddress: config.keeperAddress,
    executionMode: config.executionMode,
    chainId: config.initiaChainId,
    gasPrices: config.gasPrices,
    gasAdjustment: config.gasAdjustment
  });
}

const config = loadKeeperConfig();
const logger = createKeeperLogger(config.logLevel, {
  pretty: config.logPretty
}).child({
  mode: config.mode,
  executionMode: config.executionMode,
  keeperAddress: config.keeperAddress,
  pollIntervalMs: config.pollIntervalMs,
  lockupSeconds: config.lockupSeconds,
  lpDenom: config.lpDenom
});
const { client, db } = openDatabase(config.databaseUrl);

await client.connect();

logger.info(
  {
    initiaLcdUrl: config.initiaLcdUrl,
    initiaChainId: config.initiaChainId,
    gasPrices: config.gasPrices,
    gasAdjustment: config.gasAdjustment
  },
  "keeper starting"
);

const runner = createKeeperRunner({
  now: () => new Date(),
  usersRepository: new UsersRepository(db),
  strategiesRepository: new StrategiesRepository(db),
  grantsRepository: new GrantsRepository(db),
  executionsRepository: new ExecutionsRepository(db),
  positionsRepository: new PositionsRepository(db),
  chain: createChainClient(config),
  locks: new StrategyLocks(),
  lpDenom: config.lpDenom,
  lockStakingModuleAddress: config.lockStakingModuleAddress,
  lockStakingModuleName: config.lockStakingModuleName,
  lockupSeconds: config.lockupSeconds,
  requireGrants: config.executionMode === "authz",
  logger
});

const timer = setInterval(async () => {
  try {
    const results = await runTickJob(runner);

    if (Array.isArray(results) && results.length > 0) {
      logger.info(
        {
          tickedAt: new Date().toISOString(),
          results
        },
        "keeper tick completed"
      );
    }
  } catch (error) {
    logger.error(
      {
        error
      },
      "keeper tick failed"
    );
  }
}, config.pollIntervalMs);

const shutdown = async () => {
  clearInterval(timer);
  logger.info({}, "keeper shutting down");
  await client.end();
  process.exit(0);
};

process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
