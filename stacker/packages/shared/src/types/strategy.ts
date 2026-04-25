import { z } from "zod";

export const inputDenomSchema = z.enum(["usdc", "iusdc", "uusdc"]);
export const strategyStatusSchema = z.enum([
  "draft",
  "grant_pending",
  "active",
  "executing",
  "partial_lp",
  "paused",
  "expired",
  "error"
]);

export type InputDenom = z.infer<typeof inputDenomSchema>;
export type StrategyStatus = z.infer<typeof strategyStatusSchema>;

const strategyTransitions: Record<StrategyStatus, StrategyStatus[]> = {
  draft: ["grant_pending", "paused", "error"],
  grant_pending: ["active", "paused", "expired", "error"],
  active: ["executing", "paused", "expired", "error"],
  executing: ["active", "partial_lp", "paused", "expired", "error"],
  partial_lp: ["executing", "paused", "expired", "error"],
  paused: ["active", "expired", "error"],
  expired: ["grant_pending"],
  error: ["paused", "grant_pending"]
};

export function canTransitionStrategyStatus(
  from: StrategyStatus,
  to: StrategyStatus
): boolean {
  return strategyTransitions[from].includes(to);
}
