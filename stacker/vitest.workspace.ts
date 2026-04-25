import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    projects: [
      {
        test: {
          name: "stacker",
          environment: "node",
          include: [
            "packages/**/test/**/*.test.ts",
            "apps/**/test/**/*.test.ts",
            "test/**/*.test.ts"
          ]
        }
      }
    ]
  }
});
