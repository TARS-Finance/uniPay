import { desc, eq } from "drizzle-orm";
import { withdrawals } from "../../drizzle/schema.js";
import type { StackerDatabase } from "../client.js";

export class WithdrawalsRepository {
  constructor(private readonly db: StackerDatabase) {}

  async create(values: typeof withdrawals.$inferInsert) {
    const [withdrawal] = await this.db
      .insert(withdrawals)
      .values(values)
      .returning();

    if (!withdrawal) {
      throw new Error("Failed to create withdrawal");
    }

    return withdrawal;
  }

  async update(id: string, values: Partial<typeof withdrawals.$inferInsert>) {
    const [withdrawal] = await this.db
      .update(withdrawals)
      .set(values)
      .where(eq(withdrawals.id, id))
      .returning();

    if (!withdrawal) {
      throw new Error(`Failed to update withdrawal ${id}`);
    }

    return withdrawal;
  }

  async findById(id: string) {
    return this.db.query.withdrawals.findFirst({
      where: eq(withdrawals.id, id),
    }) ?? null;
  }

  async listByUserId(userId: string) {
    return this.db.query.withdrawals.findMany({
      where: eq(withdrawals.userId, userId),
      orderBy: desc(withdrawals.requestedAt),
    });
  }
}
