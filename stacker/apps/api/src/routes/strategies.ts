import { z } from "zod";
import type { FastifyInstance } from "fastify";
import { inputDenomSchema } from "@stacker/shared";

const createStrategySchema = z.object({
  userId: z.string().uuid(),
  inputDenom: inputDenomSchema,
  targetPoolId: z.string().min(1),
  validatorAddress: z.string().min(1),
  minBalanceAmount: z.string().min(1),
  maxAmountPerRun: z.string().min(1),
  maxSlippageBps: z.number().int().nonnegative(),
  cooldownSeconds: z.number().int().nonnegative()
});

const strategyParamsSchema = z.object({
  id: z.string().uuid()
});

export async function strategiesRoutes(app: FastifyInstance) {
  app.post("/strategies", async (request, reply) => {
    const parsed = createStrategySchema.safeParse(request.body);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const strategy = await app.services.strategies.create(parsed.data);

    return reply.status(201).send({
      strategyId: strategy.id,
      status: strategy.status
    });
  });

  app.get("/strategies/:id", async (request, reply) => {
    const parsed = strategyParamsSchema.safeParse(request.params);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const strategy = await app.services.strategies.getStatus(parsed.data.id);

    if (!strategy) {
      return reply.status(404).send({
        error: "Strategy not found"
      });
    }

    return reply.send(strategy);
  });

  app.post("/strategies/:id/pause", async (request, reply) => {
    const parsed = strategyParamsSchema.safeParse(request.params);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const strategy = await app.services.strategies.pause(parsed.data.id);

    if (!strategy) {
      return reply.status(404).send({
        error: "Strategy not found"
      });
    }

    return reply.send({
      strategyId: strategy.id,
      status: strategy.status
    });
  });

  app.post("/strategies/:id/resume", async (request, reply) => {
    const parsed = strategyParamsSchema.safeParse(request.params);

    if (!parsed.success) {
      return reply.status(400).send({
        error: parsed.error.flatten()
      });
    }

    const strategy = await app.services.strategies.resume(parsed.data.id);

    if (!strategy) {
      return reply.status(404).send({
        error: "Strategy not found"
      });
    }

    return reply.send({
      strategyId: strategy.id,
      status: strategy.status
    });
  });
}
