import React from 'react';
import { CHAINS, fmtUSD, relTime, shortAddr } from '../../data';

function fmtPrice(n: number): string {
  if (!isFinite(n) || n === 0) return fmtUSD(n);
  const abs = Math.abs(n);
  const digits = abs >= 1 ? 2 : abs >= 0.01 ? 4 : 6;
  return '$' + n.toLocaleString('en-US', { minimumFractionDigits: digits, maximumFractionDigits: digits });
}
import type { HistoryTx } from '../../types';
import { TokenLogo } from '../shared/TokenLogo';

interface Props {
  tx: HistoryTx;
  onBack: () => void;
}

export function TxDetail({ tx, onBack }: Props) {
  const chain = CHAINS[tx.chain] ?? { name: tx.chain, short: tx.chain, explorer: '', addrExplorer: '' };
  const destKey = tx.destChain ?? '';
  const initia = CHAINS[destKey] ?? { name: destKey, short: destKey, explorer: '', addrExplorer: '' };
  const dateStr = new Date(tx.ts).toUTCString().replace('GMT', 'UTC');
  const sourceLegStatus: LegStatus = tx.srcRefundHash
    ? 'Refunded'
    : tx.srcRedeemHash
    ? 'Redeemed'
    : tx.srcInitiateHash
    ? 'Initiated'
    : 'Pending';
  const receiveLegStatus: LegStatus = tx.dstRefundHash
    ? 'Refunded'
    : tx.dstRedeemHash || tx.status === 'Settled'
    ? 'Redeemed'
    : tx.status === 'Refunded'
    ? '—'
    : tx.dstInitiateHash
    ? 'Initiated'
    : 'Pending';

  return (
    <div>
      <div className="tx-header">
        <h1 style={{ margin: 0, fontSize: 20, fontWeight: 500, letterSpacing: '-0.01em' }}>Transaction details</h1>
        <button onClick={onBack} className="tx-back" aria-label="Back">
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none"><path d="M13 8H3M7 4L3 8l4 4" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /></svg>
        </button>
      </div>

      <div className="tx-grid">
        {/* LEFT — overview */}
        <div className="tx-card">
          <div style={{ padding: '18px 20px', borderBottom: '1px solid var(--hairline)' }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 12 }}>
              <div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
                  <span className="tnum" style={{ fontSize: 16, fontWeight: 500 }}>{tx.amount} {tx.token}</span>
                  <TokenLogo sym={tx.token} size="sm" chain={tx.chain} />
                </div>
                <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 8 }}>
                  <span style={{ color: 'var(--text-3)' }}>→</span>
                  <span className="tnum" style={{ fontSize: 16, fontWeight: 500 }}>{tx.initAmount.toFixed(4)} {tx.destToken || ''}</span>
                  <TokenLogo sym={tx.destToken || ''} size="sm" chain={destKey} />
                </div>
              </div>
              <StatusPill status={tx.status} />
            </div>
          </div>

          {tx.orderId && <TxInfoRow icon={<IconGlyph.orderId />} label="Order ID" value={<CopyValue text={tx.orderId} display={shortAddr(tx.orderId)} />} />}
          <TxInfoRow
            icon={<IconGlyph.rate />}
            label="Rate"
            value={<span className="tnum">1 {tx.token} → {(tx.initAmount / (tx.amount || 1)).toFixed(6)} {tx.destToken || ''}</span>}
          />
          <TxInfoRow icon={<IconGlyph.clock />} label="Created at" value={<span>{dateStr}</span>} />
          {tx.refundAddr && (
            <TxInfoRow
              icon={<IconGlyph.refund />}
              label="Refund address"
              value={<ExplorerLink href={(chain.addrExplorer || '') + tx.refundAddr} display={shortAddr(tx.refundAddr)} copy={tx.refundAddr} />}
            />
          )}
          {tx.destinationAddr && (
            <TxInfoRow
              icon={<IconGlyph.destination />}
              label="Destination address"
              value={<ExplorerLink href={(initia.addrExplorer || '') + tx.destinationAddr} display={shortAddr(tx.destinationAddr)} copy={tx.destinationAddr} />}
              last
            />
          )}
        </div>

        {/* RIGHT — two legs */}
        <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
          <TxLegCard
            title="Send"
            amount={`${tx.amount} ${tx.token}`}
            tokenSym={tx.token}
            chainId={tx.chain}
            legStatus={sourceLegStatus}
            chain={chain}
            initiatorLabel="Initiator address"
            initiator={tx.srcInitiator}
            redeemerLabel="Redeemer address"
            redeemer={tx.srcRedeemer}
            initiateHash={tx.srcInitiateHash}
            redeemHash={tx.srcRedeemHash}
            refundHash={tx.srcRefundHash}
            orderId={tx.orderId}
            price={tx.srcPrice}
            unitPrice={tx.srcUnitPrice}
            ts={tx.ts}
          />
          <TxLegCard
            title="Receive"
            amount={`${tx.initAmount.toFixed(4)} ${tx.destToken || ''}`}
            tokenSym={tx.destToken || ''}
            chainId={destKey}
            legStatus={receiveLegStatus}
            chain={initia}
            initiatorLabel="Initiator pub key"
            initiator={tx.dstInitiator}
            redeemerLabel="Redeemer pub key"
            redeemer={tx.dstRedeemer}
            initiateHash={tx.dstInitiateHash}
            redeemHash={tx.dstRedeemHash}
            refundHash={tx.dstRefundHash}
            orderId={tx.orderId}
            price={tx.dstPrice}
            unitPrice={tx.dstUnitPrice}
            ts={tx.ts}
          />
        </div>
      </div>
    </div>
  );
}

function TxInfoRow({ icon, label, value, last }: { icon: React.ReactNode; label: string; value: React.ReactNode; last?: boolean }) {
  return (
    <div style={{ padding: '14px 20px', borderBottom: last ? 'none' : '1px solid var(--hairline)' }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 6, color: 'var(--text-3)', fontSize: 12 }}>
        {icon}
        <span>{label}</span>
      </div>
      <div style={{ marginTop: 6, fontSize: 13.5, color: 'var(--text-0)' }}>{value}</div>
    </div>
  );
}

function StatusPill({ status }: { status: HistoryTx['status'] }) {
  const map: Record<HistoryTx['status'], { cls: string; label: string }> = {
    Settled: { cls: 'pill-success', label: 'Completed' },
    Pending: { cls: 'pill-warn', label: 'Pending' },
    Refunded: { cls: 'pill-danger', label: 'Refunded' },
  };
  const m = map[status];
  return (
    <span className={`pill ${m.cls}`} style={{ padding: '4px 10px' }}>
      {m.label}
      <svg width="10" height="10" viewBox="0 0 12 12" fill="none" style={{ marginLeft: 4 }}>
        <path d="M2.5 6.5l2.5 2.5 4.5-5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    </span>
  );
}

type LegStatus = 'Redeemed' | 'Refunded' | 'Initiated' | 'Pending' | '—';

function resolveOutcomeHash(legStatus: LegStatus, redeemHash?: string, refundHash?: string) {
  if (legStatus === 'Refunded') {
    return { label: 'Refund tx hash', hash: refundHash ?? redeemHash };
  }
  if (legStatus === 'Redeemed') {
    return { label: 'Redeem tx hash', hash: redeemHash ?? refundHash };
  }
  if (refundHash && !redeemHash) {
    return { label: 'Refund tx hash', hash: refundHash };
  }
  return { label: 'Redeem tx hash', hash: redeemHash };
}

function TxLegCard({
  title, amount, tokenSym, chainId, legStatus, chain, initiatorLabel, initiator, redeemerLabel,
  redeemer, initiateHash, redeemHash, refundHash, orderId, price, unitPrice, ts,
}: {
  title: string; amount: string; tokenSym: string; chainId: string; legStatus: LegStatus;
  chain: { name: string; explorer?: string; addrExplorer?: string };
  initiatorLabel: string; initiator?: string; redeemerLabel: string; redeemer?: string;
  initiateHash?: string; redeemHash?: string; refundHash?: string; orderId?: string; price?: number; unitPrice?: number; ts: number;
}) {
  const [expanded, setExpanded] = React.useState(false);
  const outcomeHash = resolveOutcomeHash(legStatus, redeemHash, refundHash);
  const pillCls = legStatus === 'Redeemed'
    ? 'pill-success'
    : legStatus === 'Refunded'
    ? 'pill-danger'
    : legStatus === '—'
    ? 'pill-neutral'
    : 'pill-warn';

  return (
    <div className="tx-card">
      <div className="tx-leg-header" style={{ padding: '14px 20px', borderBottom: '1px solid var(--hairline)' }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
          <span style={{ fontSize: 13, fontWeight: 500 }}>{title}</span>
          <span className="tnum" style={{ fontSize: 14, color: 'var(--text-0)' }}>{amount}</span>
          <TokenLogo sym={tokenSym} size="sm" chain={chainId} />
        </div>
        <span className={`pill ${pillCls}`}>{legStatus}</span>
      </div>

      <div className="tx-leg-pair" style={{ padding: '20px 20px 10px' }}>
        <AddrBlock label={initiatorLabel} value={initiator} href={initiator ? (chain.addrExplorer || '') + initiator : undefined} />
        <AddrBlock label="Initiate tx hash" value={initiateHash} href={initiateHash ? (chain.explorer || '') + initiateHash : undefined} mono align="right" />
      </div>

      <div style={{ position: 'relative', margin: '16px 24px', height: 24 }}>
        <div style={{ position: 'absolute', top: '50%', left: 0, right: 0, height: 1, background: 'var(--hairline)', transform: 'translateY(-50%)' }} />
        <div
          style={{
            position: 'absolute',
            left: '50%',
            top: '50%',
            transform: 'translate(-50%, -50%)',
            width: 26,
            height: 26,
            borderRadius: '50%',
            background: 'var(--bg-1, #0b0b0d)',
            border: '1px solid var(--hairline)',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            color: 'var(--text-3)',
          }}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><path d="M6 2v8M3 7l3 3 3-3" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" /></svg>
        </div>
      </div>

      <div className="tx-leg-pair" style={{ padding: '10px 20px 22px' }}>
        <AddrBlock label={redeemerLabel} value={redeemer} href={redeemer ? (chain.addrExplorer || '') + redeemer : undefined} />
        <AddrBlock
          label={outcomeHash.label}
          value={outcomeHash.hash}
          href={outcomeHash.hash ? (chain.explorer || '') + outcomeHash.hash : undefined}
          mono
          align="right"
        />
      </div>

      <div className="tx-leg-stats" style={{ padding: '0 20px 18px' }}>
        <MiniStat label="Order ID" value={orderId ? <CopyValue text={orderId} display={shortAddr(orderId)} /> : <span style={{ color: 'var(--text-3)' }}>—</span>} />
        <MiniStat label="Price" value={price !== undefined ? <span className="tnum">{fmtPrice(price)}</span> : <span style={{ color: 'var(--text-3)' }}>—</span>} />
        <MiniStat label="Created" value={<span>{relTime(ts)}</span>} />
      </div>

      <button className="tx-viewmore" onClick={() => setExpanded((e) => !e)}>
        View {expanded ? 'less' : 'more'}
        <svg width="11" height="11" viewBox="0 0 12 12" fill="none" style={{ marginLeft: 4, transform: expanded ? 'rotate(180deg)' : 'none', transition: 'transform 180ms' }}>
          <path d="M3 4.5L6 7.5L9 4.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>

      {expanded && (
        <div style={{ padding: '14px 20px 18px', fontSize: 12.5, color: 'var(--text-2)', borderTop: '1px solid var(--hairline)' }}>
          <div style={{ display: 'flex', justifyContent: 'space-between', padding: '4px 0' }}>
            <span>Network</span><span style={{ color: 'var(--text-0)' }}>{chain.name}</span>
          </div>
          <div style={{ display: 'flex', justifyContent: 'space-between', padding: '4px 0' }}>
            <span>Confirmations</span>
            <span className="tnum" style={{ color: 'var(--text-0)' }}>
              {legStatus === 'Redeemed' ? '128 / 128' : legStatus === 'Refunded' ? '—' : '—'}
            </span>
          </div>
          {unitPrice !== undefined && (
            <div style={{ display: 'flex', justifyContent: 'space-between', padding: '4px 0' }}>
              <span>Token price</span><span className="tnum" style={{ color: 'var(--text-0)' }}>{fmtPrice(unitPrice)} / {tokenSym}</span>
            </div>
          )}
          {price !== undefined && (
            <div style={{ display: 'flex', justifyContent: 'space-between', padding: '4px 0' }}>
              <span>Value</span><span className="tnum" style={{ color: 'var(--text-0)' }}>{fmtPrice(price)}</span>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function AddrBlock({ label, value, href, mono, align }: { label: string; value?: string; href?: string; mono?: boolean; align?: 'left' | 'right' }) {
  return (
    <div className="tx-addr-block" style={{ textAlign: align ?? 'left' }}>
      <div style={{ fontSize: 12, color: 'var(--text-3)', marginBottom: 6 }}>{label}</div>
      {value
        ? <ExplorerLink href={href} display={shortAddr(value)} copy={value} mono={mono} />
        : <span style={{ fontSize: 13, color: 'var(--text-3)' }}>—</span>}
    </div>
  );
}

function ExplorerLink({ href, display, copy, mono }: { href?: string; display: string; copy?: string; mono?: boolean }) {
  const [copied, setCopied] = React.useState(false);
  const onCopy = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (copy) {
      navigator.clipboard.writeText(copy).catch(() => {});
      setCopied(true);
      setTimeout(() => setCopied(false), 1100);
    }
  };
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 13.5, fontFamily: mono ? 'var(--font-mono)' : 'inherit' }}>
      {href
        ? (
          <a href={href} target="_blank" rel="noopener noreferrer" className="tx-link">
            {display}
            <svg width="10" height="10" viewBox="0 0 12 12" fill="none" style={{ marginLeft: 3, opacity: 0.6 }}>
              <path d="M4 2h6v6M10 2L4 8M4 5v5H1V5h3" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </a>
        )
        : <span style={{ color: 'var(--text-0)' }}>{display}</span>}
      {copy && (
        <button onClick={onCopy} className="tx-copy" title={copied ? 'Copied' : 'Copy'} aria-label="Copy">
          {copied
            ? <svg width="11" height="11" viewBox="0 0 12 12" fill="none"><path d="M2.5 6.5l2.5 2.5 4.5-5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" /></svg>
            : <svg width="11" height="11" viewBox="0 0 12 12" fill="none"><rect x="3.5" y="3.5" width="6" height="6" rx="1" stroke="currentColor" strokeWidth="1.2" /><path d="M2 7V2.5a1 1 0 011-1H7" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" /></svg>}
        </button>
      )}
    </span>
  );
}

function CopyValue({ text, display }: { text: string; display: string }) {
  const [copied, setCopied] = React.useState(false);
  const onCopy = () => {
    navigator.clipboard.writeText(text).catch(() => {});
    setCopied(true);
    setTimeout(() => setCopied(false), 1100);
  };
  return (
    <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6, fontSize: 13.5, color: 'var(--text-0)', fontFamily: 'var(--font-mono)' }}>
      {display}
      <button onClick={onCopy} className="tx-copy" title={copied ? 'Copied' : 'Copy'}>
        {copied
          ? <svg width="11" height="11" viewBox="0 0 12 12" fill="none"><path d="M2.5 6.5l2.5 2.5 4.5-5" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" /></svg>
          : <svg width="11" height="11" viewBox="0 0 12 12" fill="none"><rect x="3.5" y="3.5" width="6" height="6" rx="1" stroke="currentColor" strokeWidth="1.2" /><path d="M2 7V2.5a1 1 0 011-1H7" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" /></svg>}
      </button>
    </span>
  );
}

function MiniStat({ label, value }: { label: string; value: React.ReactNode }) {
  return (
    <div
      style={{
        padding: '14px 16px',
        background: 'rgba(255,255,255,0.025)',
        border: '1px solid var(--hairline)',
        borderRadius: 12,
        minHeight: 66,
        display: 'flex',
        flexDirection: 'column',
        justifyContent: 'center',
        gap: 6,
      }}
    >
      <div style={{ fontSize: 12, color: 'var(--text-3)', letterSpacing: '0.01em' }}>{label}</div>
      <div style={{ fontSize: 14, color: 'var(--text-0)', fontWeight: 500 }}>{value}</div>
    </div>
  );
}

const IconGlyph = {
  orderId: () => <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><rect x="2" y="2" width="8" height="8" rx="1.5" stroke="currentColor" strokeWidth="1.1" /><path d="M4 5h4M4 7h2.5" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" /></svg>,
  check: () => <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><circle cx="6" cy="6" r="4.5" stroke="currentColor" strokeWidth="1.1" /><path d="M4 6l1.5 1.5L8 4.8" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round" /></svg>,
  rate: () => <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><path d="M2 4h6M6 2l2 2-2 2M10 8H4M6 10l-2-2 2-2" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round" /></svg>,
  clock: () => <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><circle cx="6" cy="6" r="4.5" stroke="currentColor" strokeWidth="1.1" /><path d="M6 3.5V6l1.6 1" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" /></svg>,
  refund: () => <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><circle cx="6" cy="6" r="4.5" stroke="currentColor" strokeWidth="1.1" /><path d="M7.5 4.5L4.5 7.5M4.5 5v2.5H7" stroke="currentColor" strokeWidth="1.1" strokeLinecap="round" strokeLinejoin="round" /></svg>,
  destination: () => <svg width="12" height="12" viewBox="0 0 12 12" fill="none"><circle cx="6" cy="5" r="2" stroke="currentColor" strokeWidth="1.1" /><path d="M6 10C3 7.5 2 6.5 2 5a4 4 0 118 0c0 1.5-1 2.5-4 5z" stroke="currentColor" strokeWidth="1.1" strokeLinejoin="round" /></svg>,
};
