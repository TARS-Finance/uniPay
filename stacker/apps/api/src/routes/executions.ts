import { z } from "zod";
import type { FastifyInstance } from "fastify";

const executionsParamsSchema = z.object({
  id: z.string().uuid()
});

export async function executionsRoutes(app: FastifyInstance) {
  app.get("/strategies/:id/executions", async (request, reply) => {
    const parsed = executionsParamsSchema.safeParse(request.params);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const executions = await app.services.executions.listByStrategyId(
      parsed.data.id
    );

    return reply.send({ executions });
  });
}
