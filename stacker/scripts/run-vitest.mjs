import { spawnSync } from "node:child_process";

const forwardedArgs = process.argv
  .slice(2)
  .filter((arg) => arg !== "--runInBand");
const hasExplicitMaxWorkers = forwardedArgs.some((arg) =>
  arg === "--maxWorkers" || arg.startsWith("--maxWorkers=")
);
const vitestArgs = [
  "exec",
  "vitest",
  "run",
  "--config",
  "vitest.workspace.ts",
  ...(hasExplicitMaxWorkers ? [] : ["--maxWorkers=1"]),
  ...forwardedArgs,
];

const result = spawnSync(
  "pnpm",
  vitestArgs,
  {
    stdio: "inherit",
    shell: process.platform === "win32"
  }
);

if (typeof result.status === "number") {
  process.exit(result.status);
}

process.exit(1);
