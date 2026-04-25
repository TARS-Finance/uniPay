import { ExecutionsRepository } from "@stacker/db";

export class ExecutionsService {
  constructor(private readonly executionsRepository: ExecutionsRepository) {}

  async listByStrategyId(strategyId: string) {
    const executions =
      await this.executionsRepository.listByStrategyId(strategyId);

    return executions.map((execution) => ({
      id: execution.id,
      status: execution.status,
      inputAmount: execution.inputAmount,
      lpAmount: execution.lpAmount,
      provideTxHash: execution.provideTxHash,
      delegateTxHash: execution.delegateTxHash,
      errorCode: execution.errorCode,
      errorMessage: execution.errorMessage,
      startedAt: execution.startedAt.toISOString(),
      finishedAt: execution.finishedAt?.toISOString() ?? null
    }));
  }
}
