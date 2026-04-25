import cors from "@fastify/cors";
import Fastify, { type FastifyInstance } from "fastify";
import { RESTClient } from "@initia/initia.js";
import {
  ExecutionsRepository,
  GrantsRepository,
  openDatabase,
  PositionsRepository,
  StrategiesRepository,
  UsersRepository,
  WithdrawalsRepository,
  type StackerDatabase
} from "@stacker/db";
import type { ApiConfig } from "./config.js";
import { loadApiConfig } from "./config.js";
import { executionsRoutes } from "./routes/executions.js";
import { grantsRoutes } from "./routes/grants.js";
import { merchantsRoutes } from "./routes/merchants.js";
import { positionsRoutes } from "./routes/positions.js";
import { strategiesRoutes } from "./routes/strategies.js";
import { usersRoutes } from "./routes/users.js";
import { ExecutionsService } from "./services/executions-service.js";
import {
  type GrantVerifier,
  InitiaGrantVerifier
} from "./services/grant-verifier.js";
import { GrantsService } from "./services/grants-service.js";
import { ChainService } from "./services/chain-service.js";
import { PositionsService } from "./services/positions-service.js";
import { StrategiesService } from "./services/strategies-service.js";
import { UsersService } from "./services/users-service.js";
import { WithdrawalsService } from "./services/withdrawals-service.js";

export type AppServices = {
  users: UsersService;
  strategies: StrategiesService;
  grants: GrantsService;
  positions: PositionsService;
  executions: ExecutionsService;
  withdrawals: WithdrawalsService;
};

declare module "fastify" {
  interface FastifyInstance {
    db: StackerDatabase;
    services: AppServices;
    stackerConfig: ApiConfig;
  }
}

function createServices(
  db: StackerDatabase,
  config: ApiConfig,
  grantVerifier: GrantVerifier,
  rest: RESTClient
): AppServices {
  const usersRepository = new UsersRepository(db);
  const strategiesRepository = new StrategiesRepository(db);
  const grantsRepository = new GrantsRepository(db);
  const positionsRepository = new PositionsRepository(db);
  const executionsRepository = new ExecutionsRepository(db);

  const chainService = new ChainService(rest);
  const chainConfig = config.targetPoolId && config.merchantValidatorAddress
    ? {
        keeperAddress: config.keeperAddress,
        targetPoolId: config.targetPoolId,
        validatorAddress: config.merchantValidatorAddress,
        moduleAddress: config.lockStakingModuleAddress,
        moduleName: config.lockStakingModuleName,
      }
    : undefined;

  return {
    users: new UsersService(usersRepository),
    strategies: new StrategiesService(
      strategiesRepository,
      grantsRepository,
      positionsRepository,
      executionsRepository,
      config
    ),
    grants: new GrantsService(
      grantsRepository,
      strategiesRepository,
      usersRepository,
      config,
      grantVerifier
    ),
    positions: new PositionsService(
      positionsRepository,
      strategiesRepository,
      executionsRepository,
      chainService,
      chainConfig
    ),
    executions: new ExecutionsService(executionsRepository),
    withdrawals: new WithdrawalsService(
      new WithdrawalsRepository(db),
      strategiesRepository,
      chainService,
      {
        lockStakingModuleAddress: config.lockStakingModuleAddress,
        lockStakingModuleName: config.lockStakingModuleName,
        chainId: config.initiaChainId,
        explorerBase: config.initiaExplorerUrl,
      },
      positionsRepository
    ),
  };
}

export async function createApp(
  options: {
    config?: Partial<ApiConfig>;
    grantVerifier?: GrantVerifier;
    logger?: boolean;
  } = {}
): Promise<FastifyInstance> {
  const config = loadApiConfig(options.config);
  const { client, db } = openDatabase(config.databaseUrl);

  await client.connect();

  const app = Fastify({
    logger: options.logger ?? false
  });

  await app.register(cors, {
    origin: true,
    methods: ["GET", "HEAD", "POST", "PATCH", "PUT", "DELETE", "OPTIONS"],
  });

  const rest = new RESTClient(config.initiaLcdUrl);
  const grantVerifier =
    options.grantVerifier
    ?? new InitiaGrantVerifier(rest);

  app.decorate("db", db);
  app.decorate("services", createServices(db, config, grantVerifier, rest));
  app.decorate("stackerConfig", config);

  app.addHook("onClose", async () => {
    await client.end();
  });

  await app.register(usersRoutes);
  await app.register(strategiesRoutes);
  await app.register(grantsRoutes);
  await app.register(positionsRoutes);
  await app.register(merchantsRoutes);
  await app.register(executionsRoutes);

  return app;
}
