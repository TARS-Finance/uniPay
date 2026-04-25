import { z } from "zod";
import type { FastifyInstance } from "fastify";

const registerUserSchema = z.object({
  initiaAddress: z.string().min(1)
});

export async function usersRoutes(app: FastifyInstance) {
  app.post("/users/register", async (request, reply) => {
    const parsed = registerUserSchema.safeParse(request.body);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const user = await app.services.users.register(parsed.data.initiaAddress);

    return reply.status(201).send({
      userId: user.id,
      initiaAddress: user.initiaAddress
    });
  });
}
