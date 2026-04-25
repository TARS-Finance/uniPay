import { useState } from "react";
import type { PersonaType, AnyPage } from "../../types";
import { Icons } from "../shared/Icons";
import { useWallet } from "../../lib/wallet-context";
import { useInterwovenKit } from "@initia/interwovenkit-react";
import { useUnisat } from "../../hooks/useUnisat";

interface Props {
  persona: NonNullable<PersonaType>;
  page: AnyPage;
  setPage: (p: AnyPage) => void;
  wallet: string | null;
  onSwitchPersona: () => void;
  onHome: () => void;
}

function Tooltip({ label }: { label: string }) {
  return <span className="sidebar-tooltip">{label}</span>;
}

export function Sidebar({
  persona,
  page,
  setPage,
  wallet,
  onSwitchPersona,
  onHome,
}: Props) {
  const { connect, disconnect } = useWallet();
  const { initiaAddress, openBridge } = useInterwovenKit();
  const {
    unisatAddress,
    connecting: unisatConnecting,
    isInstalled: unisatInstalled,
    connectUnisat,
    disconnectUnisat,
  } = useUnisat();
  const [copied, setCopied] = useState(false);

  const copyAddress = () => {
    const addr = initiaAddress ?? wallet ?? "";
    if (!addr) return;
    navigator.clipboard.writeText(addr).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  };

  const nav: { id: AnyPage; label: string; icon: React.ReactNode }[] = persona === 'customer'
    ? [
        { id: 'pay',     label: 'Pay',     icon: <Icons.coin /> },
        { id: 'history', label: 'History', icon: <Icons.history /> },
      ]
    : [
        { id: 'overview', label: 'Overview', icon: <Icons.bars /> },
        { id: 'pools',    label: 'Pools & Earn', icon: <Icons.leaf /> },
        { id: 'activity', label: 'Activity', icon: <Icons.history /> },
      ];
  const other = persona === 'customer' ? 'merchant' : 'customer';

  return (
    <aside className="sidebar collapsed">
      <div className="sidebar-topbar">
        <button className="sidebar-brand-btn sidebar-tooltip-wrap" onClick={onHome}>
          <div className="sidebar-brand-mark">
            <img src="/unipay-mark-white.svg" alt="UniPay" style={{ width: 18, height: 18 }} />
          </div>
          <Tooltip label="Home" />
        </button>
      </div>

      {/* Nav */}
      <nav
        className="sidebar-nav"
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: 2,
          flex: 1,
          width: "100%",
        }}
      >
        {nav.map((n) => (
          <button
            key={n.id}
            className={`nav-item sidebar-tooltip-wrap${page === n.id ? " active" : ""}`}
            onClick={() => setPage(n.id)}
          >
            <span className="nav-icon">{n.icon}</span>
            <Tooltip label={n.label} />
          </button>
        ))}
      </nav>

      {/* Footer */}
      <div className="sidebar-footer">
        <button
          className="nav-item sidebar-tooltip-wrap"
          onClick={() => openBridge({ srcChainId: "initiation-2" })}
        >
          <span className="nav-icon">
            <Icons.bridge />
          </span>
          <Tooltip label="Bridge assets" />
        </button>
        <button
          className="nav-item sidebar-tooltip-wrap"
          onClick={onSwitchPersona}
        >
          <span className="nav-icon">
            <Icons.flip />
          </span>
          <Tooltip label={`Switch to ${other}`} />
        </button>

        {/* Bitcoin wallet */}
        {unisatInstalled && (
          <button
            className="sidebar-icon-action sidebar-tooltip-wrap"
            onClick={unisatAddress ? disconnectUnisat : connectUnisat}
            disabled={unisatConnecting}
            style={{ color: unisatAddress ? '#f7931a' : undefined }}
          >
            <Icons.wallet />
            <Tooltip label={unisatAddress ? 'Disconnect UniSat' : 'Connect UniSat'} />
          </button>
        )}

        {wallet ? (
          <div className="sidebar-mini-actions">
            <button
              className="sidebar-icon-action sidebar-tooltip-wrap"
              onClick={copyAddress}
            >
              {copied ? <Icons.check /> : <Icons.copy />}
              <Tooltip label={copied ? "Copied!" : "Copy address"} />
            </button>
            <button
              className="sidebar-icon-action sidebar-icon-danger sidebar-tooltip-wrap"
              onClick={() => disconnect()}
            >
              <Icons.power />
              <Tooltip label="Disconnect wallet" />
            </button>
          </div>
        ) : (
          <button
            className="sidebar-icon-action sidebar-tooltip-wrap"
            onClick={connect}
          >
            <Icons.wallet />
            <Tooltip label="Connect wallet" />
          </button>
        )}
      </div>
    </aside>
  );
}
