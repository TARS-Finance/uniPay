export type StackerEnvironment = {
  databaseUrl: string;
  keeperPrivateKey: string;
  initiaLcdUrl: string;
  initiaRpcUrl: string;
  keeperAddress: string;
  targetPoolId: string;
  dexModuleAddress: string;
  dexModuleName: string;
  lockStakingModuleAddress: string;
  lockStakingModuleName?: string;
  lockupSeconds: string;
};
