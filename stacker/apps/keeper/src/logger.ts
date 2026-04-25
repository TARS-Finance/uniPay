import pino from "pino";
import pinoPretty from "pino-pretty";

export type LoggerLike = {
  child(bindings: Record<string, unknown>): LoggerLike;
  debug(bindings: Record<string, unknown>, message?: string): void;
  info(bindings: Record<string, unknown>, message?: string): void;
  warn(bindings: Record<string, unknown>, message?: string): void;
  error(bindings: Record<string, unknown>, message?: string): void;
};

const INPUT_DENOM_DECIMALS: Record<string, number> = {
  usdc: 6,
  iusdc: 6,
  uusdc: 6
};

const INPUT_DENOM_SYMBOL: Record<string, string> = {
  usdc: "USDC",
  iusdc: "USDC",
  uusdc: "USDC"
};

function formatDecimalAmount(rawAmount: string, decimals: number) {
  const value = BigInt(rawAmount);
  const sign = value < 0n ? "-" : "";
  const absolute = value < 0n ? -value : value;
  const scale = 10n ** BigInt(decimals);
  const whole = absolute / scale;
  const fraction = absolute % scale;

  if (fraction === 0n) {
    return `${sign}${whole.toString()}`;
  }

  const paddedFraction = fraction.toString().padStart(decimals, "0");
  const trimmedFraction = paddedFraction.replace(/0+$/, "");

  return `${sign}${whole.toString()}.${trimmedFraction}`;
}

export function formatInputAmount(inputDenom: string, rawAmount: string) {
  const decimals = INPUT_DENOM_DECIMALS[inputDenom];
  const symbol = INPUT_DENOM_SYMBOL[inputDenom];

  if (decimals === undefined || !symbol) {
    return `${rawAmount} ${inputDenom}`;
  }

  return `${formatDecimalAmount(rawAmount, decimals)} ${symbol}`;
}

export function describeInputAmount(inputDenom: string, rawAmount: string) {
  return `${formatInputAmount(inputDenom, rawAmount)} (raw ${rawAmount} ${inputDenom})`;
}

export function createKeeperLogger(
  level = "info",
  input: {
    pretty?: boolean;
  } = {}
): LoggerLike {
  const stream = input.pretty
    ? pinoPretty({
        colorize: true,
        translateTime: "SYS:standard",
        ignore: "pid,hostname",
        messageFormat: "{msg}"
      })
    : undefined;

  return pino(
    {
      level,
      base: {
        service: "stacker-keeper"
      }
    },
    stream
  ) as LoggerLike;
}

export const noopLogger: LoggerLike = {
  child() {
    return noopLogger;
  },
  debug() {},
  info() {},
  warn() {},
  error() {}
};
