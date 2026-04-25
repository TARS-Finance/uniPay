import 'dotenv/config';
import { execFileSync, type ExecFileSyncOptionsWithStringEncoding } from 'node:child_process';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';

export const ROOT = process.cwd();
export const ENV_PATH = resolve(ROOT, '.env');
export const FE_ENV_PATH = resolve(ROOT, '../initia-fe/.env.local');
export const ZERO_ADDRESS = '0x0000000000000000000000000000000000000000' as const;

export function need(key: string): string {
  const value = process.env[key];

  if (!value) {
    throw new Error(`missing env ${key}`);
  }

  return value;
}

export function upsertEnv(path: string, values: Record<string, string>) {
  const existing = existsSync(path) ? readFileSync(path, 'utf8') : '';
  const lines = existing
    .split('\n')
    .filter((line) => !Object.keys(values).some((key) => line.startsWith(`${key}=`)))
    .filter(Boolean);
  const block = Object.entries(values).map(([key, value]) => `${key}=${value}`);
  writeFileSync(path, `${[...lines, ...block].join('\n')}\n`);
}

export function execText(
  command: string,
  args: string[],
  options: Partial<ExecFileSyncOptionsWithStringEncoding> = {},
) {
  return execFileSync(command, args, {
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    ...options,
  });
}

export function execJson<T>(
  command: string,
  args: string[],
  options: Partial<ExecFileSyncOptionsWithStringEncoding> = {},
) {
  const output = execText(command, args, options).trim();
  return JSON.parse(output) as T;
}

export function sleep(ms: number) {
  return new Promise<void>((resolveSleep) => {
    setTimeout(resolveSleep, ms);
  });
}
