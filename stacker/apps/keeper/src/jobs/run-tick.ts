export async function runTickJob(runner: { runTick(): Promise<unknown> }) {
  return runner.runTick();
}
