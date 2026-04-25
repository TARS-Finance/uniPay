import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { createApp } from "../src/app.js";

const execFileAsync = promisify(execFile);

describe("mock frontend flow", () => {
  let app: Awaited<ReturnType<typeof createApp>>;
  let apiBaseUrl: string;

  beforeAll(async () => {
    app = await createApp({
      config: {
        keeperAddress: "init1replacekeeperaddress",
        lockStakingModuleAddress: "0xlock",
        lockStakingModuleName: "lock_staking",
        lockupSeconds: "86400"
      },
      grantVerifier: {
        verify: async () => ({
          moveGrantActive: true,
          feegrantActive: true
        })
      }
    });
    await app.ready();
    apiBaseUrl = await app.listen({
      host: "127.0.0.1",
      port: 0
    });
  });

  beforeEach(async () => {
    await app.db.execute(`
      truncate table executions, positions, grants, strategies, users
      restart identity cascade;
    `);
  });

  afterAll(async () => {
    await app?.close();
  });

  it("runs the onboarding flow from the script and prints the resulting lifecycle", async () => {
    const { stdout } = await execFileAsync("pnpm", [
      "exec",
      "tsx",
      "scripts/mock-fe.ts",
      "--api-base-url",
      apiBaseUrl,
      "--config",
      "./scripts/mock-fe.example.json"
    ], {
      cwd: process.cwd()
    });

    expect(stdout).toContain("created user id:");
    expect(stdout).toContain("created strategy id:");
    expect(stdout).toContain("keeper address:");
    expect(stdout).toContain("resulting strategy status: active");
    expect(stdout).toContain("delegated lp kind:");
  });
});
