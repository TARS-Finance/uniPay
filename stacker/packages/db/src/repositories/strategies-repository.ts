import { and, eq, inArray, isNull, lte, or } from "drizzle-orm";
import { strategies } from "../../drizzle/schema.js";
import type { StackerDatabase } from "../client.js";

export class StrategiesRepository {
  constructor(private readonly db: StackerDatabase) {}

  async create(values: typeof strategies.$inferInsert) {
    const [strategy] = await this.db
      .insert(strategies)
      .values(values)
      .returning();

    if (!strategy) {
      throw new Error("Failed to create strategy");
    }

    return strategy;
  }

  async findById(id: string) {
    const strategy = await this.db.query.strategies.findFirst({
      where: eq(strategies.id, id)
    });

    return strategy ?? null;
  }

  async findByUserId(userId: string) {
    return this.db.query.strategies.findMany({
      where: eq(strategies.userId, userId)
    });
  }

  async patch(
    id: string,
    values: Partial<typeof strategies.$inferInsert>
  ) {
    const [strategy] = await this.db
      .update(strategies)
      .set({ ...values, updatedAt: new Date() })
      .where(eq(strategies.id, id))
      .returning();

    if (!strategy) {
      throw new Error(`Failed to update strategy ${id}`);
    }

    return strategy;
  }

  async updateStatus(id: string, status: typeof strategies.$inferInsert.status) {
    return this.patch(id, { status });
  }

  async findRunnableStrategies(now: Date) {
    return this.db.query.strategies.findMany({
      where: and(
        inArray(strategies.status, ["active", "partial_lp"]),
        or(isNull(strategies.nextEligibleAt), lte(strategies.nextEligibleAt, now))
      )
    });
  }
}
