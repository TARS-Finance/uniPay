import { shortAddr } from "../../data";
import type { PersonaType } from "../../types";
import { Icons } from "../shared/Icons";
import { useInterwovenKit } from "@initia/interwovenkit-react";

const MERCHANT_WALLET = "0xA4e2bDf091fd";

interface Props {
  persona: PersonaType;
  flipping: boolean;
  wallet?: string | null;
  onHome: () => void;
  onSelectPersona: (target: "customer" | "merchant") => void;
  onOpenHistory: () => void;
  onOpenWallet?: () => void;
  onNewInvoice: () => void;
}

export function AppHeader({
  persona,
  flipping,
  onHome,
  onSelectPersona,
  onOpenHistory,
  onNewInvoice,
}: Props) {
  const {
    openConnect,
    openWallet,
    openBridge,
    initiaAddress,
    isConnected,
  } = useInterwovenKit();

  const handleBridge = () => {
    openBridge({ srcChainId: "initiation-2" });
  };

  return (
    <header
      style={{
        position: "fixed",
        top: 0,
        left: 0,
        right: 0,
        height: 52,
        zIndex: 40,
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        padding: "0 18px",
        backdropFilter: "blur(22px)",
        WebkitBackdropFilter: "blur(22px)",
        background: "rgba(5,7,14,0.72)",
        borderBottom: "1px solid var(--hairline)",
      }}
    >
      {/* Logo / Home */}
      <button
        onClick={onHome}
        style={{
          display: "flex",
          alignItems: "center",
          background: "none",
          border: "none",
          cursor: "pointer",
          padding: "3px 7px",
          borderRadius: 8,
        }}
      >
        <img
          src="/unipay-lockup-horizontal-dark.svg"
          alt="UniPay"
          style={{ height: 22, display: "block" }}
        />
      </button>

      {/* Persona toggle */}
      <div
        style={{
          display: "flex",
          background: "rgba(255,255,255,0.04)",
          borderRadius: 10,
          padding: 2,
          border: "1px solid var(--hairline)",
          gap: 2,
          opacity: flipping ? 0.5 : 1,
          transition: "opacity 200ms",
        }}
      >
        {(["customer", "merchant"] as const).map((p) => (
          <button
            key={p}
            onClick={() => onSelectPersona(p)}
            disabled={flipping}
            style={{
              padding: "4px 14px",
              borderRadius: 7,
              fontSize: 12,
              fontWeight: 500,
              border: "none",
              cursor: flipping ? "wait" : "pointer",
              background:
                persona === p ? "rgba(255,255,255,0.09)" : "transparent",
              color: persona === p ? "var(--text-0)" : "var(--text-3)",
              transition: "background 200ms, color 200ms",
              textTransform: "capitalize",
            }}
          >
            {p}
          </button>
        ))}
      </div>

      {/* Context actions */}
      <div
        style={{
          display: "flex",
          gap: 8,
          alignItems: "center",
          minWidth: 190,
          justifyContent: "flex-end",
        }}
      >
        <span className="pill" style={{ color: "var(--text-3)", letterSpacing: "0.08em", fontSize: 11 }}>
          <span className="dot" style={{ background: "var(--accent)" }} />
          POWERED BY INITIA
        </span>
        {persona === null && (
          <>
            <button className="btn btn-ghost">
              Docs <Icons.ext />
            </button>
          </>
        )}
        {persona === "customer" && (
          <>
            {/* Native Initia feature: interwoven-bridge */}
            <button
              className="btn btn-ghost"
              onClick={handleBridge}
              title="Bridge assets via Interwoven Bridge"
              style={{ padding: "8px 12px", fontSize: 12 }}
            >
              Bridge
            </button>
            <button
              className="iconbtn"
              onClick={onOpenHistory}
              title="History"
              style={{ width: 32, height: 32, borderRadius: 9 }}
            >
              <Icons.history />
            </button>
            {isConnected && initiaAddress ? (
              <div
                className="chip"
                style={{
                  cursor: "pointer",
                  padding: "5px 9px 5px 5px",
                  fontSize: 12,
                }}
                onClick={() => openWallet()}
                title="Manage wallet"
              >
                <span className="logo lt-initia">I</span>
                <span className="mono" style={{ fontSize: 12 }}>
                  {shortAddr(initiaAddress)}
                </span>
              </div>
            ) : (
              <button
                className="btn"
                style={{ padding: "8px 12px", fontSize: 12 }}
                onClick={() => openConnect()}
              >
                <Icons.wallet /> Connect wallet
              </button>
            )}
          </>
        )}
        {persona === "merchant" && (
          <>
            {/* Native Initia feature: interwoven-bridge */}
            <button
              className="btn btn-ghost"
              onClick={handleBridge}
              title="Bridge assets via Interwoven Bridge"
              style={{ padding: "8px 12px", fontSize: 12 }}
            >
              Bridge
            </button>
            <button
              className="btn btn-ghost"
              style={{ padding: "8px 12px", fontSize: 12 }}
              onClick={onNewInvoice}
            >
              <Icons.plus /> New invoice
            </button>
            {isConnected && initiaAddress ? (
              <div
                className="chip"
                style={{
                  cursor: "pointer",
                  padding: "5px 9px 5px 5px",
                  fontSize: 12,
                }}
                onClick={() => openWallet()}
                title="Manage wallet"
              >
                <span className="logo lt-initia">M</span>
                <span className="mono" style={{ fontSize: 12 }}>
                  {shortAddr(initiaAddress)}
                </span>
                <Icons.chevron style={{ color: "var(--text-3)" }} />
              </div>
            ) : (
              <div
                className="chip"
                style={{
                  cursor: "default",
                  padding: "5px 9px 5px 5px",
                  fontSize: 12,
                }}
              >
                <span className="logo lt-initia">M</span>
                <span className="mono" style={{ fontSize: 12 }}>
                  {shortAddr(MERCHANT_WALLET)}
                </span>
                <Icons.chevron style={{ color: "var(--text-3)" }} />
              </div>
            )}
          </>
        )}
      </div>
    </header>
  );
}
