import { z } from "zod";

export const executionStatusSchema = z.enum([
  "queued",
  "providing",
  "delegating",
  "simulated",
  "success",
  "failed",
  "retryable"
]);

export type ExecutionStatus = z.infer<typeof executionStatusSchema>;

export type ExecutionStatusPayload = {
  status: ExecutionStatus;
  provideTxHash?: string;
  delegateTxHash?: string;
};

export function createExecutionStatusPayload(
  input: ExecutionStatusPayload
): ExecutionStatusPayload {
  const payload: ExecutionStatusPayload = {
    status: input.status
  };

  if (
    ["providing", "delegating", "simulated", "success", "failed", "retryable"].includes(
      input.status
    )
    && input.provideTxHash
  ) {
    payload.provideTxHash = input.provideTxHash;
  }

  if (
    ["delegating", "simulated", "success", "failed", "retryable"].includes(input.status)
    && input.delegateTxHash
  ) {
    payload.delegateTxHash = input.delegateTxHash;
  }

  return payload;
}
