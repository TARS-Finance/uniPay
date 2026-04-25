import { z } from "zod";
import type { FastifyInstance } from "fastify";

const positionsParamsSchema = z.object({
  userId: z.string().uuid()
});

export async function positionsRoutes(app: FastifyInstance) {
  app.get("/positions/:userId", async (request, reply) => {
    const parsed = positionsParamsSchema.safeParse(request.params);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const positions = await app.services.positions.listByUserId(
      parsed.data.userId
    );

    return reply.send({ positions });
  });
}
