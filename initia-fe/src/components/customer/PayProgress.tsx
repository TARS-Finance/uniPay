import { Icons } from "../shared/Icons";
import { HashLink } from "../shared/HashLink";
import type { SwapStep } from "../../hooks/useSwap";

interface Props {
  step: SwapStep;
  stepIndex: number;
  srcAmount: number;
  destAmount: number;
  srcToken: string;
  destToken: string;
  srcChainName?: string;
  destChainName?: string;
  receiverAddress?: string;
  srcDecimals?: number;
  destDecimals?: number;
  sourceTxHash: string | null;
  destinationTxHash: string | null;
  redeemTxHash: string | null;
  error: string | null;
  onDone: () => void;
}

function shortAddr(a: string): string {
  if (!a) return "";
  return a.length > 14 ? `${a.slice(0, 8)}…${a.slice(-6)}` : a;
}

export function PayProgress({
  step,
  stepIndex,
  srcAmount,
  destAmount,
  srcToken,
  destToken,
  srcChainName,
  destChainName,
  receiverAddress,
  srcDecimals = 6,
  destDecimals = 6,
  sourceTxHash,
  destinationTxHash,
  redeemTxHash,
  error,
  onDone,
}: Props) {
  const isDone = step === "done";
  const isError = step === "error";

  const srcPair = srcChainName ? `${srcToken} on ${srcChainName}` : srcToken;
  const destPair = destChainName
    ? `${destToken} on ${destChainName}`
    : destToken;
  const shortReceiver = receiverAddress ? shortAddr(receiverAddress) : "";
  const deliveredTo = shortReceiver ? `to ${shortReceiver}` : "to merchant";

  const steps = [
    {
      label: `Initiating swap`,
      desc: srcChainName
        ? `Preparing ${srcChainName} transaction · confirm in your wallet`
        : `Preparing transaction · confirm in your wallet`,
    },
    {
      label: `Locking ${srcPair}`,
      desc: `Sending ${srcAmount.toFixed(
        srcDecimals,
      )} ${srcToken} to trustless vault`,
    },
    {
      label: `${srcPair} locked`,
      desc: `Waiting for executor to lock ${destPair}`,
    },
    {
      label: `${destPair} locked by executor`,
      desc: `Revealing secret · executor delivers ${destToken} ${deliveredTo}`,
    },
    {
      label: `${destPair} delivered`,
      desc:
        destAmount > 0
          ? `${destAmount.toFixed(destDecimals)} ${destToken} arrived at ${
              shortReceiver || "merchant"
            }`
          : `${destToken} arrived at ${shortReceiver || "merchant"}`,
    },
  ];

  return (
    <div
      className="pay-progress-shell"
      style={{
        width: "100%",
        maxWidth: 620,
        margin: "0 auto",
        position: "relative",
      }}
    >
      <div
        className="glass"
        style={{ padding: 28, position: "relative", overflow: "hidden" }}
      >
        {isDone && <ConfettiBurst />}
        <div
          style={{
            position: "absolute",
            top: -80,
            left: "50%",
            transform: "translateX(-50%)",
            width: 400,
            height: 200,
            background: isDone
              ? "color-mix(in oklch, var(--success) 35%, transparent)"
              : isError
              ? "rgba(239,68,68,0.15)"
              : "var(--accent-glow)",
            filter: "blur(80px)",
            pointerEvents: "none",
            transition: "background 400ms",
          }}
        />

        <div className="pay-progress-header" style={{ position: "relative" }}>
          <div>
            <div className="eyebrow" style={{ marginBottom: 6 }}>
              Atomic swap · in-flight
            </div>
            <div
              style={{
                fontSize: 22,
                fontWeight: 500,
                letterSpacing: "-0.01em",
              }}
            >
              {isDone
                ? "Swap complete"
                : isError
                ? "Swap failed"
                : "Processing swap"}
            </div>
            <div
              className="mono tnum"
              style={{ fontSize: 13, color: "var(--text-2)", marginTop: 4 }}
            >
              {srcAmount.toFixed(srcDecimals)} {srcToken}{" "}
              <span style={{ color: "var(--text-3)" }}>→</span>{" "}
              {destAmount > 0 ? `${destAmount.toFixed(destDecimals)} ` : ""}
              {destToken}
            </div>
          </div>
        </div>

        {isError && error && (
          <div
            style={{
              marginBottom: 16,
              padding: "10px 14px",
              borderRadius: 10,
              background: "rgba(239,68,68,0.1)",
              border: "1px solid rgba(239,68,68,0.3)",
              fontSize: 13,
              color: "#f87171",
            }}
          >
            {error}
          </div>
        )}

        <div
          style={{
            display: "flex",
            flexDirection: "column",
            gap: 0,
            position: "relative",
            margin: "12px 0",
          }}
        >
          {steps.map((s, i) => (
            <StepRow
              key={i}
              index={i + 1}
              label={s.label}
              desc={s.desc}
              status={
                i < stepIndex
                  ? "done"
                  : i === stepIndex && !isDone && !isError
                  ? "active"
                  : isDone && i === steps.length - 1
                  ? "done"
                  : "pending"
              }
              isLast={i === steps.length - 1}
            />
          ))}
        </div>

        {(sourceTxHash || destinationTxHash || redeemTxHash) && (
          <div
            style={{
              marginTop: 16,
              padding: 16,
              borderRadius: 12,
              background: "rgba(255,255,255,0.015)",
              border: "1px solid var(--hairline)",
            }}
          >
            <div className="eyebrow" style={{ marginBottom: 10 }}>
              Transaction links
            </div>
            <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
              {sourceTxHash && (
                <HashLink hash={sourceTxHash} label="Source HTLC lock ·" />
              )}
              {destinationTxHash && (
                <HashLink
                  hash={destinationTxHash}
                  label="Executor USDC lock ·"
                />
              )}
              {redeemTxHash && (
                <HashLink hash={redeemTxHash} label="USDC delivered ·" />
              )}
            </div>
          </div>
        )}

        <div className="progress-actions">
          {isDone ? (
            <button
              className="btn btn-primary btn-lg"
              style={{ flex: 1 }}
              onClick={onDone}
            >
              Make another swap
            </button>
          ) : (
            <button className="btn btn-lg" style={{ flex: 1 }} onClick={onDone}>
              {isError ? "Dismiss" : "Cancel"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

export function StepRow({
  index,
  label,
  desc,
  status,
  isLast,
}: {
  index: number;
  label: string;
  desc: string;
  status: "pending" | "active" | "done";
  isLast: boolean;
}) {
  const isActive = status === "active";
  const isDone = status === "done";
  return (
    <div
      style={{
        display: "flex",
        gap: 14,
        opacity: status === "pending" ? 0.4 : 1,
        transition: "opacity 400ms",
        animation: isActive ? "fade-in 400ms ease" : undefined,
      }}
    >
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
        }}
      >
        <div
          className={`step-dot ${isActive ? "active" : isDone ? "done" : ""}`}
        >
          {isDone ? (
            <Icons.check />
          ) : isActive ? (
            <Icons.spinner />
          ) : (
            <span>{index}</span>
          )}
        </div>
        {!isLast && <div className={`step-line ${isDone ? "done" : ""}`} />}
      </div>
      <div style={{ paddingBottom: isLast ? 0 : 18, flex: 1 }}>
        <div
          style={{
            fontSize: 14,
            fontWeight: 500,
            color: isDone
              ? "var(--success)"
              : isActive
              ? "var(--text-0)"
              : "var(--text-1)",
          }}
        >
          {label}
        </div>
        <div
          className="mono"
          style={{ fontSize: 12, color: "var(--text-2)", marginTop: 3 }}
        >
          {desc}
        </div>
      </div>
    </div>
  );
}

function ConfettiBurst() {
  const colors = ["#6ea9ff", "#b7d4ff", "#86efac", "#fde68a", "#fca5a5"];
  const pieces = Array.from({ length: 36 }, (_, i) => ({
    id: i,
    x: Math.random() * 100,
    delay: Math.random() * 0.2,
    dur: 1.5 + Math.random() * 1.2,
    color: colors[i % colors.length],
    size: 6 + Math.random() * 6,
  }));
  return (
    <>
      {pieces.map((p) => (
        <div
          key={p.id}
          style={{
            position: "absolute",
            top: 0,
            left: `${p.x}%`,
            width: p.size,
            height: p.size * 1.6,
            background: p.color,
            animation: `confetti-fall ${p.dur}s ease-in ${p.delay}s forwards`,
            borderRadius: 1,
            zIndex: 5,
          }}
        />
      ))}
    </>
  );
}
