export class StrategyLocks {
  private readonly activeLocks = new Set<string>();

  acquire(strategyId: string): boolean {
    if (this.activeLocks.has(strategyId)) {
      return false;
    }

    this.activeLocks.add(strategyId);
    return true;
  }

  release(strategyId: string): void {
    this.activeLocks.delete(strategyId);
  }

  isLocked(strategyId: string): boolean {
    return this.activeLocks.has(strategyId);
  }
}
