export type RewardLockView = {
  kind: "bonded-locked";
  stakingAccount: string;
  metadata: string;
  releaseTime: string;
  releaseTimeIso: string;
  validatorAddress: string;
  lockedShare: string;
};

export function parseRewardLockSnapshot(
  value: string | null
): RewardLockView | null {
  if (!value) {
    return null;
  }

  try {
    const parsed = JSON.parse(value) as Partial<RewardLockView>;

    if (
      parsed.kind !== "bonded-locked"
      || typeof parsed.stakingAccount !== "string"
      || typeof parsed.metadata !== "string"
      || typeof parsed.releaseTime !== "string"
      || typeof parsed.releaseTimeIso !== "string"
      || typeof parsed.validatorAddress !== "string"
      || typeof parsed.lockedShare !== "string"
    ) {
      return null;
    }

    return parsed as RewardLockView;
  } catch {
    return null;
  }
}
