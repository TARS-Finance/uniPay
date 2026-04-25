import { web3 } from "@coral-xyz/anchor";

import logger, { loadConfig } from "./config";
import Watcher, { updateConfirmations } from "./watcher";
import PgStore from "./storage";
import SolanaEventParser from "./event-parser";
import SolanaClient from "./solana-client";

async function main() {
  const config = await loadConfig();
  logger.info("starting solana-watcher", {
    chain: config.chainName,
    rpc: config.rpcUrl,
    nativeProgram: config.nativeProgram.idl.address,
    splProgram: config.splProgram.idl.address,
  });

  const connection = new web3.Connection(config.rpcUrl, "confirmed");
  const storage = new PgStore(config.databaseUrl, config.chainName);
  const solanaClient = new SolanaClient(connection);

  const tasks: Promise<unknown>[] = [];

  const nativeId = config.nativeProgram.idl.address;
  if (nativeId) {
    const nativeProgramId = new web3.PublicKey(nativeId);
    const nativeParser = new SolanaEventParser(config.nativeProgram.idl);
    const nativeWatcher = new Watcher(nativeParser, nativeProgramId, solanaClient, storage);
    tasks.push(
      nativeWatcher.run(
        config.watcherPollIntervalSecs,
        config.nativeProgram.startAfterTransaction,
      ),
    );
  }

  const splId = config.splProgram.idl.address;
  if (splId) {
    const splProgramId = new web3.PublicKey(splId);
    const splParser = new SolanaEventParser(config.splProgram.idl);
    const splWatcher = new Watcher(splParser, splProgramId, solanaClient, storage);
    tasks.push(
      splWatcher.run(
        config.watcherPollIntervalSecs,
        config.splProgram.startAfterTransaction,
      ),
    );
  }

  tasks.push(
    updateConfirmations(solanaClient, storage, config.confirmationPollIntervalSecs),
  );

  await Promise.all(tasks);
}

if (require.main === module) {
  main().catch((err) => {
    logger.error("solana-watcher crashed", { err: err instanceof Error ? err.stack : String(err) });
    process.exit(1);
  });
}
