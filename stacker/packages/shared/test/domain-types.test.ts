import { describe, expect, it } from "vitest";
import {
  canTransitionStrategyStatus,
  inputDenomSchema
} from "../src/types/strategy.js";
import { createExecutionStatusPayload } from "../src/types/execution.js";

describe("shared domain types", () => {
  it("represents strategy status transitions consistently", () => {
    expect(canTransitionStrategyStatus("active", "executing")).toBe(true);
    expect(canTransitionStrategyStatus("draft", "active")).toBe(false);
  });

  it("only accepts usdc, iusdc, or uusdc as input denoms", () => {
    expect(inputDenomSchema.parse("usdc")).toBe("usdc");
    expect(inputDenomSchema.parse("iusdc")).toBe("iusdc");
    expect(inputDenomSchema.parse("uusdc")).toBe("uusdc");
    expect(() => inputDenomSchema.parse("init")).toThrowError();
  });

  it("only includes execution hashes when the status makes them relevant", () => {
    expect(
      createExecutionStatusPayload({
        status: "queued",
        provideTxHash: "0xprovide",
        delegateTxHash: "0xdelegate"
      })
    ).toEqual({ status: "queued" });

    expect(
      createExecutionStatusPayload({
        status: "delegating",
        provideTxHash: "0xprovide",
        delegateTxHash: "0xdelegate"
      })
    ).toEqual({
      status: "delegating",
      provideTxHash: "0xprovide",
      delegateTxHash: "0xdelegate"
    });
  });
});
