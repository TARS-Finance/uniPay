import { z } from "zod";
import type { FastifyInstance } from "fastify";
import { WithdrawalsService } from "../services/withdrawals-service.js";

const merchantParamsSchema = z.object({
  initiaAddress: z.string().min(1)
});

const activityQuerySchema = z.object({
  limit: z.coerce.number().int().min(1).max(200).optional().default(50)
});

const withdrawalBodySchema = z.object({
  strategyId: z.string().uuid(),
  inputAmount: z.string().regex(/^\d+$/, "Must be a positive integer string"),
});

const confirmWithdrawalBodySchema = z.object({
  txHash: z.string().min(1),
});

const withdrawalParamsSchema = z.object({
  initiaAddress: z.string().min(1),
  withdrawalId: z.string().uuid(),
});

async function resolveMerchant(app: FastifyInstance, initiaAddress: string) {
  return app.services.users.findByInitiaAddress(initiaAddress);
}

export async function merchantsRoutes(app: FastifyInstance) {
  app.get("/merchants/:initiaAddress/balance", async (request, reply) => {
    const parsed = merchantParamsSchema.safeParse(request.params);
    if (!parsed.success) {
      return reply.status(400).send({ error: parsed.error.flatten() });
    }

    const merchant = await resolveMerchant(app, parsed.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    const balance = await app.services.positions.getMerchantBalance(
      merchant.id,
      app.stackerConfig.merchantDemoApyBps,
      parsed.data.initiaAddress
    );
    return reply.send(balance);
  });

  app.get("/merchants/:initiaAddress/overview", async (request, reply) => {
    const parsed = merchantParamsSchema.safeParse(request.params);
    if (!parsed.success) {
      return reply.status(400).send({ error: parsed.error.flatten() });
    }

    const merchant = await resolveMerchant(app, parsed.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    const overview = await app.services.positions.getMerchantOverview(
      merchant.id,
      app.stackerConfig.merchantDemoApyBps,
      parsed.data.initiaAddress
    );
    return reply.send(overview);
  });

  app.get("/merchants/:initiaAddress/pools", async (request, reply) => {
    const parsed = merchantParamsSchema.safeParse(request.params);
    if (!parsed.success) {
      return reply.status(400).send({ error: parsed.error.flatten() });
    }

    const merchant = await resolveMerchant(app, parsed.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    const pools = await app.services.positions.getMerchantPools(
      merchant.id,
      app.stackerConfig.merchantDemoApyBps
    );
    return reply.send({ pools });
  });

  app.get("/merchants/:initiaAddress/activity", async (request, reply) => {
    const parsed = merchantParamsSchema.safeParse(request.params);
    if (!parsed.success) {
      return reply.status(400).send({ error: parsed.error.flatten() });
    }

    const query = activityQuerySchema.safeParse(request.query);
    if (!query.success) {
      return reply.status(400).send({ error: query.error.flatten() });
    }

    const merchant = await resolveMerchant(app, parsed.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    const activity = await app.services.positions.getMerchantActivity(
      merchant.id,
      query.data.limit,
      app.stackerConfig.initiaExplorerUrl,
      app.stackerConfig.initiaChainId
    );
    return reply.send({ activity });
  });

  app.get("/merchants/:initiaAddress/chart", async (request, reply) => {
    const parsed = merchantParamsSchema.safeParse(request.params);
    if (!parsed.success) {
      return reply.status(400).send({ error: parsed.error.flatten() });
    }

    const merchant = await resolveMerchant(app, parsed.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    const chart = await app.services.positions.getMerchantChart(merchant.id);
    return reply.send(chart);
  });

  // ── Withdrawal routes ─────────────────────────────────────────────────────

  app.post("/merchants/:initiaAddress/withdrawals", async (request, reply) => {
    const params = merchantParamsSchema.safeParse(request.params);
    if (!params.success) {
      return reply.status(400).send({ error: params.error.flatten() });
    }

    const body = withdrawalBodySchema.safeParse(request.body);
    if (!body.success) {
      return reply.status(400).send({ error: body.error.flatten() });
    }

    const merchant = await resolveMerchant(app, params.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    try {
      const result = await app.services.withdrawals.createWithdrawal({
        userId: merchant.id,
        initiaAddress: params.data.initiaAddress,
        strategyId: body.data.strategyId,
        inputAmount: body.data.inputAmount,
      });
      return reply.status(201).send(result);
    } catch (err: unknown) {
      const e = err as { statusCode?: number; message?: string };
      return reply.status(e.statusCode ?? 500).send({ error: e.message ?? "Internal error" });
    }
  });

  app.patch("/merchants/:initiaAddress/withdrawals/:withdrawalId", async (request, reply) => {
    const params = withdrawalParamsSchema.safeParse(request.params);
    if (!params.success) {
      return reply.status(400).send({ error: params.error.flatten() });
    }

    const body = confirmWithdrawalBodySchema.safeParse(request.body);
    if (!body.success) {
      return reply.status(400).send({ error: body.error.flatten() });
    }

    const merchant = await resolveMerchant(app, params.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    try {
      const updated = await app.services.withdrawals.confirmWithdrawal({
        userId: merchant.id,
        withdrawalId: params.data.withdrawalId,
        txHash: body.data.txHash,
      });
      return reply.send({ status: updated.status, txHash: updated.txHash });
    } catch (err: unknown) {
      const e = err as { statusCode?: number; message?: string };
      return reply.status(e.statusCode ?? 500).send({ error: e.message ?? "Internal error" });
    }
  });

  app.post("/merchants/:initiaAddress/unbonds", async (request, reply) => {
    const params = merchantParamsSchema.safeParse(request.params);
    if (!params.success) {
      return reply.status(400).send({ error: params.error.flatten() });
    }

    const body = withdrawalBodySchema.safeParse(request.body);
    if (!body.success) {
      return reply.status(400).send({ error: body.error.flatten() });
    }

    const merchant = await resolveMerchant(app, params.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    try {
      const result = await app.services.withdrawals.createUnbond({
        userId: merchant.id,
        initiaAddress: params.data.initiaAddress,
        strategyId: body.data.strategyId,
        inputAmount: body.data.inputAmount,
      });
      return reply.status(201).send(result);
    } catch (err: unknown) {
      const e = err as { statusCode?: number; message?: string };
      return reply.status(e.statusCode ?? 500).send({ error: e.message ?? "Internal error" });
    }
  });

  app.get("/merchants/:initiaAddress/withdrawals", async (request, reply) => {
    const params = merchantParamsSchema.safeParse(request.params);
    if (!params.success) {
      return reply.status(400).send({ error: params.error.flatten() });
    }

    const merchant = await resolveMerchant(app, params.data.initiaAddress);
    if (!merchant) {
      return reply.status(404).send({ error: "Merchant not found" });
    }

    const withdrawals = await app.services.withdrawals.listWithdrawals(
      merchant.id,
      app.stackerConfig.initiaExplorerUrl,
      app.stackerConfig.initiaChainId
    );
    return reply.send({ withdrawals });
  });
}
