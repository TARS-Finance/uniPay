import { eq } from "drizzle-orm";
import { users } from "../../drizzle/schema.js";
import type { StackerDatabase } from "../client.js";

export class UsersRepository {
  constructor(private readonly db: StackerDatabase) {}

  async create(initiaAddress: string) {
    const [user] = await this.db
      .insert(users)
      .values({ initiaAddress })
      .returning();

    if (!user) {
      throw new Error("Failed to create user");
    }

    return user;
  }

  async findById(id: string) {
    const user = await this.db.query.users.findFirst({
      where: eq(users.id, id)
    });

    return user ?? null;
  }

  async findByInitiaAddress(initiaAddress: string) {
    const user = await this.db.query.users.findFirst({
      where: eq(users.initiaAddress, initiaAddress)
    });

    return user ?? null;
  }
}
