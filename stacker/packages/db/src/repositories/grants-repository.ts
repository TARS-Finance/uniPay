import { eq } from "drizzle-orm";
import { grants } from "../../drizzle/schema.js";
import type { StackerDatabase } from "../client.js";

export class GrantsRepository {
  constructor(private readonly db: StackerDatabase) {}

  async upsertForUser(values: typeof grants.$inferInsert) {
    const [grant] = await this.db
      .insert(grants)
      .values(values)
      .onConflictDoUpdate({
        target: grants.userId,
        set: {
          keeperAddress: values.keeperAddress,
          moveGrantExpiresAt: values.moveGrantExpiresAt,
          stakingGrantExpiresAt: values.stakingGrantExpiresAt,
          feegrantExpiresAt: values.feegrantExpiresAt,
          moveGrantStatus: values.moveGrantStatus,
          stakingGrantStatus: values.stakingGrantStatus,
          feegrantStatus: values.feegrantStatus,
          scopeJson: values.scopeJson,
          updatedAt: new Date()
        }
      })
      .returning();

    if (!grant) {
      throw new Error(`Failed to upsert grants for user ${values.userId}`);
    }

    return grant;
  }

  async findByUserId(userId: string) {
    const grant = await this.db.query.grants.findFirst({
      where: eq(grants.userId, userId)
    });

    return grant ?? null;
  }
}
