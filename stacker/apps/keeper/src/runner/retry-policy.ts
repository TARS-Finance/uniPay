type GrantBundle = {
  moveGrantExpiresAt: Date | null;
  stakingGrantExpiresAt: Date | null;
  feegrantExpiresAt: Date | null;
  moveGrantStatus: string;
  stakingGrantStatus: string;
  feegrantStatus: string;
};

export function isGrantBundleActive(
  grant: GrantBundle | null,
  now: Date,
  input: {
    requiresStakingGrant?: boolean;
  } = {}
): boolean {
  if (!grant) {
    return false;
  }

  const requiresStakingGrant = input.requiresStakingGrant ?? true;

  return (
    grant.moveGrantStatus === "active"
    && grant.feegrantStatus === "active"
    && !!grant.moveGrantExpiresAt
    && !!grant.feegrantExpiresAt
    && grant.moveGrantExpiresAt > now
    && grant.feegrantExpiresAt > now
    && (
      !requiresStakingGrant
      || (
        grant.stakingGrantStatus === "active"
        && !!grant.stakingGrantExpiresAt
        && grant.stakingGrantExpiresAt > now
      )
    )
  );
}

export function computeNextEligibleAt(now: Date, cooldownSeconds: string): Date {
  return new Date(now.getTime() + Number(cooldownSeconds) * 1000);
}

export function minBigIntString(left: string, right: string): string {
  return (BigInt(left) < BigInt(right) ? BigInt(left) : BigInt(right)).toString();
}

export function serializeError(error: unknown): string {
  if (error instanceof Error) {
    const candidate = error as Error & {
      code?: string;
      response?: {
        status?: number;
        data?: unknown;
      };
    };
    const parts = [error.message];

    if (candidate.code) {
      parts.push(`code=${candidate.code}`);
    }

    if (candidate.response?.status !== undefined) {
      parts.push(`status=${candidate.response.status}`);
    }

    if (candidate.response?.data !== undefined) {
      const responseData =
        typeof candidate.response.data === "string"
          ? candidate.response.data
          : JSON.stringify(candidate.response.data);

      parts.push(`response=${responseData}`);
    }

    return parts.join(" | ");
  }

  if (typeof error === "object") {
    return JSON.stringify(error);
  }

  return String(error);
}
