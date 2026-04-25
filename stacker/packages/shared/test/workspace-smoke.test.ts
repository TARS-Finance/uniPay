import { describe, expect, it } from "vitest";
import { STACKER_APP_NAME } from "../src/index.js";

describe("workspace wiring", () => {
  it("exports the shared app name constant", () => {
    expect(STACKER_APP_NAME).toBe("stacker");
  });
});
