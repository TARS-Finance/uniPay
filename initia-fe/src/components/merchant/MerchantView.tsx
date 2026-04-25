import { useEffect, useMemo, useRef, useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { QRCodeSVG } from 'qrcode.react';
import { CHAINS, fmtUSD, relTime, shortAddr } from '../../data';
import { useWallet } from '../../lib/wallet-context';
import type { PaymentInvoice, Pool, Settlement } from '../../types';
import { Icons } from '../shared/Icons';
import { TokenLogo, ChainLogo } from '../shared/TokenLogo';
import { useInterwovenKit } from '@initia/interwovenkit-react';
import { EarnPanel } from './EarnPanel';
import { useStrategies } from '../../hooks/useStrategies';
import {
  buildPaymentInvoiceCode,
  buildPaymentInvoiceUrl,
  generateInvoiceId,
} from '../../lib/invoice';
import {
  useMerchantOverview,
  useMerchantPools,
  useMerchantActivity,
  type MerchantPool,
  type MerchantActivityItem,
} from '../../lib/stacker';
import { L1_REST_URL, L1_TX_EXPLORER_URL, STACKER_API } from '../../lib/config';

// stacker amounts come back as raw denom units (uusdc has 6 decimals)
const USDC_DECIMALS = 6;
function denomToNumber(raw: string | null | undefined, decimals = USDC_DECIMALS): number {
  if (!raw) return 0;
  try {
    const n = Number(raw);
    if (!Number.isFinite(n)) return 0;
    return n / 10 ** decimals;
  } catch {
    return 0;
  }
}

function normalizeTokenSymbol(raw: string): string {
  if (!raw) return raw;
  const stripped = raw.replace(/^u(?=[a-z])/, '');
  return stripped.toUpperCase();
}

function normalizeChainId(raw: string): string {
  if (!raw) return raw;
  if (raw === 'initiation-2') return 'initia';
  return raw;
}

function poolFromMerchantPool(p: MerchantPool): Pool {
  const [a, b] = p.tokens;
  return {
    id: p.id,
    name: p.name,
    tokens: [normalizeTokenSymbol(a), normalizeTokenSymbol(b)],
    staked: denomToNumber(p.staked),
    apy: p.apy_bps / 100,
    earned: denomToNumber(p.earned),
    chain: normalizeChainId('initiation-2'),
    tvl: '—',
  };
}

async function fetchClaimedFromL1Tx(txHash: string): Promise<bigint | null> {
  const base = L1_REST_URL.replace(/\/$/, '');
  const url = `${base}/cosmos/tx/v1beta1/txs/${txHash}`;
  for (let attempt = 0; attempt < 8; attempt++) {
    try {
      const res = await fetch(url);
      if (res.ok) {
        const body = (await res.json()) as {
          tx_response?: {
            events?: Array<{ type: string; attributes: Array<{ key: string; value: string }> }>;
            logs?: Array<{
              events?: Array<{ type: string; attributes: Array<{ key: string; value: string }> }>;
            }>;
          };
        };
        const events: Array<{ type: string; attributes: Array<{ key: string; value: string }> }> = [];
        const tx = body.tx_response;
        if (tx?.events) events.push(...tx.events);
        if (tx?.logs) for (const log of tx.logs) if (log.events) events.push(...log.events);
        let total = 0n;
        for (const ev of events) {
          if (ev.type !== 'withdraw_rewards') continue;
          const amount = ev.attributes.find((a) => a.key === 'amount')?.value;
          if (!amount) continue;
          // amount is a comma-separated coin list, e.g. "12345uinit,67890uusdc"
          for (const part of amount.split(',')) {
            const match = /^(\d+)uinit$/.exec(part.trim());
            if (match) total += BigInt(match[1]);
          }
        }
        return total;
      }
    } catch {
      /* retry */
    }
    await new Promise((r) => setTimeout(r, 1500));
  }
  return null;
}

function settlementFromActivity(a: MerchantActivityItem): Settlement {
  return {
    id: a.id,
    amount: denomToNumber(a.amount),
    token: normalizeTokenSymbol(a.inputDenom),
    srcChain: 'initia',
    ts: new Date(a.startedAt).getTime(),
    staked: a.staked,
  };
}

interface Props {
  page: 'overview' | 'pools' | 'activity' | 'earn';
  setPage: (p: 'overview' | 'pools' | 'activity' | 'earn') => void;
}

export function MerchantView({ page, setPage }: Props) {
  const { openBridge } = useWallet();
  const { username, initiaAddress, requestTxBlock } = useInterwovenKit();
  const [passive, setPassive] = useState(true);
  const [claiming, setClaiming] = useState<string | null>(null);
  const [claimError, setClaimError] = useState<string | null>(null);
  const [claimSuccess, setClaimSuccess] = useState<{ txHash: string; claimedUinit: bigint | null } | null>(null);
  const [realizing, setRealizing] = useState<string | null>(null);
  const [realizeError, setRealizeError] = useState<string | null>(null);
  const [realizeSuccess, setRealizeSuccess] = useState<{ txHash: string; releaseAt: string; usdcAmount: number } | null>(null);
  const queryClient = useQueryClient();
  const [invoiceAmount, setInvoiceAmount] = useState('25');
  const [invoiceRecipient, setInvoiceRecipient] = useState('');
  const [invoiceTokenId, setInvoiceTokenId] = useState('');
  const [copiedInvoice, setCopiedInvoice] = useState(false);
  const invoiceStudioRef = useRef<HTMLDivElement | null>(null);
  const [invoiceId, setInvoiceId] = useState('');
  const invoiceSignatureRef = useRef('');
  const { strategies, loading: strategiesLoading, error: strategiesError } = useStrategies();

  const merchantId = initiaAddress ?? undefined;
  const { data: overview, isLoading: overviewLoading, error: overviewError } = useMerchantOverview(merchantId);
  const { data: livePools = [], isLoading: poolsLoading } = useMerchantPools(merchantId);
  const { data: liveActivity = [], isLoading: activityLoading } = useMerchantActivity(merchantId);

  const balance = useMemo(() => ({
    avail: denomToNumber(overview?.principal_available),
    staked: denomToNumber(overview?.principal_staked),
    yield: denomToNumber(overview?.yield_earned),
  }), [overview]);
  const apyPct = (overview?.apy_bps ?? 0) / 100;

  const pools: Pool[] = useMemo(() => livePools.map(poolFromMerchantPool), [livePools]);
  const settlements: Settlement[] = useMemo(() => liveActivity.map(settlementFromActivity), [liveActivity]);
  const destinationOptions = useMemo(() => {
    const unique = new Map<string, { tokenId: string; displayToken: string; asset: string; destChain: string }>();
    for (const strategy of strategies) {
      if (!unique.has(strategy.destTokenId)) {
        unique.set(strategy.destTokenId, {
          tokenId: strategy.destTokenId,
          displayToken: strategy.destDisplaySymbol ?? strategy.destTokenId.toUpperCase(),
          asset: strategy.destAsset,
          destChain: strategy.destChain,
        });
      }
    }

    return Array.from(unique.values()).sort((left, right) =>
      left.displayToken.localeCompare(right.displayToken),
    );
  }, [strategies]);
  const selectedInvoiceToken = destinationOptions.find((option) => option.tokenId === invoiceTokenId) ?? null;
  const invoiceAmountValid = Number(invoiceAmount) > 0;
  const invoiceRecipientValid = invoiceRecipient.trim().length > 10;
  const invoiceSignature = selectedInvoiceToken && invoiceRecipientValid && invoiceAmountValid
    ? `${invoiceRecipient.trim().toLowerCase()}|${selectedInvoiceToken.tokenId}|${invoiceAmount.trim()}`
    : '';
  const invoicePayload: PaymentInvoice | null = selectedInvoiceToken && invoiceAmountValid && invoiceRecipientValid && invoiceId
    ? {
      version: '1',
      recipient: invoiceRecipient.trim(),
      destChain: selectedInvoiceToken.destChain,
      destTokenId: selectedInvoiceToken.tokenId,
      destAmount: invoiceAmount.trim(),
      invoiceId,
    }
    : null;
  const invoiceCode = invoicePayload ? buildPaymentInvoiceCode(invoicePayload) : '';
  const invoiceUrl = invoicePayload ? buildPaymentInvoiceUrl(invoicePayload) : '';

  useEffect(() => {
    if (!invoiceSignature) {
      setInvoiceId('');
      invoiceSignatureRef.current = '';
      return;
    }
    if (invoiceSignatureRef.current === invoiceSignature) return;
    invoiceSignatureRef.current = invoiceSignature;
    setInvoiceId(generateInvoiceId());
  }, [invoiceSignature]);

  const onClaim = async (id: string) => {
    if (!initiaAddress) {
      setClaimError('Connect a wallet first');
      return;
    }
    const merchantPool = livePools.find((p) => p.id === id);
    if (!merchantPool) {
      setClaimError('Pool not found');
      return;
    }
    const stakedRaw = merchantPool.staked;
    if (!stakedRaw || BigInt(stakedRaw) <= 0n) {
      setClaimError('Nothing staked to claim');
      return;
    }

    setClaiming(id);
    setClaimError(null);
    setClaimSuccess(null);
    try {
      const createRes = await fetch(`${STACKER_API}/merchants/${initiaAddress}/withdrawals`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ strategyId: id, inputAmount: stakedRaw }),
      });
      if (!createRes.ok) {
        const text = await createRes.text();
        throw new Error(`stacker ${createRes.status}: ${text.slice(0, 160)}`);
      }
      const created = (await createRes.json()) as {
        withdrawalId: string;
        messages: Array<{ typeUrl: string; value: Record<string, unknown> }>;
        chainId: string;
      };

      const decodedMessages = created.messages.map((msg) => {
        if (msg.typeUrl === '/initia.move.v1.MsgExecute' && Array.isArray((msg.value as { args?: unknown[] }).args)) {
          const value = msg.value as { args: unknown[] };
          const args = value.args.map((a) => {
            if (typeof a === 'string') {
              const binary = atob(a);
              const bytes = new Uint8Array(binary.length);
              for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
              return bytes;
            }
            return a;
          });
          return { ...msg, value: { ...value, args } };
        }
        return msg;
      });

      const { transactionHash } = await requestTxBlock({
        messages: decodedMessages,
        chainId: created.chainId,
      });
      if (!transactionHash) throw new Error('Wallet did not return a tx hash');

      const confirmRes = await fetch(
        `${STACKER_API}/merchants/${initiaAddress}/withdrawals/${created.withdrawalId}`,
        {
          method: 'PATCH',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ txHash: transactionHash }),
        },
      );
      if (!confirmRes.ok) {
        const text = await confirmRes.text();
        throw new Error(`confirm failed ${confirmRes.status}: ${text.slice(0, 160)}`);
      }
      setClaimSuccess({ txHash: transactionHash, claimedUinit: null });
      void queryClient.invalidateQueries({ queryKey: ['stacker'] });
      void fetchClaimedFromL1Tx(transactionHash).then((claimedUinit) => {
        setClaimSuccess((prev) => (prev && prev.txHash === transactionHash ? { ...prev, claimedUinit } : prev));
      });
    } catch (err) {
      setClaimError(err instanceof Error ? err.message : 'Claim failed');
    } finally {
      setClaiming(null);
    }
  };

  const onRealize = async (id: string) => {
    if (!initiaAddress) {
      setRealizeError('Connect a wallet first');
      return;
    }
    const merchantPool = livePools.find((p) => p.id === id);
    if (!merchantPool) {
      setRealizeError('Pool not found');
      return;
    }
    const stakedRaw = merchantPool.staked;
    if (!stakedRaw || BigInt(stakedRaw) <= 0n) {
      setRealizeError('No principal to unstake');
      return;
    }

    setRealizing(id);
    setRealizeError(null);
    setRealizeSuccess(null);
    try {
      const createRes = await fetch(`${STACKER_API}/merchants/${initiaAddress}/unbonds`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ strategyId: id, inputAmount: stakedRaw }),
      });
      if (!createRes.ok) {
        const text = await createRes.text();
        throw new Error(`stacker ${createRes.status}: ${text.slice(0, 160)}`);
      }
      const created = (await createRes.json()) as {
        messages: Array<{ typeUrl: string; value: Record<string, unknown> }>;
        chainId: string;
        releaseAt: string;
        unbondingMs: number;
      };

      const decodedMessages = created.messages.map((msg) => {
        if (msg.typeUrl === '/initia.move.v1.MsgExecute' && Array.isArray((msg.value as { args?: unknown[] }).args)) {
          const value = msg.value as { args: unknown[] };
          const args = value.args.map((a) => {
            if (typeof a === 'string') {
              const binary = atob(a);
              const bytes = new Uint8Array(binary.length);
              for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
              return bytes;
            }
            return a;
          });
          return { ...msg, value: { ...value, args } };
        }
        return msg;
      });

      const { transactionHash } = await requestTxBlock({
        messages: decodedMessages,
        chainId: created.chainId,
      });
      if (!transactionHash) throw new Error('Wallet did not return a tx hash');

      const usdcAmount = denomToNumber(stakedRaw) + denomToNumber(merchantPool.earned);
      setRealizeSuccess({ txHash: transactionHash, releaseAt: created.releaseAt, usdcAmount });
      void queryClient.invalidateQueries({ queryKey: ['stacker'] });
    } catch (err) {
      setRealizeError(err instanceof Error ? err.message : 'Realize failed');
    } finally {
      setRealizing(null);
    }
  };

  useEffect(() => {
    if (!invoiceRecipient && initiaAddress) {
      setInvoiceRecipient(initiaAddress);
    }
  }, [initiaAddress, invoiceRecipient]);

  useEffect(() => {
    if (destinationOptions.length === 0) return;
    if (destinationOptions.some((option) => option.tokenId === invoiceTokenId)) return;
    setInvoiceTokenId(destinationOptions[0].tokenId);
  }, [destinationOptions, invoiceTokenId]);

  const focusInvoiceStudio = () => {
    invoiceStudioRef.current?.scrollIntoView({ behavior: 'smooth', block: 'start' });
  };

  const copyInvoiceCode = () => {
    if (!invoiceCode) return;
    navigator.clipboard.writeText(invoiceCode).then(() => {
      setCopiedInvoice(true);
      setTimeout(() => setCopiedInvoice(false), 1500);
    });
  };

  const copyInvoiceLink = () => {
    if (!invoiceUrl) return;
    navigator.clipboard.writeText(invoiceUrl).then(() => {
      setCopiedInvoice(true);
      setTimeout(() => setCopiedInvoice(false), 1500);
    });
  };

  return (
    <div className="persona-merchant persona-screen" style={{ flex: 1, display: 'flex', flexDirection: 'column', overflow: 'hidden', position: 'relative' }}>
      <div className="ambient" style={{ '--g1': 'rgba(140,170,110,0.16)', '--g2': 'rgba(180,150,90,0.14)' } as React.CSSProperties} />
      <div className="gridlines" />

      <div className="page-scroll">
        <div className="page-section" style={{ maxWidth: 1080, margin: '0 auto' }}>

          {page === 'overview' && (
            <>
              <PageHeader
                title="Overview"
                subtitle={username ?? (initiaAddress ? shortAddr(initiaAddress) : 'merchant.init')}
                actions={<button className="btn" onClick={focusInvoiceStudio}><Icons.qr /> Invoice QR</button>}
              />

              <div ref={invoiceStudioRef} className="merchant-invoice-grid">
                <div className="glass merchant-invoice-card">
                  <div className="merchant-invoice-eyebrow">Merchant invoice</div>
                  <div className="merchant-invoice-title">Generate a payer QR</div>
                  <div className="merchant-invoice-subtitle">
                    Lock the payout asset, amount, and recipient. The payer only chooses the source chain and token.
                  </div>

                  <div className="merchant-invoice-field">
                    <div className="field-label">Payout asset</div>
                    <div className="merchant-token-grid">
                      {destinationOptions.map((option) => {
                        const active = option.tokenId === invoiceTokenId;
                        return (
                          <button
                            key={option.tokenId}
                            type="button"
                            className={`merchant-token-pill ${active ? 'is-active' : ''}`}
                            onClick={() => setInvoiceTokenId(option.tokenId)}
                          >
                            <TokenLogo sym={option.displayToken} size="sm" />
                            <span>{option.displayToken}</span>
                          </button>
                        );
                      })}
                    </div>
                    {strategiesLoading && <div className="merchant-inline-note">Loading live payout assets…</div>}
                    {strategiesError && <div className="merchant-inline-note merchant-inline-note-error">Could not load payout assets: {strategiesError}</div>}
                  </div>

                  <div className="merchant-invoice-row">
                    <div className="merchant-invoice-field">
                      <div className="field-label">Destination amount</div>
                      <input
                        className="input mono"
                        value={invoiceAmount}
                        onChange={(event) => setInvoiceAmount(event.target.value.replace(/[^\d.]/g, ''))}
                        inputMode="decimal"
                        placeholder="0"
                        style={{ marginTop: 6, padding: '12px 14px', fontSize: 13.5 }}
                      />
                    </div>
                    <div className="merchant-invoice-field">
                      <div className="field-label">Destination chain</div>
                      <div className="merchant-chain-pill">
                        {selectedInvoiceToken ? (
                          <>
                            <ChainLogo id={selectedInvoiceToken.destChain} size="sm" />
                            <span>{CHAINS[selectedInvoiceToken.destChain]?.name ?? selectedInvoiceToken.destChain}</span>
                          </>
                        ) : (
                          <span>Select payout asset</span>
                        )}
                      </div>
                    </div>
                  </div>

                  <div className="merchant-invoice-field">
                    <div className="field-label">Recipient address</div>
                    <input
                      className="input mono"
                      value={invoiceRecipient}
                      onChange={(event) => setInvoiceRecipient(event.target.value)}
                      placeholder="init1… or 0x…"
                      style={{ marginTop: 6, padding: '12px 14px', fontSize: 13.5 }}
                    />
                  </div>

                  <div className="merchant-invoice-summary">
                    <SummaryItem label="Pays out" value={selectedInvoiceToken ? `${invoiceAmount || '0'} ${selectedInvoiceToken.displayToken}` : 'Select asset'} />
                    <SummaryItem label="Recipient" value={invoiceRecipientValid ? shortAddr(invoiceRecipient.trim()) : 'Set recipient'} />
                    <SummaryItem label="Mode" value="Exact destination payout" />
                  </div>
                </div>

                <div className="glass merchant-qr-card">
                  <div className="merchant-invoice-eyebrow">Scan to pay</div>
                  <div className="merchant-qr-frame">
                    {invoiceUrl || invoiceCode ? (
                      <QRCodeSVG
                        value={invoiceUrl || invoiceCode}
                        size={220}
                        level="M"
                        includeMargin={false}
                      />
                    ) : (
                      <div className="merchant-qr-placeholder">
                        Fill the invoice fields to generate a QR.
                      </div>
                    )}
                  </div>

                  <div className="merchant-qr-title">
                    {selectedInvoiceToken ? `${invoiceAmount || '0'} ${selectedInvoiceToken.displayToken}` : 'Invoice preview'}
                  </div>
                  <div className="merchant-qr-subtitle">
                    {invoiceRecipientValid ? shortAddr(invoiceRecipient.trim()) : 'Recipient address required'}
                  </div>

                  <button className="btn btn-primary" onClick={invoiceCode ? copyInvoiceCode : copyInvoiceLink} disabled={!invoiceUrl && !invoiceCode}>
                    {copiedInvoice ? <><Icons.check /> Copied</> : <><Icons.copy /> {invoiceCode ? 'Copy invoice ID' : 'Copy payment link'}</>}
                  </button>

                  <div className="merchant-link-box mono">
                    {invoiceCode || invoiceUrl || 'unipay1…'}
                  </div>
                </div>
              </div>

              <div className="stat-grid">
                <StatCard label="Available balance" value={balance.avail} unit="USDC" usd={balance.avail} accent action="Withdraw" spark onAction={() => openBridge({ srcChainId: 'initiation-2', srcDenom: 'uusdc' })} loading={overviewLoading && !overview} />
                <StatCard label="Total staked" value={balance.staked} unit="USDC" usd={balance.staked} sub={overview ? `across ${overview.pool_count} pool${overview.pool_count === 1 ? '' : 's'}` : undefined} loading={overviewLoading && !overview} />
                <StatCard label="LP yield (unrealized)" value={balance.yield} unit="USDC" usd={balance.yield} sub={`Target APY · ${apyPct.toFixed(2)}%`} highlight="success" loading={overviewLoading && !overview} />
              </div>
              {overviewError && !overview && (
                <div className="merchant-inline-note merchant-inline-note-error" style={{ marginTop: -8 }}>
                  Couldn't reach the staker API — start the merchant first or check VITE_STACKER_API_URL.
                </div>
              )}

              <div className="glass portfolio-summary" style={{ marginBottom: 20 }}>
                <div style={{ flex: 1 }}>
                  <div className="eyebrow">Portfolio total</div>
                  <div className="mono tnum" style={{ fontSize: 28, fontWeight: 500, letterSpacing: '-0.01em', marginTop: 4 }}>
                    {(balance.avail + balance.staked + balance.yield).toFixed(2)} <span style={{ color: 'var(--text-2)', fontSize: 16 }}>USDC</span>
                  </div>
                </div>
                <PortfolioBars avail={balance.avail} staked={balance.staked} yld={balance.yield} />
              </div>

              <PassiveEarnRow passive={passive} setPassive={setPassive} est={balance.avail * 0.201 / 12} />

              {claimError && (
                <div className="merchant-inline-note merchant-inline-note-error" style={{ marginBottom: 8 }}>
                  Claim failed: {claimError}
                </div>
              )}
              {claimSuccess && (
                <ClaimReceipt
                  txHash={claimSuccess.txHash}
                  claimedUinit={claimSuccess.claimedUinit}
                  onDismiss={() => setClaimSuccess(null)}
                />
              )}
              {realizeError && (
                <div className="merchant-inline-note merchant-inline-note-error" style={{ margin: '12px 4px' }}>
                  Realize failed: {realizeError}
                </div>
              )}
              {realizeSuccess && (
                <RealizeReceipt
                  txHash={realizeSuccess.txHash}
                  releaseAt={realizeSuccess.releaseAt}
                  usdcAmount={realizeSuccess.usdcAmount}
                  onDismiss={() => setRealizeSuccess(null)}
                />
              )}

              <RewardsLegend />

              <div className="overview-split">
                <div>
                  <div className="section-heading-row">
                    <div style={{ fontSize: 15, fontWeight: 500 }}>Your pools</div>
                    <button className="btn btn-ghost" style={{ fontSize: 13 }} onClick={() => setPage('pools')}>View all <Icons.arrow /></button>
                  </div>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
                    {poolsLoading && pools.length === 0 && (
                      <div className="glass" style={{ padding: 14, color: 'var(--text-2)', fontSize: 13 }}>Loading pools…</div>
                    )}
                    {!poolsLoading && pools.length === 0 && (
                      <div className="glass" style={{ padding: 14, color: 'var(--text-3)', fontSize: 13 }}>No active strategies yet.</div>
                    )}
                    {pools.slice(0, 3).map((p) => (
                      <PoolRow
                        key={p.id}
                        pool={p}
                        onClaim={() => onClaim(p.id)}
                        claiming={claiming === p.id}
                        onRealize={() => { void onRealize(p.id); }}
                        realizing={realizing === p.id}
                        claimableInitMicro={overview?.claimable_init_rewards}
                      />
                    ))}
                  </div>
                </div>

                <div>
                  <div className="section-heading-row">
                    <div style={{ fontSize: 15, fontWeight: 500 }}>Recent settlements</div>
                    <span className="pill pill-success"><span className="dot" style={{ animation: 'pulse-ring 1.6s infinite' }} />Live</span>
                  </div>
                  <div className="glass" style={{ padding: 6 }}>
                    {activityLoading && settlements.length === 0 && (
                      <div style={{ padding: 14, color: 'var(--text-2)', fontSize: 13 }}>Loading activity…</div>
                    )}
                    {!activityLoading && settlements.length === 0 && (
                      <div style={{ padding: 14, color: 'var(--text-3)', fontSize: 13 }}>No settlements yet.</div>
                    )}
                    {settlements.slice(0, 6).map((s) => <SettlementRow key={s.id} s={s} />)}
                  </div>
                </div>
              </div>

            </>
          )}

          {(page === 'pools' || page === 'earn') && (
            <>
              <PageHeader
                title="Pools & Earn"
                subtitle="Your staked positions and L1 bridging"
                actions={<button className="btn"><Icons.plus /> Stake USDC</button>}
              />
              {claimError && (
                <div className="merchant-inline-note merchant-inline-note-error" style={{ marginBottom: 8 }}>
                  Claim failed: {claimError}
                </div>
              )}
              {claimSuccess && (
                <ClaimReceipt
                  txHash={claimSuccess.txHash}
                  claimedUinit={claimSuccess.claimedUinit}
                  onDismiss={() => setClaimSuccess(null)}
                />
              )}
              <EarnPanel />
            </>
          )}

          {page === 'activity' && (
            <>
              <PageHeader title="Activity" subtitle="All incoming settlements" />
              {activityLoading && settlements.length === 0 && (
                <div className="glass" style={{ padding: 18, color: 'var(--text-2)' }}>Loading activity…</div>
              )}
              {!activityLoading && settlements.length === 0 && (
                <div className="glass" style={{ padding: 18, color: 'var(--text-3)' }}>No settlements yet.</div>
              )}
              {settlements.length > 0 && (
                <div
                  style={{
                    display: 'grid',
                    gridTemplateColumns: 'repeat(auto-fit, minmax(360px, 1fr))',
                    gap: 10,
                  }}
                >
                  {settlements.map((s) => (
                    <div key={s.id} className="glass" style={{ padding: 6 }}>
                      <SettlementRow s={s} />
                    </div>
                  ))}
                </div>
              )}
            </>
          )}

        </div>
      </div>
    </div>
  );
}

function PageHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: React.ReactNode }) {
  return (
    <div className="page-header page-header-large">
      <div className="page-header-copy">
        <h1 style={{ margin: 0, fontSize: 24, fontWeight: 500, letterSpacing: '-0.01em' }}>{title}</h1>
        {subtitle && <div style={{ color: 'var(--text-2)', fontSize: 14, marginTop: 4 }}>{subtitle}</div>}
      </div>
      {actions && <div className="page-header-actions">{actions}</div>}
    </div>
  );
}

function SummaryItem({ label, value }: { label: string; value: string }) {
  return (
    <div className="merchant-summary-item">
      <div className="merchant-summary-label">{label}</div>
      <div className="merchant-summary-value mono">{value}</div>
    </div>
  );
}

function StatCard({ label, value, unit, usd, sub, accent, highlight, action, spark, onAction, loading }: {
  label: string; value: number; unit: string; usd: number;
  sub?: string; accent?: boolean; highlight?: string; action?: string; spark?: boolean; onAction?: () => void; loading?: boolean;
}) {
  return (
    <div className="glass stat-card">
      {accent && <div style={{ position: 'absolute', top: -60, right: -60, width: 180, height: 180, borderRadius: '50%', background: 'var(--accent-glow)', filter: 'blur(50px)', pointerEvents: 'none' }} />}
      <div className="stat-card-header" style={{ position: 'relative' }}>
        <div className="eyebrow">{label}</div>
        {action && <button className="btn" style={{ fontSize: 11, padding: '4px 10px' }} onClick={onAction}>{action}</button>}
      </div>
      <div className="mono tnum" style={{ fontSize: 32, fontWeight: 500, letterSpacing: '-0.02em', marginTop: 10, position: 'relative', color: highlight === 'success' ? 'var(--success)' : 'var(--text-0)' }}>
        {loading ? '…' : value.toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 })}
        <span style={{ color: 'var(--text-2)', fontSize: 14, marginLeft: 6 }}>{unit}</span>
      </div>
      <div className="stat-card-footer" style={{ marginTop: 8, position: 'relative' }}>
        <span className="mono tnum" style={{ fontSize: 12, color: 'var(--text-2)' }}>≈ {fmtUSD(usd)}</span>
        {sub && <span className="mono" style={{ fontSize: 11, color: 'var(--text-3)' }}>{sub}</span>}
        {spark && <Sparkline />}
      </div>
    </div>
  );
}

function Sparkline() {
  const pts = [3, 5, 4, 7, 6, 8, 7, 9, 8, 10, 12, 11, 13].map((v, i) => `${i * 6},${16 - v}`).join(' ');
  return (
    <svg width="80" height="18" viewBox="0 0 80 18" fill="none">
      <polyline points={pts} stroke="var(--accent)" strokeWidth="1.4" fill="none" strokeLinecap="round" />
      <polyline points={pts + ' 78,18 0,18'} fill="var(--accent)" fillOpacity="0.15" />
    </svg>
  );
}

function PassiveEarnRow({ passive, setPassive, est }: { passive: boolean; setPassive: (v: boolean) => void; est: number }) {
  return (
    <div className="glass passive-earn-row">
      <div style={{ position: 'absolute', top: -40, right: -40, width: 180, height: 180, borderRadius: '50%', background: passive ? 'var(--accent-glow)' : 'transparent', filter: 'blur(50px)', transition: 'background 300ms', pointerEvents: 'none' }} />
      <div style={{ width: 38, height: 38, borderRadius: 10, background: 'var(--accent-soft)', display: 'grid', placeItems: 'center', color: passive ? 'var(--accent)' : 'var(--text-2)', transition: 'color 200ms' }}>
        <Icons.sparkle />
      </div>
      <div className="passive-earn-copy">
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <span style={{ fontSize: 15, fontWeight: 500 }}>Passive Earn</span>
          <span className={`pill ${passive ? 'pill-accent' : ''}`} style={{ fontSize: 10 }}>{passive ? 'ACTIVE' : 'OFF'}</span>
        </div>
        <div style={{ fontSize: 13, color: 'var(--text-2)', marginTop: 2 }}>
          Auto-stake incoming settlements into your top-APY pool.
        </div>
      </div>
      <div className="passive-earn-meta">
        <span style={{ fontSize: 13, color: 'var(--text-2)' }}>+{fmtUSD(est)}/mo</span>
        <div className={`toggle ${passive ? 'on' : ''}`} onClick={() => setPassive(!passive)}>
          <div className="knob" />
        </div>
      </div>
    </div>
  );
}

function PoolRow({
  pool,
  onClaim,
  claiming,
  onRealize,
  realizing,
  showManage,
  claimableInitMicro,
}: {
  pool: Pool;
  onClaim: () => void;
  claiming: boolean;
  onRealize?: () => void;
  realizing?: boolean;
  showManage?: boolean;
  claimableInitMicro?: string;
}) {
  const claimableInit = claimableInitMicro && claimableInitMicro !== '0'
    ? Number(claimableInitMicro) / 1e6
    : 0;
  return (
    <div className="glass lift" style={{ padding: '14px 16px' }}>
      <div className="pool-row-wrap">
        <div className="pool-row-main">
          <div style={{ display: 'flex' }}>
            <TokenLogo sym={pool.tokens[0]} size="md" />
            <div style={{ marginLeft: -8 }}><TokenLogo sym={pool.tokens[1]} size="md" /></div>
          </div>
          <div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
              <span style={{ fontSize: 14, fontWeight: 500 }}>{pool.name}</span>
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--text-3)', fontSize: 11 }}>
                <ChainLogo id={normalizeChainId(pool.chain)} size="sm" />
                <span>{CHAINS[normalizeChainId(pool.chain)]?.name ?? 'Initia'}</span>
              </span>
              {claimableInit > 0 && (
                <span
                  className="pill pill-accent"
                  style={{ fontSize: 10, padding: '2px 8px' }}
                  title="Accrued INIT staking rewards available to claim"
                >
                  Claim {claimableInit.toFixed(4)} INIT
                </span>
              )}
            </div>
            <div className="mono" style={{ fontSize: 11, color: 'var(--text-3)' }}>TVL · {pool.tvl}</div>
          </div>
        </div>
        <StatMini label="APY" value={`${pool.apy.toFixed(1)}%`} accent />
        <StatMini label="Staked" value={pool.staked.toFixed(2)} sub={fmtUSD(pool.staked)} />
        <StatMini label="LP yield" value={`+${pool.earned.toFixed(2)}`} sub={fmtUSD(pool.earned)} success />
        <div className="pool-row-spacer" />
        <div className="pool-row-actions">
          <button
            className="btn btn-primary"
            onClick={onClaim}
            disabled={claiming || realizing}
            title="Sweep accrued INIT staking rewards into your L1 wallet now. Principal stays staked."
          >
            {claiming ? <><Icons.spinner /> Claiming</> : 'Claim INIT rewards'}
          </button>
          {onRealize && (
            <button
              className="btn"
              onClick={onRealize}
              disabled={claiming || realizing}
              title="Unstake your LP shares so the underlying USDC + INIT can be withdrawn after the unbond window."
            >
              {realizing ? <><Icons.spinner /> Unbonding</> : 'Realize USDC'}
            </button>
          )}
          {showManage && <button className="btn">Manage</button>}
        </div>
      </div>
    </div>
  );
}

function ClaimReceipt({
  txHash,
  claimedUinit,
  onDismiss,
}: {
  txHash: string;
  claimedUinit: bigint | null;
  onDismiss: () => void;
}) {
  const explorerUrl = `${L1_TX_EXPLORER_URL.replace(/\/$/, '')}/${txHash}`;
  const claimedInit = claimedUinit !== null ? Number(claimedUinit) / 1e6 : null;
  const amountLabel =
    claimedInit === null
      ? 'Looking up claimed amount on L1…'
      : claimedInit > 0
        ? `Claimed ${claimedInit.toLocaleString('en-US', { maximumFractionDigits: 6 })} INIT (validator rewards) to your L1 wallet.`
        : 'Tx confirmed on L1, but no INIT rewards had accrued yet at the moment of the claim. Your principal is still staked.';

  return (
    <div
      className="glass"
      style={{
        margin: '12px 4px',
        padding: '14px 18px',
        display: 'flex',
        gap: 16,
        alignItems: 'center',
        border: '1px solid rgba(125, 233, 154, 0.26)',
        background:
          'linear-gradient(135deg, rgba(52, 124, 74, 0.22), rgba(21, 35, 28, 0.7))',
      }}
    >
      <div style={{ flex: 1, display: 'grid', gap: 4 }}>
        <strong style={{ fontWeight: 500, color: '#d9ffe5' }}>Claim confirmed on L1</strong>
        <span style={{ fontSize: 13, color: 'var(--text-2)' }}>{amountLabel}</span>
        <span style={{ fontSize: 12, color: 'var(--text-3)' }}>
          Principal staked and LP yield numbers don't change — those are realized via "Realize USDC".
        </span>
        <span className="mono" style={{ fontSize: 12, color: 'var(--text-3)', wordBreak: 'break-all' }}>
          tx · {txHash}
        </span>
      </div>
      <div style={{ display: 'flex', gap: 8, flexShrink: 0 }}>
        <a className="btn btn-primary" href={explorerUrl} target="_blank" rel="noreferrer">
          Verify on explorer
        </a>
        <button className="btn" onClick={onDismiss}>Dismiss</button>
      </div>
    </div>
  );
}

function RewardsLegend() {
  return (
    <div
      className="glass"
      style={{
        margin: '0 4px 12px',
        padding: '12px 16px',
        display: 'grid',
        gap: 8,
        background: 'linear-gradient(135deg, rgba(40, 60, 50, 0.18), rgba(20, 28, 24, 0.5))',
      }}
    >
      <div style={{ fontSize: 12, color: 'var(--text-3)', letterSpacing: '0.06em' }}>HOW REWARDS WORK</div>
      <div style={{ display: 'grid', gridTemplateColumns: '1fr 1fr', gap: 16 }}>
        <div>
          <div style={{ fontSize: 13, fontWeight: 500, color: '#d9ffe5' }}>Claim INIT rewards</div>
          <div style={{ fontSize: 12, color: 'var(--text-2)', marginTop: 2 }}>
            Sweeps validator staking rewards (paid in INIT) into your L1 wallet right now.
            Principal stays staked. Instant.
          </div>
        </div>
        <div>
          <div style={{ fontSize: 13, fontWeight: 500, color: '#d9ffe5' }}>Realize USDC</div>
          <div style={{ fontSize: 12, color: 'var(--text-2)', marginTop: 2 }}>
            Unstakes the LP so principal + LP yield can be redeemed as USDC. Funds become
            withdrawable after the chain's unbond window (~14 days on testnet).
          </div>
        </div>
      </div>
    </div>
  );
}

function RealizeReceipt({
  txHash,
  releaseAt,
  usdcAmount,
  onDismiss,
}: {
  txHash: string;
  releaseAt: string;
  usdcAmount: number;
  onDismiss: () => void;
}) {
  const explorerUrl = `${L1_TX_EXPLORER_URL.replace(/\/$/, '')}/${txHash}`;
  const releaseDate = new Date(releaseAt);
  const remainingMs = releaseDate.getTime() - Date.now();
  const remainingDays = Math.max(0, Math.ceil(remainingMs / (24 * 60 * 60 * 1000)));
  const releaseLabel = releaseDate.toLocaleString();

  return (
    <div
      className="glass"
      style={{
        margin: '12px 4px',
        padding: '14px 18px',
        display: 'flex',
        gap: 16,
        alignItems: 'center',
        border: '1px solid rgba(108, 197, 225, 0.26)',
        background:
          'linear-gradient(135deg, rgba(46, 96, 118, 0.22), rgba(21, 30, 35, 0.7))',
      }}
    >
      <div style={{ flex: 1, display: 'grid', gap: 4 }}>
        <strong style={{ fontWeight: 500, color: '#d9efff' }}>Unbonding initiated on L1</strong>
        <span style={{ fontSize: 13, color: 'var(--text-2)' }}>
          {usdcAmount > 0
            ? `~${usdcAmount.toLocaleString('en-US', { maximumFractionDigits: 4 })} USDC`
            : 'Your principal'} will be claimable in ~{remainingDays} day{remainingDays === 1 ? '' : 's'}
          {' '}(after {releaseLabel}). Until then it stays bonded and keeps earning.
        </span>
        <span className="mono" style={{ fontSize: 12, color: 'var(--text-3)', wordBreak: 'break-all' }}>
          tx · {txHash}
        </span>
      </div>
      <div style={{ display: 'flex', gap: 8, flexShrink: 0 }}>
        <a className="btn btn-primary" href={explorerUrl} target="_blank" rel="noreferrer">
          Verify on explorer
        </a>
        <button className="btn" onClick={onDismiss}>Dismiss</button>
      </div>
    </div>
  );
}

function StatMini({ label, value, sub, accent, success }: { label: string; value: string; sub?: string; accent?: boolean; success?: boolean }) {
  return (
    <div className="stat-mini">
      <div style={{ fontSize: 11, color: 'var(--text-3)', marginBottom: 2 }}>{label}</div>
      <div className="tnum" style={{ fontSize: 15, fontWeight: 500, color: accent ? 'var(--accent)' : success ? 'var(--success)' : 'var(--text-0)' }}>{value}</div>
      {sub && <div className="tnum" style={{ fontSize: 11, color: 'var(--text-3)' }}>{sub}</div>}
    </div>
  );
}

function PortfolioBars({ avail, staked, yld }: { avail: number; staked: number; yld: number }) {
  const total = avail + staked + yld;
  const sPct = staked / total * 100, aPct = avail / total * 100, yPct = yld / total * 100;
  return (
    <div className="portfolio-bars">
      <div style={{ display: 'flex', height: 8, borderRadius: 999, overflow: 'hidden', border: '1px solid var(--hairline)' }}>
        <div style={{ width: `${sPct}%`, background: 'var(--accent)' }} />
        <div style={{ width: `${aPct}%`, background: 'var(--text-2)', opacity: 0.5 }} />
        <div style={{ width: `${yPct}%`, background: 'var(--success)' }} />
      </div>
      <div className="mono portfolio-legend">
        <span><span style={{ display: 'inline-block', width: 8, height: 8, background: 'var(--accent)', borderRadius: 2, marginRight: 6, verticalAlign: 'middle' }} />Staked {sPct.toFixed(0)}%</span>
        <span><span style={{ display: 'inline-block', width: 8, height: 8, background: 'var(--text-2)', opacity: 0.5, borderRadius: 2, marginRight: 6, verticalAlign: 'middle' }} />Available {aPct.toFixed(0)}%</span>
        <span><span style={{ display: 'inline-block', width: 8, height: 8, background: 'var(--success)', borderRadius: 2, marginRight: 6, verticalAlign: 'middle' }} />Yield {yPct.toFixed(0)}%</span>
      </div>
    </div>
  );
}

function SettlementRow({ s }: { s: Settlement }) {
  const chain = CHAINS[s.srcChain];
  return (
    <div className="lift settlement-row">
      <div style={{ display: 'flex', alignItems: 'center', flexShrink: 0 }}>
        <TokenLogo sym={s.token} size="md" />
      </div>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div className="tnum" style={{ fontSize: 14, fontWeight: 500 }}>+{s.amount.toFixed(2)} <span style={{ color: 'var(--text-2)', fontWeight: 400 }}>{s.token}</span></div>
        <div className="mono" style={{ fontSize: 12, color: 'var(--text-3)' }}>{chain?.name ?? s.srcChain} · {relTime(s.ts)}</div>
      </div>
      <span className={`pill ${s.staked ? 'pill-accent' : ''}`} style={{ fontSize: 11 }}>{s.staked ? 'Staked' : 'Liquid'}</span>
    </div>
  );
}
