import React from "react";
import { TOKENS, CHAINS, fmtUSD } from "../../data";
import type { PreparedPayment, QuoteMode } from "../../types";
import { TokenLogo, ChainLogo } from "../shared/TokenLogo";
import { Icons } from "../shared/Icons";
import { useStrategies } from "../../hooks/useStrategies";
import { useQuote } from "../../hooks/useQuote";

interface Props {
  srcChain: string;
  srcToken: string;
  destChain: string;
  destTokenId: string;
  amount: string;
  setSrcChain: (v: string) => void;
  setSrcToken: (v: string) => void;
  setAmount: (v: string) => void;
  setShowPicker: (v: "src" | null) => void;
  usdValue: number;
  initAmount: number;
  feeUsd: number;
  onPay: (payment: PreparedPayment) => void;
  wallet: string | null;
  receiverAddress: string;
  setReceiverAddress: (v: string) => void;
  btcRefundAddress: string;
  setBtcRefundAddress: (v: string) => void;
  quoteMode?: QuoteMode;
  requestedDestinationAmount?: string | null;
  lockDestinationToken?: boolean;
  lockRecipientAddress?: boolean;
  onDestTokenChange?: (tokenId: string) => void;
  payDisabled?: boolean;
  payDisabledReason?: string;
  btcWalletConnected?: boolean;
}

function formatMinAmount(rawAmount: number, decimals: number): string {
  const human = rawAmount / Math.pow(10, decimals);
  if (human === 0) return "0";
  // Use enough significant digits so tiny values (e.g. 0.00001) are never shown as 0
  const sigFigs = Math.max(1, Math.ceil(-Math.log10(human)) + 2);
  return human.toLocaleString(undefined, {
    maximumSignificantDigits: Math.min(sigFigs, 8),
  });
}

function friendlyQuoteError(err: string): string {
  if (err.includes("insufficient destination liquidity"))
    return "No liquidity available for this route right now";
  if (err.includes("within")) {
    const m = err.match(/within (\d+) and (\d+)/);
    if (m)
      return `Amount out of range (min ${Number(
        m[1],
      ).toLocaleString()} – max ${Number(m[2]).toLocaleString()} base units)`;
  }
  if (err.includes("unknown asset")) return "This token pair is not supported";
  if (err.includes("no route")) return "No route found for this pair";
  return err.replace(/^(bad request|conflict): /i, "");
}

function toRawAmount(amount: string, decimals: number): string {
  const trimmed = amount.trim();
  const parsed = Number(trimmed);
  if (!trimmed || !Number.isFinite(parsed) || parsed <= 0) return "";
  return String(Math.round(parsed * Math.pow(10, decimals)));
}

export function PayCard({
  srcChain,
  srcToken,
  destChain,
  destTokenId,
  amount,
  setSrcChain: _setSrcChain,
  setSrcToken: _setSrcToken,
  setAmount,
  setShowPicker,
  usdValue,
  initAmount,
  feeUsd,
  onPay,
  wallet,
  receiverAddress,
  setReceiverAddress,
  btcRefundAddress,
  setBtcRefundAddress,
  quoteMode = "exact-in",
  requestedDestinationAmount,
  lockDestinationToken = false,
  lockRecipientAddress = false,
  onDestTokenChange,
  payDisabled,
  payDisabledReason,
  btcWalletConnected = false,
}: Props) {
  const [addressError, setAddressError] = React.useState(false);
  const [btcRefundError, setBtcRefundError] = React.useState(false);
  const [destMenuOpen, setDestMenuOpen] = React.useState(false);
  const destMenuRef = React.useRef<HTMLDivElement | null>(null);
  const isBitcoin = srcChain === "bitcoin_testnet";
  const isSolana = srcChain === "solana_devnet";
  const isNonEvm = isBitcoin || isSolana;
  const isExactOut = quoteMode === "exact-out";

  const handleDestTokenChange = (id: string) => {
    onDestTokenChange?.(id);
  };
  const srcChainInfo = CHAINS[srcChain] ?? { name: srcChain, short: srcChain };
  const destChainInfo = CHAINS[destChain] ?? { name: destChain };
  const destChainLabel = destChainInfo.name;
  const { strategies, getDestOptions, sourceOptions } = useStrategies();

  // Resolve source tokenId from strategies (matches by chain + display symbol)
  const srcTokenId =
    sourceOptions.find(
      (o) => o.chain === srcChain && o.displayToken === srcToken,
    )?.tokenId ?? srcToken.toLowerCase();

  // Destination tokens come straight from the live strategies feed.
  const destOptions = getDestOptions(srcChain, srcTokenId);
  // If current destTokenId is no longer valid for the new source, reset to first available
  React.useEffect(() => {
    if (
      !lockDestinationToken &&
      destOptions.length > 0 &&
      !destOptions.find((o) => o.tokenId === destTokenId)
    ) {
      handleDestTokenChange(destOptions[0].tokenId);
    }
  }, [srcChain, srcTokenId, destOptions, destTokenId, lockDestinationToken]);

  React.useEffect(() => {
    if (!destMenuOpen) return;

    const handlePointerDown = (event: MouseEvent | TouchEvent) => {
      if (!destMenuRef.current?.contains(event.target as Node)) {
        setDestMenuOpen(false);
      }
    };

    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setDestMenuOpen(false);
    };

    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("touchstart", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("touchstart", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [destMenuOpen]);

  const selectedDest = destOptions.find((o) => o.tokenId === destTokenId);
  const destDisplay =
    selectedDest?.displayToken ?? (destTokenId ? destTokenId.toUpperCase() : "");
  const strategy = strategies.find(
    (s) =>
      s.sourceChain === srcChain &&
      s.sourceTokenId === srcTokenId &&
      s.destTokenId === destTokenId,
  );
  const feeDisplay = strategy ? `${(strategy.fee / 100).toFixed(2)}%` : "—";

  // Compute raw fromAmount for quote (human amount * 10^decimals)
  const fromAmountRaw = React.useMemo(() => {
    if (!strategy) return "";
    return toRawAmount(amount, strategy.sourceDecimals);
  }, [amount, strategy]);
  const toAmountRaw = React.useMemo(() => {
    if (!strategy || !requestedDestinationAmount) return "";
    return toRawAmount(requestedDestinationAmount, strategy.destDecimals);
  }, [requestedDestinationAmount, strategy]);

  // Use asset identifiers from strategy for quote API calls
  const quoteFrom = strategy?.sourceAsset ?? "";
  const quoteTo = strategy?.destAsset ?? "";
  const quote = useQuote({
    from: quoteFrom,
    to: quoteTo,
    mode: quoteMode,
    fromAmount: fromAmountRaw,
    toAmount: toAmountRaw,
  });

  // Use live quote data when available, fall back to props
  const liveSourceAmount = isExactOut
    ? quote.sourceDisplay
      ? parseFloat(quote.sourceDisplay)
      : 0
    : parseFloat(amount) || 0;
  const liveDestAmount = isExactOut
    ? quote.destinationDisplay
      ? parseFloat(quote.destinationDisplay)
      : parseFloat(requestedDestinationAmount ?? "0") || 0
    : quote.destinationDisplay
    ? parseFloat(quote.destinationDisplay)
    : initAmount;
  const liveSourceUsdValue =
    quote.inputTokenPrice && liveSourceAmount
      ? liveSourceAmount * quote.inputTokenPrice
      : usdValue;
  const liveDestUsdValue =
    quote.outputTokenPrice && liveDestAmount
      ? liveDestAmount * quote.outputTokenPrice
      : liveDestAmount * (TOKENS[destDisplay]?.price || 0);
  const liveDestPrice =
    quote.outputTokenPrice || TOKENS[destDisplay]?.price || 0;

  const minReceived = quote.destinationDisplay
    ? `${quote.destinationDisplay} ${destDisplay}`
    : liveDestAmount
    ? `${liveDestAmount.toFixed(4)} ${destDisplay}`
    : "—";
  const quoteReady =
    !!strategy &&
    !!quote.sourceAmount &&
    !!quote.destinationAmount &&
    !quote.loading &&
    !quote.error;
  const sendAmountDisplay = isExactOut
    ? quote.loading
      ? "…"
      : quote.sourceDisplay || "0.0000"
    : amount;
  const receiveAmountDisplay = isExactOut
    ? quote.loading
      ? "…"
      : quote.destinationDisplay || requestedDestinationAmount || "0.0000"
    : quote.loading
    ? "…"
    : liveDestAmount
    ? liveDestAmount.toFixed(4)
    : "0.0000";

  const [addrOpen, setAddrOpen] = React.useState(false);

  const triggerPay = () => {
    if (payDisabled) return;
    if (!quoteReady || !strategy) return;
    const addr = receiverAddress.trim();
    if (!addr || addr.length < 10) {
      setAddressError(true);
      setAddrOpen(true);
      return;
    }
    if (isBitcoin && !btcRefundAddress.trim()) {
      setBtcRefundError(true);
      setAddrOpen(true);
      return;
    }
    onPay({
      quoteMode,
      sourceAmountRaw: quote.sourceAmount || fromAmountRaw,
      sourceAmountDisplay: quote.sourceDisplay || String(liveSourceAmount),
      destinationAmountRaw: quote.destinationAmount || toAmountRaw,
      destinationAmountDisplay:
        quote.destinationDisplay ||
        requestedDestinationAmount ||
        String(liveDestAmount),
      sourceAsset: strategy.sourceAsset,
      destinationAsset: strategy.destAsset,
      strategyId: quote.strategyId || strategy.id,
    });
  };

  const shortReceiver =
    receiverAddress.length >= 10
      ? `${receiverAddress.slice(0, 6)}…${receiverAddress.slice(-4)}`
      : "";
  const routeSourceLabel = srcChainInfo.name;
  const routeProtocolLabel = "Unipay";
  const routeDestinationLabel = destDisplay || "USDC";
  const routeLabel = `${routeSourceLabel} → ${routeProtocolLabel} → ${routeDestinationLabel}`;
  const canChooseDestToken = !lockDestinationToken && destOptions.length > 1;

  const ctaLabel =
    payDisabled && !wallet
      ? "Loading chain data…"
      : !receiverAddress
      ? "Enter merchant address"
      : isBitcoin && !btcWalletConnected && !btcRefundAddress.trim()
      ? "Enter BTC refund address"
      : !isExactOut && (!amount || parseFloat(amount) <= 0)
      ? "Enter amount"
      : isExactOut && !requestedDestinationAmount
      ? "Invalid invoice amount"
      : quote.loading
      ? "Fetching route…"
      : wallet || isNonEvm
      ? `Pay ${fmtUSD(liveSourceUsdValue)}`
      : `Connect wallet & pay ${fmtUSD(liveSourceUsdValue)}`;

  const ctaReady =
    !payDisabled &&
    !!receiverAddress &&
    (!isBitcoin || btcWalletConnected || !!btcRefundAddress.trim()) &&
    (isExactOut
      ? !!requestedDestinationAmount
      : !!amount && parseFloat(amount) > 0) &&
    quoteReady;

  return (
    <div className="pay-card-shell">
      <div className="swapcard">
        {/* SEND */}
        <div className="swap-box">
          <div className="swap-box-top">
            <span className="swap-box-label">
              {isExactOut ? "You send" : "Send"}
            </span>
            <span className="swap-box-sub tnum">
              ~{fmtUSD(liveSourceUsdValue)}
            </span>
          </div>
          <div className="swap-box-row">
            {isExactOut ? (
              <span
                className="tnum swap-amount-input as-readonly"
                style={{
                  color: quote.error ? "var(--text-3)" : "var(--text-0)",
                }}
              >
                {sendAmountDisplay}
              </span>
            ) : (
              <input
                className="tnum swap-amount-input"
                value={amount}
                onChange={(e) =>
                  setAmount(e.target.value.replace(/[^\d.]/g, ""))
                }
                inputMode="decimal"
                placeholder="0"
              />
            )}
            <button
              type="button"
              className="swap-token-chip swap-source-trigger"
              onClick={() => setShowPicker("src")}
              aria-label={`Choose source asset, currently ${srcToken} on ${srcChainInfo.name}`}
              aria-haspopup="dialog"
            >
              <TokenLogo sym={srcToken} size="sm" />
              <span style={{ fontWeight: 500 }}>{srcToken}</span>
              <Icons.chevron style={{ color: "var(--text-3)" }} />
            </button>
          </div>
          <div className="swap-box-bot">
            <span
              className="tnum"
              style={{
                color: "var(--text-3)",
                display: "inline-flex",
                alignItems: "center",
                gap: 4,
              }}
            >
              <ChainLogo id={srcChain} size="sm" /> on {srcChainInfo.name}
            </span>
            {strategy && (
              <span className="tnum" style={{ color: "var(--text-3)" }}>
                {`Min ${formatMinAmount(
                  Number(strategy.minAmount),
                  strategy.sourceDecimals,
                )}`}
              </span>
            )}
          </div>
        </div>

        {/* Swap handle */}
        <div className="swap-divider" aria-hidden>
          <div className="swap-divider-btn">
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
              <path
                d="M3 1v8M3 9l-2-2M3 9l2-2M9 11V3M9 3L7 5M9 3l2 2"
                stroke="currentColor"
                strokeWidth="1.4"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </div>
        </div>

        {/* RECEIVE */}
        <div className="swap-box">
          <div className="swap-box-top">
            <span className="swap-box-label">
              {isExactOut ? "Merchant receives" : "Receive"}
            </span>
            <span className="swap-box-sub tnum">
              ~{fmtUSD(liveDestUsdValue)}
            </span>
            <span className="swap-box-timer tnum">
              <svg
                width="11"
                height="11"
                viewBox="0 0 12 12"
                fill="none"
                style={{ marginRight: 3 }}
              >
                <circle
                  cx="6"
                  cy="7"
                  r="4"
                  stroke="currentColor"
                  strokeWidth="1.2"
                />
                <path
                  d="M6 5V7L7.2 8"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                />
                <path
                  d="M5 1h2"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                />
              </svg>
              ~8s
            </span>
          </div>
          <div className="swap-box-row">
            <span
              className="tnum swap-amount-input as-readonly"
              style={{ color: quote.error ? "var(--text-3)" : "var(--text-0)" }}
            >
              {receiveAmountDisplay}
            </span>
            <div className="swap-token-select" ref={destMenuRef}>
              <button
                type="button"
                className={`swap-token-chip swap-token-trigger ${
                  destMenuOpen ? "is-open" : ""
                } ${canChooseDestToken ? "" : "is-static"}`}
                onClick={() => {
                  if (!canChooseDestToken) return;
                  setDestMenuOpen((open) => !open);
                }}
                aria-haspopup={canChooseDestToken ? "listbox" : undefined}
                aria-expanded={canChooseDestToken ? destMenuOpen : undefined}
              >
                <span className="swap-token-trigger-main">
                  <TokenLogo sym={destDisplay} size="sm" />
                  <span className="swap-token-trigger-label">
                    {destDisplay}
                  </span>
                </span>
                {canChooseDestToken && (
                  <Icons.chevron
                    className={`swap-token-caret ${
                      destMenuOpen ? "is-open" : ""
                    }`}
                  />
                )}
              </button>

              {canChooseDestToken && destMenuOpen && (
                <div
                  className="swap-token-menu"
                  role="listbox"
                  aria-label="Receive token"
                >
                  {destOptions.map((opt) => {
                    const isActive = opt.tokenId === destTokenId;
                    return (
                      <button
                        key={opt.tokenId}
                        type="button"
                        className={`swap-token-option ${
                          isActive ? "is-active" : ""
                        }`}
                        onClick={() => {
                          handleDestTokenChange(opt.tokenId);
                          setDestMenuOpen(false);
                        }}
                        role="option"
                        aria-selected={isActive}
                      >
                        <span className="swap-token-option-main">
                          <TokenLogo sym={opt.displayToken} size="sm" />
                          <span className="swap-token-option-label">
                            {opt.displayToken}
                          </span>
                        </span>
                        {isActive && (
                          <Icons.check className="swap-token-option-check" />
                        )}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          </div>
          <div className="swap-box-bot">
            <span
              className="tnum"
              style={{
                color: "var(--text-3)",
                display: "inline-flex",
                alignItems: "center",
                gap: 4,
              }}
            >
              <ChainLogo id={destChain} size="sm" /> on {destChainLabel}
            </span>
            {liveDestPrice > 0 && (
              <span className="tnum" style={{ color: "var(--accent)" }}>
                1 {destDisplay} = {fmtUSD(liveDestPrice)}
              </span>
            )}
          </div>
        </div>

        {/* Details table */}
        <div className="swap-details">
          <DetailRow
            label="Rate"
            value={
              strategy
                ? `1 ${srcToken} ≈ ${fmtUSD(
                    quote.inputTokenPrice || TOKENS[srcToken]?.price || 0,
                  )}`
                : "—"
            }
          />
          <DetailRow label="Protocol fee" value={fmtUSD(feeUsd)} />
          <DetailRow label="Slippage" value={feeDisplay} />
          <DetailRow
            label={isExactOut ? "Invoice payout" : "Minimum received"}
            value={minReceived}
          />
          <DetailRow label="Route" value={routeLabel} />
          <DetailRow
            label="Merchant address"
            value={
              receiverAddress ? (
                <span style={{ color: "var(--text-0)" }}>{shortReceiver}</span>
              ) : !lockRecipientAddress ? (
                <button
                  onClick={() => setAddrOpen((v) => !v)}
                  style={{
                    color: "var(--accent)",
                    background: "none",
                    border: "none",
                    padding: 0,
                    cursor: "pointer",
                    font: "inherit",
                  }}
                >
                  Set address →
                </button>
              ) : (
                "—"
              )
            }
          />
          {isBitcoin && !btcWalletConnected && (
            <DetailRow
              label="BTC refund address"
              value={
                btcRefundAddress ? (
                  <span style={{ color: 'var(--text-0)' }}>
                    {btcRefundAddress.slice(0, 6)}…{btcRefundAddress.slice(-4)}
                  </span>
                ) : (
                  <button
                    onClick={() => setAddrOpen((v) => !v)}
                    style={{ color: 'var(--accent)', background: 'none', border: 'none', padding: 0, cursor: 'pointer', font: 'inherit' }}
                  >
                    Set address →
                  </button>
                )
              }
            />
          )}
        </div>

        {/* Address inputs, revealed inline */}
        {addrOpen && (
          <div
            style={{
              marginTop: 12,
              display: "flex",
              flexDirection: "column",
              gap: 10,
            }}
          >
            {!lockRecipientAddress && (
              <Field
                label="Merchant address (receiver on Initia)"
                required
                value={receiverAddress}
                onChange={(v) => {
                  setReceiverAddress(v.trim());
                  setAddressError(false);
                }}
                placeholder="init1… or 0x… address"
                error={addressError}
              />
            )}
            {isBitcoin && !btcWalletConnected && (
              <Field
                label="Your Bitcoin refund address"
                required
                value={btcRefundAddress}
                onChange={(v) => {
                  setBtcRefundAddress(v.trim());
                  setBtcRefundError(false);
                }}
                placeholder="Bitcoin address (tb1…, 2…, m/n…)"
                error={btcRefundError}
              />
            )}
          </div>
        )}

        {quote.error && !quote.loading && (
          <div
            style={{
              marginTop: 10,
              padding: "8px 12px",
              background: "rgba(251,146,60,0.08)",
              border: "1px solid rgba(251,146,60,0.2)",
              borderRadius: 8,
              fontSize: 12,
              color: "#fb923c",
              display: "flex",
              alignItems: "flex-start",
              gap: 6,
            }}
          >
            <span style={{ flexShrink: 0 }}>⚠</span>
            <span>{friendlyQuoteError(quote.error)}</span>
          </div>
        )}

        {payDisabledReason && (
          <div
            style={{
              marginTop: 10,
              padding: "8px 12px",
              background: "rgba(248,113,113,0.08)",
              border: "1px solid rgba(248,113,113,0.2)",
              borderRadius: 8,
              fontSize: 12,
              color: "#f87171",
              display: "flex",
              alignItems: "center",
              gap: 6,
            }}
          >
            <span>⚠</span> {payDisabledReason}
          </div>
        )}

        {/* CTA */}
        <button
          className={`swap-cta ${ctaReady ? "swap-cta-primary" : ""}`}
          onClick={() => {
            if (!receiverAddress || (isBitcoin && !btcWalletConnected && !btcRefundAddress.trim())) {
              setAddrOpen(true);
              return;
            }
            triggerPay();
          }}
          disabled={payDisabled}
        >
          {!wallet && !isNonEvm && receiverAddress && amount && (
            <Icons.wallet />
          )}
          {ctaLabel}
        </button>
      </div>

      {/* Footer route strip */}
      <div
        style={{
          marginTop: 18,
          display: "flex",
          justifyContent: "center",
          alignItems: "center",
          gap: 10,
          color: "var(--text-3)",
          fontSize: 11,
          flexWrap: "wrap",
        }}
      >
        <span style={{ textTransform: "uppercase", letterSpacing: "0.1em" }}>
          Route
        </span>
        <span className="route-pill">{routeSourceLabel}</span>
        <Icons.arrow />
        <span className="route-pill">{routeProtocolLabel}</span>
        <Icons.arrow />
        <span className="route-pill route-pill-accent">
          {routeDestinationLabel}
        </span>
      </div>
    </div>
  );
}

function DetailRow({
  label,
  value,
}: {
  label: string;
  value: React.ReactNode;
}) {
  return (
    <div className="swap-detail-row">
      <span className="swap-detail-label">{label}</span>
      <span className="swap-detail-value tnum">{value}</span>
    </div>
  );
}

function Field({
  label,
  required,
  value,
  onChange,
  placeholder,
  error,
}: {
  label: string;
  required?: boolean;
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  error?: boolean;
}) {
  return (
    <div>
      <div className="field-label">
        {label}
        {required && <span style={{ color: "#f87171", marginLeft: 4 }}>*</span>}
      </div>
      <input
        className="input mono"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder={placeholder}
        style={{
          marginTop: 6,
          padding: "12px 14px",
          fontSize: 13.5,
          width: "100%",
          boxSizing: "border-box",
          borderColor: error ? "#f87171" : undefined,
        }}
      />
    </div>
  );
}

export function TokenChainPicker({
  currentChain,
  onClose,
  onPick,
  requiredDestination,
}: {
  currentChain?: string;
  onClose: () => void;
  onPick: (c: string, t: string) => void;
  requiredDestination?: { chain: string; tokenId: string } | null;
}) {
  const { sourceOptions, strategies, loading, error } = useStrategies();
  const [assetSearch, setAssetSearch] = React.useState("");
  const [chainSearch, setChainSearch] = React.useState("");
  const [activeChainSelection, setActiveChainSelection] = React.useState<
    string | null
  >(currentChain ?? null);
  const [view, setView] = React.useState<"assets" | "chains">("assets");
  const dialogTitleId = React.useId();
  const sourcePriority = React.useMemo(
    () =>
      new Map([
        ["bitcoin_testnet", 0],
        ["base_sepolia", 1],
        ["solana_devnet", 2],
        ["arbitrum_sepolia", 3],
        ["ethereum_sepolia", 4],
        ["optimism_sepolia", 5],
      ]),
    [],
  );

  React.useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [onClose]);

  const pickerChainName = React.useCallback(
    (chain: string, fallback: string) => {
      const raw = (CHAINS[chain]?.name ?? fallback ?? chain).trim();
      const stripped = raw
        .replace(/\s+\([^)]*\)$/g, "")
        .replace(/\s+(Sepolia|Testnet|Devnet)$/i, "")
        .trim();

      if (/^Bnb chain$/i.test(stripped)) return "BNB Chain";
      if (/^Hyperevm$/i.test(stripped)) return "HyperEVM";
      return stripped;
    },
    [],
  );

  // Show every source with a live strategy. If an invoice locks the destination,
  // restrict to strategies that match it; otherwise trust the strategies feed.
  const tarsOptions = sourceOptions.filter((opt) =>
    strategies.some(
      (s) =>
        s.sourceChain === opt.chain &&
        s.sourceTokenId === opt.tokenId &&
        (!requiredDestination?.chain || s.destChain === requiredDestination.chain) &&
        (!requiredDestination?.tokenId ||
          s.destTokenId === requiredDestination.tokenId),
    ),
  );

  // Unique chains for filter chips
  const uniqueChains = React.useMemo(() => {
    const seen = new Set<string>();
    return tarsOptions
      .filter((opt) => {
        if (seen.has(opt.chain)) return false;
        seen.add(opt.chain);
        return true;
      })
      .sort((a, b) => {
        const rankA = sourcePriority.get(a.chain) ?? 999;
        const rankB = sourcePriority.get(b.chain) ?? 999;
        return rankA - rankB;
      })
      .map((opt) => ({
        chain: opt.chain,
        displayChain: pickerChainName(opt.chain, opt.displayChain),
      }));
  }, [pickerChainName, sourcePriority, tarsOptions]);

  const activeChain = uniqueChains.some(
    ({ chain }) => chain === activeChainSelection,
  )
    ? activeChainSelection
    : uniqueChains.some(({ chain }) => chain === currentChain)
    ? currentChain ?? null
    : uniqueChains[0]?.chain ?? null;

  const primaryChains = React.useMemo(() => {
    if (uniqueChains.length <= 7) return uniqueChains;

    const head = uniqueChains.slice(0, 7);
    if (activeChain && !head.some(({ chain }) => chain === activeChain)) {
      const active = uniqueChains.find(({ chain }) => chain === activeChain);
      if (active) {
        return [...head.slice(0, 6), active];
      }
    }

    return head;
  }, [activeChain, uniqueChains]);

  const hiddenChains = React.useMemo(
    () =>
      uniqueChains.filter(
        ({ chain }) =>
          !primaryChains.some((candidate) => candidate.chain === chain),
      ),
    [primaryChains, uniqueChains],
  );

  const filteredChains = React.useMemo(() => {
    const query = chainSearch.trim().toLowerCase();
    if (!query) return uniqueChains;

    return uniqueChains.filter(
      ({ chain, displayChain }) =>
        displayChain.toLowerCase().includes(query) ||
        chain.toLowerCase().includes(query),
    );
  }, [chainSearch, uniqueChains]);

  const filtered = React.useMemo(() => {
    const query = assetSearch.trim().toLowerCase();
    const scopedChain = query ? null : activeChain;

    return tarsOptions
      .filter((opt) => {
        if (scopedChain && opt.chain !== scopedChain) return false;
        if (!query) return true;

        const displayChain = pickerChainName(opt.chain, opt.displayChain);
        return (
          opt.displayToken.toLowerCase().includes(query) ||
          opt.tokenId.toLowerCase().includes(query) ||
          opt.assetDisplayName.toLowerCase().includes(query) ||
          displayChain.toLowerCase().includes(query) ||
          opt.chain.toLowerCase().includes(query)
        );
      })
      .sort((a, b) => {
        const rankA = sourcePriority.get(a.chain) ?? 999;
        const rankB = sourcePriority.get(b.chain) ?? 999;
        if (rankA !== rankB) return rankA - rankB;
        return a.assetDisplayName.localeCompare(b.assetDisplayName);
      });
  }, [activeChain, assetSearch, pickerChainName, sourcePriority, tarsOptions]);

  const sections = React.useMemo(() => {
    const grouped = new Map<
      string,
      {
        chain: string;
        displayChain: string;
        options: Array<typeof filtered[number]>;
      }
    >();

    for (const opt of filtered) {
      const section = grouped.get(opt.chain);
      if (section) {
        section.options.push(opt);
        continue;
      }

      grouped.set(opt.chain, {
        chain: opt.chain,
        displayChain: pickerChainName(opt.chain, opt.displayChain),
        options: [opt],
      });
    }

    return Array.from(grouped.values())
      .sort((a, b) => {
        const rankA = sourcePriority.get(a.chain) ?? 999;
        const rankB = sourcePriority.get(b.chain) ?? 999;
        return rankA - rankB;
      })
      .map((section) => ({
        ...section,
        options: [...section.options].sort((a, b) =>
          a.assetDisplayName.localeCompare(b.assetDisplayName),
        ),
      }));
  }, [filtered, pickerChainName, sourcePriority]);

  const backToAssets = () => {
    setView("assets");
    setChainSearch("");
  };

  return (
    <div className="modal-backdrop source-picker-backdrop" onClick={onClose}>
      <div className="source-picker-shell" onClick={(e) => e.stopPropagation()}>
        <div
          className="source-picker"
          role="dialog"
          aria-modal="true"
          aria-labelledby={dialogTitleId}
        >
          {view === "chains" ? (
            <>
              <div className="source-picker-header source-picker-header-nested">
                <div className="source-picker-title" id={dialogTitleId}>
                  Select chain
                </div>
                <button
                  type="button"
                  className="source-picker-close source-picker-back"
                  onClick={backToAssets}
                  aria-label="Back to token list"
                >
                  <svg
                    width="22"
                    height="22"
                    viewBox="0 0 24 24"
                    fill="none"
                    aria-hidden="true"
                  >
                    <path
                      d="M19 12H5M12 19L5 12L12 5"
                      stroke="currentColor"
                      strokeWidth="2.2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    />
                  </svg>
                </button>
              </div>

              <div className="source-picker-search-wrap">
                <input
                  className="input source-picker-search"
                  placeholder="Search chains"
                  value={chainSearch}
                  onChange={(e) => setChainSearch(e.target.value)}
                  autoFocus
                />
                <span className="source-picker-search-icon" aria-hidden="true">
                  <svg width="26" height="26" viewBox="0 0 24 24" fill="none">
                    <circle
                      cx="11"
                      cy="11"
                      r="6.8"
                      stroke="currentColor"
                      strokeWidth="2.2"
                    />
                    <path
                      d="M16.2 16.2L20 20"
                      stroke="currentColor"
                      strokeWidth="2.2"
                      strokeLinecap="round"
                    />
                  </svg>
                </span>
              </div>

              <div className="source-picker-chain-panel">
                {loading && (
                  <div className="source-picker-status">Loading chains…</div>
                )}
                {error && (
                  <div className="source-picker-status source-picker-status-error">
                    Failed to load: {error}
                  </div>
                )}

                {!loading &&
                  !error &&
                  filteredChains.map(({ chain, displayChain }) => (
                    <button
                      key={chain}
                      type="button"
                      className={`source-picker-chain-option${
                        activeChain === chain ? " is-active" : ""
                      }`}
                      onClick={() => {
                        setActiveChainSelection(chain);
                        backToAssets();
                      }}
                    >
                      <span className="source-picker-chain-option-main">
                        <ChainLogo id={chain} size="md" />
                        <span className="source-picker-chain-option-label">
                          {displayChain}
                        </span>
                      </span>
                      {activeChain === chain && (
                        <Icons.check className="source-picker-chain-option-check" />
                      )}
                    </button>
                  ))}

                {!loading && !error && filteredChains.length === 0 && (
                  <div className="source-picker-status">No matching chains</div>
                )}
              </div>
            </>
          ) : (
            <>
              <div className="source-picker-header">
                <div className="source-picker-title" id={dialogTitleId}>
                  Select token to send
                </div>
                <button
                  type="button"
                  className="source-picker-close"
                  onClick={onClose}
                  aria-label="Close source picker"
                >
                  <Icons.x />
                </button>
              </div>

              {primaryChains.length > 0 && (
                <div className="source-picker-chips">
                  {primaryChains.map(({ chain, displayChain }) => (
                    <button
                      key={chain}
                      type="button"
                      className={`source-picker-chip${
                        activeChain === chain ? " is-active" : ""
                      }`}
                      onClick={() => setActiveChainSelection(chain)}
                      title={displayChain}
                    >
                      <span className="source-picker-chip-icon">
                        <ChainLogo id={chain} size="md" />
                      </span>
                    </button>
                  ))}
                  {hiddenChains.length > 0 && (
                    <button
                      type="button"
                      className="source-picker-chip source-picker-chip-more"
                      onClick={() => setView("chains")}
                      aria-label="Browse all chains"
                    >
                      <span className="source-picker-chip-more-label">
                        +{hiddenChains.length}
                      </span>
                    </button>
                  )}
                </div>
              )}

              <div className="source-picker-search-wrap">
                <input
                  className="input source-picker-search"
                  placeholder="Search assets or chains"
                  value={assetSearch}
                  onChange={(e) => setAssetSearch(e.target.value)}
                  autoFocus
                />
                <span className="source-picker-search-icon" aria-hidden="true">
                  <svg width="26" height="26" viewBox="0 0 24 24" fill="none">
                    <circle
                      cx="11"
                      cy="11"
                      r="6.8"
                      stroke="currentColor"
                      strokeWidth="2.2"
                    />
                    <path
                      d="M16.2 16.2L20 20"
                      stroke="currentColor"
                      strokeWidth="2.2"
                      strokeLinecap="round"
                    />
                  </svg>
                </span>
              </div>

              <div className="source-picker-list">
                {loading && (
                  <div className="source-picker-status">
                    Loading strategies…
                  </div>
                )}
                {error && (
                  <div className="source-picker-status source-picker-status-error">
                    Failed to load: {error}
                  </div>
                )}

                {!loading &&
                  !error &&
                  sections.map((section) => (
                    <section
                      key={section.chain}
                      className="source-picker-section"
                    >
                      <div className="source-picker-section-title">
                        Assets on {section.displayChain}
                      </div>
                      <div className="source-picker-section-body">
                        {section.options.map((opt) => {
                          const strategy = strategies.find(
                            (s) =>
                              s.sourceChain === opt.chain &&
                              s.sourceTokenId === opt.tokenId,
                          );
                          const minHuman = strategy
                            ? formatMinAmount(
                                Number(strategy.minAmount),
                                strategy.sourceDecimals,
                              )
                            : null;
                          const assetName =
                            TOKENS[opt.displayToken]?.name ?? opt.displayToken;

                          return (
                            <button
                              key={`${opt.chain}:${opt.tokenId}`}
                              type="button"
                              className="source-picker-asset"
                              onClick={() =>
                                onPick(opt.chain, opt.displayToken)
                              }
                            >
                              <span className="source-picker-asset-main">
                                <span className="source-picker-asset-icon">
                                  {opt.icon ? (
                                    <img
                                      src={opt.icon}
                                      alt={opt.displayToken}
                                      width={48}
                                      height={48}
                                      className="source-picker-option-icon"
                                    />
                                  ) : (
                                    <TokenLogo
                                      sym={opt.displayToken}
                                      size="lg"
                                      chain={opt.chain}
                                    />
                                  )}
                                </span>
                                <span className="source-picker-asset-copy">
                                  <span className="source-picker-asset-title">
                                    {assetName}
                                  </span>
                                  <span className="source-picker-asset-subtitle">
                                    {section.displayChain}
                                  </span>
                                </span>
                              </span>
                              <span className="source-picker-asset-meta">
                                <span className="source-picker-asset-symbol">
                                  {opt.displayToken}
                                </span>
                                {minHuman && (
                                  <span className="mono source-picker-asset-min">
                                    Min {minHuman}
                                  </span>
                                )}
                              </span>
                            </button>
                          );
                        })}
                      </div>
                    </section>
                  ))}

                {!loading && !error && sections.length === 0 && (
                  <div className="source-picker-status">No matching assets</div>
                )}
              </div>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
