import { z } from "zod";
import type { FastifyInstance } from "fastify";

const grantsMutationSchema = z.object({
  userId: z.string().uuid(),
  strategyId: z.string().uuid()
});

export async function grantsRoutes(app: FastifyInstance) {
  app.post("/grants/prepare", async (request, reply) => {
    const parsed = grantsMutationSchema.safeParse(request.body);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const prepared = await app.services.grants.prepare(
      parsed.data.userId,
      parsed.data.strategyId
    );

    if (!prepared) {
      return reply.status(404).send({
        error: "Strategy not found"
      });
    }

    return reply.send(prepared);
  });

  app.post("/grants/confirm", async (request, reply) => {
    const parsed = grantsMutationSchema.safeParse(request.body);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const confirmed = await app.services.grants.confirm(
      parsed.data.userId,
      parsed.data.strategyId
    );

    if (confirmed.kind === "not_found") {
      return reply.status(404).send({
        error: "Grant not found"
      });
    }

    if (confirmed.kind === "verification_failed") {
      return reply.status(409).send({
        error: "Grant verification failed",
        missing: confirmed.missing
      });
    }

    return reply.send({
      strategyId: confirmed.strategyId,
      strategyStatus: confirmed.strategyStatus,
      grantStatus: confirmed.grantStatus
    });
  });
}
