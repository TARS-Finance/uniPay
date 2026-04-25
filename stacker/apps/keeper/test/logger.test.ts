import { describe, expect, it } from "vitest";
import { describeInputAmount } from "../src/logger.js";
import { serializeError } from "../src/runner/retry-policy.js";

describe("keeper logger helpers", () => {
  it("formats uusdc thresholds in human-readable USDC units", () => {
    expect(describeInputAmount("uusdc", "200")).toBe(
      "0.0002 USDC (raw 200 uusdc)"
    );
    expect(describeInputAmount("uusdc", "200000000")).toBe(
      "200 USDC (raw 200000000 uusdc)"
    );
  });

  it("includes upstream response details when serializing keeper errors", () => {
    const error = Object.assign(
      new Error("Request failed with status code 500"),
      {
        code: "ERR_BAD_RESPONSE",
        response: {
          status: 500,
          data: {
            code: 2,
            message:
              "VM aborted: location=0xlock::lock_staking, code=65544"
          }
        }
      }
    );

    expect(serializeError(error)).toContain("Request failed with status code 500");
    expect(serializeError(error)).toContain("ERR_BAD_RESPONSE");
    expect(serializeError(error)).toContain("VM aborted");
  });
});
