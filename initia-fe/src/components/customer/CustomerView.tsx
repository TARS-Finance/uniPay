import { useEffect, useMemo, useRef, useState } from 'react';
import { QRCodeSVG } from 'qrcode.react';
import { CHAINS, fmtUSD, relTime, shortAddr } from '../../data';
import type { HistoryTx, PaymentInvoice, PreparedPayment, QuoteMode } from '../../types';
import { Icons } from '../shared/Icons';
import { PayCard, TokenChainPicker } from './PayCard';
import { PayProgress, StepRow } from './PayProgress';
import { HashLink } from '../shared/HashLink';
import { TokenLogo, ChainLogo } from '../shared/TokenLogo';
import { useSwap } from '../../hooks/useSwap';
import { useChains } from '../../hooks/useChains';
import { useBitcoinSwap } from '../../hooks/useBitcoinSwap';
import { useSolanaSwap } from '../../hooks/useSolanaSwap';
import { useUnisat } from '../../hooks/useUnisat';
import { useStrategies } from '../../hooks/useStrategies';
import { useWallet } from '../../lib/wallet-context';
import { parsePaymentInvoiceFromInput } from '../../lib/invoice';
import { useOrders } from '../../hooks/useOrders';
import { TxDetail } from './TxDetail';

interface Props {
  wallet: string | null;
  page: 'pay' | 'history';
  setPage: (p: 'pay' | 'history') => void;
  invoicePrefill?: PaymentInvoice | null;
}

function formatRawAmount(rawAmount: string | null | undefined, decimals: number): string {
  if (!rawAmount) return '';
  const scale = Math.pow(10, decimals);
  const human = Number(rawAmount) / scale;
  if (!Number.isFinite(human) || human <= 0) return '';
  const precision = decimals > 6 ? 8 : 6;
  return human.toLocaleString('en-US', {
    minimumFractionDigits: 0,
    maximumFractionDigits: precision,
    useGrouping: false,
  });
}

export function CustomerView({ wallet, page, setPage: _setPage, invoicePrefill = null }: Props) {
  const { connect } = useWallet();
  const [srcChain, setSrcChain] = useState(() => {
    try {
      const pending = localStorage.getItem('btc.pending_order');
      if (pending) { JSON.parse(pending); return 'bitcoin_testnet'; }
    } catch { /* ignore */ }
    return 'base_sepolia';
  });
  const [srcToken, setSrcToken] = useState(() => {
    try {
      const pending = localStorage.getItem('btc.pending_order');
      if (pending) { JSON.parse(pending); return 'BTC'; }
    } catch { /* ignore */ }
    return 'USDC';
  });
  const [amount, setAmount] = useState('10');
  const [destTokenId, setDestTokenId] = useState('');
  const [receiverAddress, setReceiverAddress] = useState('');
  const [btcRefundAddress, setBtcRefundAddress] = useState('');

  const [showPicker, setShowPicker] = useState<'src' | null>(null);
  const [pendingPayment, setPendingPayment] = useState<PreparedPayment | null>(null);
  const [activePayment, setActivePayment] = useState<PreparedPayment | null>(null);
  const [activeInvoice, setActiveInvoice] = useState<PaymentInvoice | null>(invoicePrefill);
  const [invoiceInput, setInvoiceInput] = useState('');
  const [invoiceInputError, setInvoiceInputError] = useState('');
  const [selectedTxId, setSelectedTxId] = useState<string | null>(null);

  const { getChainConfig, loading: chainsLoading, error: chainsError } = useChains();
  const { strategies, getDestOptions, sourceOptions } = useStrategies();
  const [copiedDeposit, setCopiedDeposit] = useState(false);
  const [resumeOrder, setResumeOrder] = useState<{ depositAddress: string; amountStr: string; token: string; destToken: string } | null>(null);
  const appliedInvoiceKeyRef = useRef<string | null>(null);

  const invoiceKey = useMemo(() => {
    if (!invoicePrefill) return null;
    return [
      invoicePrefill.version,
      invoicePrefill.recipient,
      invoicePrefill.destChain,
      invoicePrefill.destTokenId,
      invoicePrefill.destAmount,
    ].join(':');
  }, [invoicePrefill]);

  const swap = useSwap();
  const btcSwap = useBitcoinSwap();
  const solSwap = useSolanaSwap();
  const { unisatAddress, sendBitcoin: unisatSendBitcoin } = useUnisat();

  // Auto-fill BTC refund address from UniSat when Bitcoin source is selected
  useEffect(() => {
    if (srcChain === 'bitcoin_testnet' && unisatAddress) {
      setBtcRefundAddress(unisatAddress);
    } else if (srcChain !== 'bitcoin_testnet') {
      setBtcRefundAddress((prev) => (prev === unisatAddress ? '' : prev));
    }
  }, [srcChain, unisatAddress]);

  const isBitcoin = srcChain === 'bitcoin_testnet';
  const isSolana = srcChain === 'solana_devnet';
  const quoteMode: QuoteMode = activeInvoice ? 'exact-out' : 'exact-in';

  const isActive = isBitcoin
    ? (btcSwap.step !== 'idle' && btcSwap.step !== 'error') || btcSwap.depositAddress !== null
    : isSolana
    ? (solSwap.step !== 'idle' && solSwap.step !== 'error')
    : (swap.step !== 'idle' && swap.step !== 'error');
  const isDone = isBitcoin
    ? btcSwap.step === 'fulfilled' || btcSwap.step === 'done'
    : isSolana
    ? solSwap.step === 'fulfilled'
    : swap.step === 'done';

  const {
    orders,
    loading: ordersLoading,
    refreshing: ordersRefreshing,
    refetch: refetchOrders,
  } = useOrders(wallet);

  // Resolve source tokenId from strategy metadata
  const srcTokenId = sourceOptions.find((o) => o.chain === srcChain && o.displayToken === srcToken)?.tokenId
    ?? srcToken.toLowerCase();

  useEffect(() => {
    if (!invoiceKey || !invoicePrefill) return;
    if (invoiceKey === appliedInvoiceKeyRef.current) return;
    appliedInvoiceKeyRef.current = invoiceKey;
    setActiveInvoice(invoicePrefill);
  }, [invoiceKey, invoicePrefill]);

  const invoiceSupportedSources = useMemo(() => {
    if (!activeInvoice) return [];
    const priority = new Map([
      ['bitcoin_testnet', 0],
      ['base_sepolia', 1],
      ['solana_devnet', 2],
      ['arbitrum_sepolia', 3],
      ['ethereum_sepolia', 4],
      ['optimism_sepolia', 5],
    ]);

    return sourceOptions
      .filter((opt) =>
        getDestOptions(opt.chain, opt.tokenId).some(
          (dest) => dest.chain === activeInvoice.destChain && dest.tokenId === activeInvoice.destTokenId,
        ),
      )
      .sort((a, b) => {
        const rankA = priority.get(a.chain) ?? 999;
        const rankB = priority.get(b.chain) ?? 999;
        if (rankA !== rankB) return rankA - rankB;
        return a.assetDisplayName.localeCompare(b.assetDisplayName);
      });
  }, [activeInvoice, getDestOptions, sourceOptions]);

  useEffect(() => {
    if (!activeInvoice) return;
    setReceiverAddress(activeInvoice.recipient);
    setDestTokenId(activeInvoice.destTokenId);
  }, [activeInvoice]);

  useEffect(() => {
    if (!activeInvoice || invoiceSupportedSources.length === 0) return;
    const currentSourceSupported = invoiceSupportedSources.some(
      (opt) => opt.chain === srcChain && opt.tokenId === srcTokenId,
    );
    if (currentSourceSupported) return;
    const nextSource = invoiceSupportedSources[0];
    setSrcChain(nextSource.chain);
    setSrcToken(nextSource.displayToken);
  }, [activeInvoice, invoiceSupportedSources, srcChain, srcTokenId]);

  const chainConfig = !isBitcoin && !isSolana ? getChainConfig(srcChain, srcToken.toLowerCase()) : undefined;

  // Decimal counts come from chain config (EVM), Bitcoin (8) or SOL (9)
  const srcDecimals: number = isBitcoin ? 8 : isSolana ? 9 : (chainConfig?.tokenDecimals ?? 6);

  // Destination token info from strategy — live data drives this; the destination
  // chain is whatever the strategies API reports (no hardcoded rollup id).
  const destStrategy = strategies.find(
    (s) => s.sourceChain === srcChain && s.sourceTokenId === srcTokenId && s.destTokenId === destTokenId,
  );
  const destOpts = getDestOptions(srcChain, srcTokenId);
  const invoiceDestStrategy = activeInvoice
    ? strategies.find((strategy) => strategy.destChain === activeInvoice.destChain && strategy.destTokenId === activeInvoice.destTokenId)
    : undefined;
  const selectedDestChain = activeInvoice?.destChain
    ?? destStrategy?.destChain
    ?? destOpts.find((o) => o.tokenId === destTokenId)?.chain
    ?? destOpts[0]?.chain
    ?? '';
  const destTokenDisplay = destStrategy?.destDisplaySymbol
    ?? invoiceDestStrategy?.destDisplaySymbol
    ?? destOpts.find((o) => o.tokenId === destTokenId)?.displayToken
    ?? (destTokenId ? destTokenId.toUpperCase() : '');
  const destDecimals = destStrategy?.destDecimals ?? invoiceDestStrategy?.destDecimals ?? 6;

  const amountNum = parseFloat(amount) || 0;
  const payDisabled = !isBitcoin && !isSolana && !chainsLoading && !chainConfig;
  const progressSourceAmount = parseFloat(activePayment?.sourceAmountDisplay ?? amount) || 0;
  const progressDestAmount = parseFloat(activePayment?.destinationAmountDisplay ?? (activeInvoice?.destAmount ?? '0')) || 0;
  const bitcoinAmountDisplay = formatRawAmount(btcSwap.sourceAmountSats, 8)
    || activePayment?.sourceAmountDisplay
    || amount;

  const payDisabledReason = chainsError
    ? 'Could not load chain data from quote service — is it running?'
    : payDisabled
    ? `No chain config found for ${srcToken} on ${srcChain}`
    : undefined;

  // Reset stale swap state when navigating to pay page with no active payment
  useEffect(() => {
    if (page !== 'pay' || activePayment) return;
    if (swap.step !== 'idle' && swap.step !== 'error') swap.reset();
    if (
      btcSwap.step !== 'idle' &&
      btcSwap.step !== 'error' &&
      btcSwap.step !== 'fulfilled' &&
      btcSwap.step !== 'done' &&
      btcSwap.step !== 'sending'
    ) btcSwap.reset();
    if (solSwap.step !== 'idle' && solSwap.step !== 'error' && solSwap.step !== 'fulfilled') solSwap.reset();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page]);

  // When leaving the pay page, wipe all pending orders from storage so they
  // never re-surface on the next visit.
  useEffect(() => {
    if (page === 'pay') return;
    localStorage.removeItem('btc.pending_order');
    localStorage.removeItem('evm.pending_order');
    localStorage.removeItem('sol.pending_order');
  }, [page]);

  // auto-start EVM swap once wallet is connected after a pending connect request
  useEffect(() => {
    if (wallet && pendingPayment && swap.step === 'idle' && !isBitcoin) {
      if (!chainConfig) return;
      setActivePayment(pendingPayment);
      swap.startSwap({
        chainConfig,
        quoteMode: pendingPayment.quoteMode,
        sourceAmountRaw: pendingPayment.sourceAmountRaw,
        destinationAmountRaw: pendingPayment.destinationAmountRaw,
        receiverAddress,
        from: pendingPayment.sourceAsset,
        to: pendingPayment.destinationAsset,
        strategyId: pendingPayment.strategyId,
      });
      setPendingPayment(null);
    }
  }, [wallet, pendingPayment, swap.step, swap.startSwap, chainConfig, receiverAddress, isBitcoin]);

  const onPay = (payment: PreparedPayment) => {
    setResumeOrder(null);
    if (isBitcoin) {
      setActivePayment(payment);
      btcSwap.startBitcoinSwap({
        quoteMode: payment.quoteMode,
        sourceAmountRaw: payment.sourceAmountRaw,
        destinationAmountRaw: payment.destinationAmountRaw,
        destinationAsset: payment.destinationAsset,
        receiverAddress,
        sourceAsset: payment.sourceAsset,
        btcRefundAddress,
        strategyId: payment.strategyId,
        unisatSendBitcoin: unisatAddress ? unisatSendBitcoin : undefined,
      });
      return;
    }
    if (isSolana) {
      setActivePayment(payment);
      solSwap.startSolanaSwap({
        quoteMode: payment.quoteMode,
        sourceAmountRaw: payment.sourceAmountRaw,
        destinationAmountRaw: payment.destinationAmountRaw,
        sourceAssetId: payment.sourceAsset,
        destinationAssetId: payment.destinationAsset,
        merchantInitiaAddress: receiverAddress,
        strategyId: payment.strategyId,
      });
      return;
    }
    if (!chainConfig) return;
    if (!wallet) {
      setPendingPayment(payment);
      connect();
      return;
    }
    setActivePayment(payment);
    swap.startSwap({
      chainConfig,
      quoteMode: payment.quoteMode,
      sourceAmountRaw: payment.sourceAmountRaw,
      destinationAmountRaw: payment.destinationAmountRaw,
      receiverAddress,
      from: payment.sourceAsset,
      to: payment.destinationAsset,
      strategyId: payment.strategyId,
    });
  };

  const reset = () => {
    if (isDone) refetchOrders();
    if (isBitcoin) {
      btcSwap.reset();
    } else if (isSolana) {
      solSwap.reset();
    } else {
      swap.reset();
    }
    setPendingPayment(null);
    setActivePayment(null);
    setResumeOrder(null);
  };

  const onResume = (tx: HistoryTx) => {
    _setPage('pay');
    setActivePayment(null);
    if (tx.token === 'BTC' && tx.swapId) {
      setResumeOrder({
        depositAddress: tx.swapId,
        amountStr: tx.amount.toFixed(8),
        token: tx.token,
        destToken: tx.destToken,
      });
    }
  };

  const copyDeposit = (addr: string) => {
    navigator.clipboard.writeText(addr).then(() => {
      setCopiedDeposit(true);
      setTimeout(() => setCopiedDeposit(false), 1500);
    });
  };

  const clearInvoice = () => {
    setActiveInvoice(null);
    setInvoiceInput('');
    setInvoiceInputError('');
  };

  const applyInvoiceInput = (value = invoiceInput) => {
    const parsed = parsePaymentInvoiceFromInput(value);
    if (!parsed) {
      setInvoiceInputError('Invalid or unsupported invoice format');
      return;
    }
    setActiveInvoice(parsed);
    setInvoiceInput('');
    setInvoiceInputError('');
  };

  const dismissActiveSwap = () => {
    reset();
  };

  return (
    <div className="persona-customer persona-screen" style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden', position: 'relative' }}>
      <div className="ambient" style={{ '--g1': 'rgba(80,120,220,0.20)', '--g2': 'rgba(40,70,150,0.16)' } as React.CSSProperties} />
      <div className="gridlines" />

      <div className="page-scroll">
        {page === 'pay' && (
          <div className="page-section customer-pay-stage" style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: !isActive && !isDone && swap.step !== 'error' ? 'center' : 'flex-start', minHeight: '100%' }}>
            {!isActive && !isDone && swap.step !== 'error' && btcSwap.step !== 'error' && !resumeOrder && (
              <div style={{ width: '100%', maxWidth: 800 }}>
                <PageHeader
                  title="New payment"
                  subtitle={
                    activeInvoice
                      ? `${activeInvoice.destAmount} ${destTokenDisplay} to ${shortAddr(activeInvoice.recipient)}`
                      : 'Select source · pay · merchant receives on Initia'
                  }
                  actions={
                    activeInvoice ? (
                      <button className="btn btn-ghost" onClick={clearInvoice}>
                        Clear invoice
                      </button>
                    ) : undefined
                  }
                />
                {!activeInvoice && (
                  <div style={{ marginBottom: 14, display: 'flex', flexDirection: 'column', gap: 0 }}>
                    <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                      <div style={{ position: 'relative', flex: 1 }}>
                        <span style={{ position: 'absolute', left: 12, top: '50%', transform: 'translateY(-50%)', fontSize: 11, color: 'var(--text-3)', pointerEvents: 'none', letterSpacing: '0.06em', textTransform: 'uppercase', fontWeight: 600 }}>Invoice</span>
                        <input
                          className="input mono"
                          value={invoiceInput}
                          onChange={(event) => {
                            setInvoiceInput(event.target.value);
                            if (invoiceInputError) setInvoiceInputError('');
                          }}
                          onKeyDown={(event) => {
                            if (event.key === 'Enter') applyInvoiceInput();
                          }}
                          placeholder="unipay1... or paste payment link"
                          style={{ marginTop: 0, paddingLeft: 68, fontSize: 12, height: 38, background: 'rgba(255,255,255,0.03)', border: '1px solid var(--hairline)', borderRadius: 10 }}
                        />
                      </div>
                      <button
                        className="btn"
                        onClick={() => applyInvoiceInput()}
                        disabled={!invoiceInput.trim()}
                        style={{ height: 38, padding: '0 14px', fontSize: 12, flexShrink: 0 }}
                      >
                        Load
                      </button>
                    </div>
                    {invoiceInputError && (
                      <div style={{ color: 'var(--danger)', marginTop: 5, fontSize: 11, paddingLeft: 4 }}>
                        {invoiceInputError}
                      </div>
                    )}
                  </div>
                )}
                <div className="pay-card-stack">
                  <PayCard
                    srcChain={srcChain}
                    srcToken={srcToken}
                    destChain={selectedDestChain}
                    destTokenId={destTokenId}
                    amount={amount}
                    setSrcChain={(c) => { setSrcChain(c); }}
                    setSrcToken={setSrcToken}
                    setAmount={setAmount}
                    setShowPicker={setShowPicker}
                    usdValue={0}
                    initAmount={0}
                    feeUsd={0}
                    onPay={onPay}
                    wallet={wallet}
                    receiverAddress={receiverAddress}
                    setReceiverAddress={setReceiverAddress}
                    btcRefundAddress={btcRefundAddress}
                    setBtcRefundAddress={setBtcRefundAddress}
                    quoteMode={quoteMode}
                    requestedDestinationAmount={activeInvoice?.destAmount}
                    lockDestinationToken={!!activeInvoice}
                    lockRecipientAddress={!!activeInvoice}
                    onDestTokenChange={setDestTokenId}
                    payDisabled={chainsLoading || payDisabled}
                    payDisabledReason={payDisabledReason}
                    btcWalletConnected={isBitcoin && !!unisatAddress}
                  />

                  {showPicker === 'src' && (
                    <TokenChainPicker
                      currentChain={srcChain}
                      onClose={() => setShowPicker(null)}
                      requiredDestination={activeInvoice ? { chain: activeInvoice.destChain, tokenId: activeInvoice.destTokenId } : null}
                      onPick={(chain, token) => {
                        setSrcChain(chain);
                        setSrcToken(token);
                        setShowPicker(null);
                      }}
                    />
                  )}
                </div>
              </div>
            )}
            {/* Resumed Bitcoin order from history */}
            {resumeOrder && !isActive && (
              <div style={{ width: '100%', maxWidth: 480 }}>
                <PageHeader
                  title="Send Bitcoin"
                  subtitle={`${resumeOrder.amountStr} ${resumeOrder.token} → ${resumeOrder.destToken} on Initia`}
                />
                <BitcoinDepositCard
                  step="awaiting"
                  depositAddress={resumeOrder.depositAddress}
                  amountBtc={resumeOrder.amountStr}
                  destAmount={0}
                  destToken={resumeOrder.destToken}
                  destChainName="Initia"
                  receiverAddress={receiverAddress}
                  sourceTxHash={null}
                  destinationTxHash={null}
                  redeemTxHash={null}
                  error={null}
                  copied={copiedDeposit}
                  onCopy={copyDeposit}
                  onDone={reset}
                />
              </div>
            )}

            {/* Bitcoin QR deposit flow */}
            {isBitcoin && (isActive || isDone || btcSwap.step === 'error') && (
              <div style={{ width: '100%', maxWidth: 480 }}>
                <PageHeader
                  title={
                    btcSwap.step === 'fulfilled' || btcSwap.step === 'done'
                      ? 'Payment complete'
                      : btcSwap.step === 'refunded'
                      ? 'Payment refunded'
                      : btcSwap.step === 'error'
                      ? 'Payment failed'
                      : 'Send Bitcoin'
                  }
                  subtitle={`${bitcoinAmountDisplay || amount} BTC → ${destTokenDisplay} on Initia`}
                />
                <BitcoinDepositCard
                  step={btcSwap.step}
                  depositAddress={btcSwap.depositAddress}
                  amountBtc={bitcoinAmountDisplay || amount}
                  destAmount={progressDestAmount}
                  destToken={destTokenDisplay}
                  destChainName={(CHAINS[selectedDestChain] ?? { name: selectedDestChain }).name}
                  receiverAddress={receiverAddress}
                  sourceTxHash={btcSwap.sourceTxHash}
                  destinationTxHash={btcSwap.destinationTxHash}
                  redeemTxHash={btcSwap.redeemTxHash}
                  error={btcSwap.error}
                  copied={copiedDeposit}
                  onCopy={copyDeposit}
                  onDone={reset}
                />
              </div>
            )}
            {/* Solana swap flow */}
            {isSolana && (isActive || isDone || solSwap.step === 'error') && (
              <div style={{ width: '100%', maxWidth: 520 }}>
                <PageHeader
                  title={
                    solSwap.step === 'fulfilled'
                      ? 'Payment complete'
                      : solSwap.step === 'refunded'
                      ? 'Payment refunded'
                      : solSwap.step === 'error'
                      ? 'Payment failed'
                      : 'Paying with Solana'
                  }
                  subtitle={`${progressSourceAmount || amountNum} SOL → ${destTokenDisplay} on Initia`}
                />
                <PayProgress
                  step={
                    solSwap.step === 'connecting' || solSwap.step === 'creating'
                      ? 'connecting'
                      : solSwap.step === 'locking'
                      ? 'locking'
                      : solSwap.step === 'user_initiated'
                      ? 'user_initiated'
                      : solSwap.step === 'cobi_initiated'
                      ? 'cobi_initiated'
                      : solSwap.step === 'user_redeemed'
                      ? 'user_redeemed'
                      : solSwap.step === 'fulfilled'
                      ? 'done'
                      : solSwap.step === 'error'
                      ? 'error'
                      : 'idle'
                  }
                  stepIndex={
                    solSwap.step === 'connecting' || solSwap.step === 'creating' ? 0
                    : solSwap.step === 'locking' ? 1
                    : solSwap.step === 'user_initiated' ? 2
                    : solSwap.step === 'cobi_initiated' ? 3
                    : solSwap.step === 'user_redeemed' || solSwap.step === 'fulfilled' ? 4
                    : 0
                  }
                  error={solSwap.error}
                  sourceTxHash={solSwap.sourceTxHash}
                  destinationTxHash={null}
                  redeemTxHash={null}
                  srcChainName={'Solana Devnet'}
                  srcAmount={progressSourceAmount}
                  srcToken={'SOL'}
                  srcDecimals={9}
                  destAmount={progressDestAmount}
                  destToken={destTokenDisplay}
                  onDone={
                    solSwap.step === 'fulfilled' || solSwap.step === 'refunded' || solSwap.step === 'error'
                      ? reset
                      : dismissActiveSwap
                  }
                />
              </div>
            )}
            {/* EVM swap flow */}
            {!isBitcoin && !isSolana && (isActive || isDone || swap.step === 'error') && (
              <div style={{ width: '100%', maxWidth: 680 }}>
                <PageHeader
                  title={isDone ? 'Payment complete' : 'Processing payment'}
                  subtitle={`${progressSourceAmount || amountNum} ${srcToken} → ${destTokenDisplay} on Initia`}
                />
                <PayProgress
                  step={swap.step}
                  stepIndex={swap.stepIndex}
                  srcAmount={progressSourceAmount}
                  destAmount={progressDestAmount}
                  srcToken={srcToken}
                  destToken={destTokenDisplay}
                  srcChainName={(CHAINS[srcChain] ?? { name: srcChain }).name}
                  destChainName={(CHAINS[selectedDestChain] ?? { name: selectedDestChain }).name}
                  receiverAddress={receiverAddress}
                  srcDecimals={srcDecimals}
                  destDecimals={destDecimals}
                  sourceTxHash={swap.sourceTxHash}
                  destinationTxHash={swap.destinationTxHash}
                  redeemTxHash={swap.redeemTxHash}
                  error={swap.error}
                  onDone={
                    swap.step === 'fulfilled' || swap.step === 'done' || swap.step === 'refunded' || swap.step === 'error'
                      ? reset
                      : dismissActiveSwap
                  }
                />
              </div>
            )}
          </div>
        )}

        {page === 'history' && !selectedTxId && (
          <div className="page-section" style={{ maxWidth: 1280, margin: '0 auto' }}>
            <PageHeader
              title="History"
              subtitle={
                ordersLoading
                  ? 'Loading…'
                  : ordersRefreshing
                    ? `${orders.length} transactions · syncing`
                    : `${orders.length} transactions`
              }
              actions={
                <button
                  className="btn"
                  onClick={refetchOrders}
                  disabled={ordersLoading || ordersRefreshing}
                  style={{ padding: '8px 10px', borderRadius: 10, minWidth: 0 }}
                  title="Refresh transactions"
                >
                  <Icons.refresh />
                </button>
              }
            />
            <HistoryPage
              history={orders}
              loading={ordersLoading}
              refreshing={ordersRefreshing}
              onResume={onResume}
              onSelect={setSelectedTxId}
            />
          </div>
        )}

        {page === 'history' && selectedTxId && (() => {
          const tx = orders.find((t) => t.id === selectedTxId);
          if (!tx) { setSelectedTxId(null); return null; }
          return (
            <div className="page-section" style={{ maxWidth: 1100, margin: '0 auto' }}>
              <TxDetail tx={tx} onBack={() => setSelectedTxId(null)} />
            </div>
          );
        })()}
      </div>
    </div>
  );
}


type BitcoinStep = import('../../hooks/useBitcoinSwap').BitcoinSwapStep;

function BitcoinDepositCard({
  step,
  depositAddress,
  amountBtc,
  destAmount,
  destToken,
  destChainName,
  receiverAddress,
  sourceTxHash,
  destinationTxHash,
  redeemTxHash,
  error,
  copied,
  onCopy,
  onDone,
}: {
  step: BitcoinStep;
  depositAddress: string | null;
  amountBtc: string;
  destAmount: number;
  destToken: string;
  destChainName?: string;
  receiverAddress?: string;
  sourceTxHash: string | null;
  destinationTxHash: string | null;
  redeemTxHash: string | null;
  error: string | null;
  copied: boolean;
  onCopy: (addr: string) => void;
  onDone: () => void;
}) {
  // Map the lifecycle into a 5-step index that mirrors the EVM/Solana PayProgress.
  // 0: initiating order  1: locking BTC (QR shown)  2: BTC locked  3: dest locked  4: delivered
  const stepIndex =
    step === 'idle' || step === 'loading' || step === 'creating' ? 0
    : step === 'sending' || step === 'awaiting' ? 1
    : step === 'user_initiated' ? 2
    : step === 'cobi_initiated' ? 3
    : step === 'user_redeemed' ? 4
    : step === 'fulfilled' || step === 'done' ? 4
    : 0;

  const isDone = step === 'fulfilled' || step === 'done';
  const isError = step === 'error';
  const isRefunded = step === 'refunded';
  const showQr = !!depositAddress && (step === 'awaiting' || step === 'sending' || step === 'loading' || step === 'creating');

  const destPair = destChainName ? `${destToken} on ${destChainName}` : destToken;
  const shortReceiver = receiverAddress ? shortAddr(receiverAddress) : '';
  const deliveredTo = shortReceiver ? `to ${shortReceiver}` : 'to merchant';

  const steps = [
    {
      label: 'Initiating swap',
      desc: 'Generating Bitcoin deposit address',
    },
    {
      label: 'Locking BTC on Bitcoin Testnet',
      desc: step === 'sending'
        ? 'Confirm the transaction in your UniSat wallet…'
        : `Send ${amountBtc} BTC to the deposit address`,
    },
    {
      label: 'BTC on Bitcoin Testnet locked',
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
          ? `${destAmount.toFixed(6)} ${destToken} arrived at ${shortReceiver || 'merchant'}`
          : `${destToken} arrived at ${shortReceiver || 'merchant'}`,
    },
  ];

  return (
    <div
      className="pay-progress-shell"
      style={{ width: '100%', maxWidth: 620, margin: '0 auto', position: 'relative' }}
    >
      <div className="glass" style={{ padding: 28, position: 'relative', overflow: 'hidden' }}>
        <div
          style={{
            position: 'absolute',
            top: -80,
            left: '50%',
            transform: 'translateX(-50%)',
            width: 400,
            height: 200,
            background: isDone
              ? 'color-mix(in oklch, var(--success) 35%, transparent)'
              : isError
              ? 'rgba(239,68,68,0.15)'
              : 'var(--accent-glow)',
            filter: 'blur(80px)',
            pointerEvents: 'none',
            transition: 'background 400ms',
          }}
        />

        <div className="pay-progress-header" style={{ position: 'relative' }}>
          <div>
            <div className="eyebrow" style={{ marginBottom: 6 }}>
              Atomic swap · in-flight
            </div>
            <div style={{ fontSize: 22, fontWeight: 500, letterSpacing: '-0.01em' }}>
              {isDone ? 'Swap complete' : isError ? 'Swap failed' : isRefunded ? 'Swap refunded' : 'Processing swap'}
            </div>
            <div className="mono tnum" style={{ fontSize: 13, color: 'var(--text-2)', marginTop: 4 }}>
              {amountBtc} BTC <span style={{ color: 'var(--text-3)' }}>→</span>{' '}
              {destAmount > 0 ? `${destAmount.toFixed(6)} ` : ''}{destToken}
            </div>
          </div>
        </div>

        {isError && error && (
          <div
            style={{
              marginTop: 16,
              marginBottom: 4,
              padding: '10px 14px',
              borderRadius: 10,
              background: 'rgba(239,68,68,0.1)',
              border: '1px solid rgba(239,68,68,0.3)',
              fontSize: 13,
              color: '#f87171',
            }}
          >
            {error}
          </div>
        )}

        {isRefunded && (
          <div
            style={{
              marginTop: 16,
              marginBottom: 4,
              padding: '10px 14px',
              borderRadius: 10,
              background: 'rgba(251,146,60,0.1)',
              border: '1px solid rgba(251,146,60,0.3)',
              fontSize: 13,
              color: '#fb923c',
            }}
          >
            ↩ Swap refunded — BTC returned to your refund address
          </div>
        )}

        {/* Compact QR + deposit address — only while awaiting deposit */}
        {showQr && depositAddress && (
          <div
            style={{
              marginTop: 18,
              padding: 14,
              borderRadius: 12,
              background: 'rgba(255,255,255,0.015)',
              border: '1px solid var(--hairline)',
              display: 'flex',
              gap: 14,
              alignItems: 'center',
            }}
          >
            <div style={{ background: '#fff', padding: 6, borderRadius: 8, flexShrink: 0 }}>
              <QRCodeSVG
                value={`bitcoin:${depositAddress}?amount=${amountBtc}`}
                size={96}
                level="M"
                includeMargin={false}
              />
            </div>
            <div style={{ flex: 1, minWidth: 0 }}>
              <div className="eyebrow" style={{ marginBottom: 4 }}>Deposit address</div>
              <div style={{ display: 'flex', gap: 6, alignItems: 'center' }}>
                <span
                  className="mono"
                  style={{ flex: 1, fontSize: 12, color: 'var(--text-1)', wordBreak: 'break-all', lineHeight: 1.4 }}
                >
                  {depositAddress}
                </span>
                <button
                  className="iconbtn"
                  onClick={() => onCopy(depositAddress)}
                  title="Copy address"
                  style={{ flexShrink: 0, color: copied ? '#4ade80' : undefined }}
                >
                  {copied ? '✓' : <Icons.copy />}
                </button>
              </div>
              <div className="mono" style={{ fontSize: 11, color: 'var(--text-3)', marginTop: 6 }}>
                Send exactly <span style={{ color: 'var(--text-1)' }}>{amountBtc} BTC</span> · Bitcoin Testnet 4
              </div>
            </div>
          </div>
        )}

        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            gap: 0,
            position: 'relative',
            margin: '18px 0 4px',
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
                  ? 'done'
                  : i === stepIndex && !isDone && !isError && !isRefunded
                  ? 'active'
                  : isDone && i === steps.length - 1
                  ? 'done'
                  : 'pending'
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
              background: 'rgba(255,255,255,0.015)',
              border: '1px solid var(--hairline)',
            }}
          >
            <div className="eyebrow" style={{ marginBottom: 10 }}>Transaction links</div>
            <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
              {sourceTxHash && <HashLink hash={sourceTxHash} label="Source HTLC lock ·" />}
              {destinationTxHash && <HashLink hash={destinationTxHash} label="Executor lock ·" />}
              {redeemTxHash && <HashLink hash={redeemTxHash} label={`${destToken} delivered ·`} />}
            </div>
          </div>
        )}

        <div className="progress-actions" style={{ display: 'flex', gap: 10, marginTop: 20 }}>
          {isDone ? (
            <button className="btn btn-primary btn-lg" style={{ flex: 1 }} onClick={onDone}>
              Make another swap
            </button>
          ) : isError || isRefunded ? (
            <button className="btn btn-primary btn-lg" style={{ flex: 1 }} onClick={onDone}>
              {isRefunded ? 'Close' : 'Try again'}
            </button>
          ) : (
            <button className="btn btn-lg" style={{ flex: 1 }} onClick={onDone}>
              Cancel
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: React.ReactNode }) {
  return (
    <div className="page-header page-header-compact">
      <div className="page-header-copy">
        <h1 style={{ margin: 0, fontSize: 21, fontWeight: 500, letterSpacing: '-0.01em' }}>{title}</h1>
        {subtitle && <div style={{ color: 'var(--text-2)', fontSize: 13, marginTop: 3 }}>{subtitle}</div>}
      </div>
      {actions && <div className="page-header-actions">{actions}</div>}
    </div>
  );
}

function historyStatusMeta(status: HistoryTx['status']): { cls: string; label: string } {
  if (status === 'Settled') return { cls: 'pill-success', label: 'Completed' };
  if (status === 'Pending') return { cls: 'pill-warn', label: 'Pending' };
  return { cls: 'pill-danger', label: 'Refunded' };
}

function historyAmountDecimals(token: string): number {
  if (token === 'BTC') return 6;
  if (token === 'ETH' || token === 'SOL') return 4;
  return 2;
}

function formatHistoryAmount(amount: number, token: string): string {
  return amount.toLocaleString('en-US', {
    minimumFractionDigits: 0,
    maximumFractionDigits: historyAmountDecimals(token),
  });
}

function formatHistoryValue(tx: HistoryTx): string {
  const notional = tx.srcPrice ?? tx.dstPrice;
  if (typeof notional !== 'number' || !Number.isFinite(notional)) return '—';
  return fmtUSD(notional, notional >= 1 ? 2 : 4);
}

function formatHistoryRate(tx: HistoryTx): string {
  const dst = tx.destToken || '';
  if (!tx.amount || !tx.initAmount) return `${dst} payout`;
  const impliedRate = tx.initAmount / tx.amount;
  const digits = impliedRate >= 1 ? 4 : 6;
  return `1 ${tx.token} → ${impliedRate.toLocaleString('en-US', {
    minimumFractionDigits: 0,
    maximumFractionDigits: digits,
  })} ${dst}`;
}

function historyAddress(value?: string | null): string {
  return value ? shortAddr(value) : '—';
}

function HistoryFilter({
  prefix,
  value,
  options,
  onChange,
}: {
  prefix: string;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
}) {
  return (
    <label className="history-filter-pill">
      {prefix && <span className="history-filter-prefix">{prefix}</span>}
      <select className="history-filter-select" value={value} onChange={(e) => onChange(e.target.value)}>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      <Icons.chevron className="history-filter-chevron" />
    </label>
  );
}

function HistoryPage({
  history,
  loading,
  refreshing,
  onResume,
  onSelect,
}: {
  history: HistoryTx[];
  loading?: boolean;
  refreshing?: boolean;
  onResume: (tx: HistoryTx) => void;
  onSelect: (id: string) => void;
}) {
  const [srcFilter, setSrcFilter] = useState('all');
  const [destFilter, setDestFilter] = useState('all');
  const [statusFilter, setStatusFilter] = useState('all');

  const sourceChains = Array.from(new Set(history.map((tx) => tx.chain).filter(Boolean))).sort((a, b) => {
    const aLabel = CHAINS[a]?.name ?? a;
    const bLabel = CHAINS[b]?.name ?? b;
    return aLabel.localeCompare(bLabel);
  });
  const destinationChains = Array.from(new Set(history.map((tx) => tx.destChain).filter(Boolean) as string[])).sort((a, b) => {
    const aLabel = CHAINS[a]?.name ?? a;
    const bLabel = CHAINS[b]?.name ?? b;
    return aLabel.localeCompare(bLabel);
  });

  const safeSrcFilter = srcFilter === 'all' || sourceChains.includes(srcFilter) ? srcFilter : 'all';
  const safeDestFilter = destFilter === 'all' || destinationChains.includes(destFilter) ? destFilter : 'all';

  const filteredHistory = history.filter((tx) => {
    return (safeSrcFilter === 'all' || tx.chain === safeSrcFilter)
      && (safeDestFilter === 'all' || tx.destChain === safeDestFilter)
      && (statusFilter === 'all' || tx.status === statusFilter);
  });

  const sourceOptions = [
    { value: 'all', label: 'All chains' },
    ...sourceChains.map((chainId) => ({ value: chainId, label: CHAINS[chainId]?.name ?? chainId })),
  ];
  const destinationOptions = [
    { value: 'all', label: 'All chains' },
    ...destinationChains.map((chainId) => ({ value: chainId, label: CHAINS[chainId]?.name ?? chainId })),
  ];
  const statusOptions = [
    { value: 'all', label: 'All statuses' },
    { value: 'Pending', label: 'Pending' },
    { value: 'Settled', label: 'Completed' },
    { value: 'Refunded', label: 'Refunded' },
  ];
  const resultsLabel = filteredHistory.length === history.length
    ? `${history.length} total`
    : `${filteredHistory.length} of ${history.length} shown`;

  return (
    <div className="glass history-table-shell">
      <div className="history-table-toolbar">
        <div>
          <div className="history-table-title">Transactions</div>
          <div className="history-table-subtitle">
            {loading ? 'Loading orders…' : refreshing ? `${resultsLabel} · syncing` : resultsLabel}
          </div>
        </div>
        <div className="history-table-controls">
          <HistoryFilter prefix="From" value={safeSrcFilter} options={sourceOptions} onChange={setSrcFilter} />
          <div className="history-filter-swap" aria-hidden="true">
            <Icons.arrowRight />
          </div>
          <HistoryFilter prefix="To" value={safeDestFilter} options={destinationOptions} onChange={setDestFilter} />
          <HistoryFilter prefix="" value={statusFilter} options={statusOptions} onChange={setStatusFilter} />
        </div>
      </div>

      <div className="history-table-scroll">
        <div className="history-table-grid history-table-head">
          <span>Created</span>
          <span>Address</span>
          <span>Asset</span>
          <span>Value</span>
          <span>Source</span>
          <span>Status</span>
        </div>

        {loading && (
          <div className="history-table-empty">
            Loading orders…
          </div>
        )}

        {!loading && history.length === 0 && (
          <div className="history-table-empty">
            No transactions yet
          </div>
        )}

        {!loading && history.length > 0 && filteredHistory.length === 0 && (
          <div className="history-table-empty">
            No transactions match these filters
          </div>
        )}

        {!loading && filteredHistory.map((tx) => (
          <HistoryRow
            key={tx.id}
            tx={tx}
            onSelect={() => onSelect(tx.id)}
            onResume={onResume}
          />
        ))}
      </div>
    </div>
  );
}

function HistoryRow({ tx, onSelect, onResume }: { tx: HistoryTx; onSelect: () => void; onResume: (tx: HistoryTx) => void }) {
  const sourceChain = CHAINS[tx.chain] ?? { name: tx.chain, short: tx.chain };
  const destChainId = tx.destChain ?? '';
  const destinationChain = CHAINS[destChainId] ?? { name: destChainId, short: destChainId };
  const destLabel = tx.destToken || '';
  const status = historyStatusMeta(tx.status);
  const sourceAddress = tx.srcInitiator ?? tx.refundAddr ?? tx.swapId;
  const destinationAddress = tx.destinationAddr ?? tx.dstRedeemer;

  return (
    <div
      className="history-table-grid history-table-row"
      onClick={onSelect}
    >
      <div className="history-table-cell">
        <div className="history-table-primary">{relTime(tx.ts)}</div>
        <div className="history-table-secondary">
          {new Date(tx.ts).toLocaleDateString('en-US', { month: 'short', day: 'numeric' })}
        </div>
      </div>

      <div className="history-table-cell">
        <div className="history-address-pair">
          <span className="history-address-text mono">{historyAddress(sourceAddress)}</span>
          <span className="history-address-arrow">
            <Icons.arrowRight />
          </span>
          <span className="history-address-text mono">{historyAddress(destinationAddress)}</span>
        </div>
        <div className="history-table-secondary">
          {sourceChain.short} to {destinationChain.short}
        </div>
      </div>

      <div className="history-table-cell">
        <div className="history-asset-pair">
          <span className="history-asset-item">
            <TokenLogo sym={tx.token} size="sm" chain={tx.chain} />
            <span className="history-asset-amount tnum">{formatHistoryAmount(tx.amount, tx.token)} {tx.token}</span>
          </span>
          <span className="history-asset-arrow">
            <Icons.arrowRight />
          </span>
          <span className="history-asset-item">
            <TokenLogo sym={destLabel} size="sm" chain={destChainId} />
            <span className="history-asset-amount tnum">{formatHistoryAmount(tx.initAmount, destLabel)} {destLabel}</span>
          </span>
        </div>
        <div className="history-table-secondary">{formatHistoryRate(tx)}</div>
      </div>

      <div className="history-table-cell">
        <div className="history-table-primary tnum">{formatHistoryValue(tx)}</div>
      </div>

      <div className="history-table-cell">
        <div className="history-source-line">
          <ChainLogo id={tx.chain} size="sm" />
          <span className="history-table-primary">{sourceChain.name}</span>
        </div>
        <div className="history-table-secondary">To {destinationChain.name}</div>
      </div>

      <div className="history-table-cell history-table-status">
        <span className={`pill ${status.cls}`}><span className="dot" />{status.label}</span>
        {tx.status === 'Pending' && (
          <button
            className="history-row-action"
            onClick={(e) => { e.stopPropagation(); onResume(tx); }}
          >
            Resume
          </button>
        )}
      </div>
    </div>
  );
}
