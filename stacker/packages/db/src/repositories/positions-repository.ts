import { eq } from "drizzle-orm";
import { positions } from "../../drizzle/schema.js";
import type { StackerDatabase } from "../client.js";

export class PositionsRepository {
  constructor(private readonly db: StackerDatabase) {}

  async upsertForStrategy(values: typeof positions.$inferInsert) {
    const [position] = await this.db
      .insert(positions)
      .values(values)
      .onConflictDoUpdate({
        target: positions.strategyId,
        set: {
          userId: values.userId,
          lastInputBalance: values.lastInputBalance,
          lastLpBalance: values.lastLpBalance,
          lastDelegatedLpBalance: values.lastDelegatedLpBalance,
          lastRewardSnapshot: values.lastRewardSnapshot,
          lastSyncedAt: new Date()
        }
      })
      .returning();

    if (!position) {
      throw new Error(
        `Failed to upsert position for strategy ${values.strategyId}`
      );
    }

    return position;
  }

  async findByStrategyId(strategyId: string) {
    const position = await this.db.query.positions.findFirst({
      where: eq(positions.strategyId, strategyId)
    });

    return position ?? null;
  }

  async listByUserId(userId: string) {
    return this.db.query.positions.findMany({
      where: eq(positions.userId, userId)
    });
  }
}
