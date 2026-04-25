import { useState } from 'react';
import type { HistoryTx } from '../../types';
import { CHAINS, TOKENS, fmtUSD, relTime } from '../../data';
import { TokenLogo, ChainLogo } from '../shared/TokenLogo';
import { HashLink } from '../shared/HashLink';
import { Icons } from '../shared/Icons';

interface Props { history: HistoryTx[]; onClose: () => void; }

export function HistoryDrawer({ history, onClose }: Props) {
  const [expanded, setExpanded] = useState<string | null>(null);

  return (
    <>
      <div className="drawer-backdrop" onClick={onClose} />
      <div className="drawer" style={{ animation: 'slide-in 320ms cubic-bezier(.2,.8,.2,1)' }}>
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', padding: '20px 22px', borderBottom: '1px solid var(--hairline)' }}>
          <div>
            <div className="eyebrow" style={{ marginBottom: 4 }}>Transactions</div>
            <div style={{ fontSize: 18, fontWeight: 500 }}>Payment history</div>
          </div>
          <button className="iconbtn" onClick={onClose}><Icons.x /></button>
        </div>
        <div style={{ flex: 1, overflowY: 'auto', padding: '10px 12px' }}>
          {history.map((tx) => (
            <HistoryRow
              key={tx.id}
              tx={tx}
              expanded={expanded === tx.id}
              onToggle={() => setExpanded((e) => (e === tx.id ? null : tx.id))}
            />
          ))}
        </div>
        <div style={{ padding: '12px 22px', borderTop: '1px solid var(--hairline)', display: 'flex', justifyContent: 'space-between', color: 'var(--text-3)', fontSize: 11 }} className="mono">
          <span>{history.length} TRANSACTIONS</span>
          <span>EXPORT CSV</span>
        </div>
      </div>
    </>
  );
}

function HistoryRow({ tx, expanded, onToggle }: { tx: HistoryTx; expanded: boolean; onToggle: () => void }) {
  const chain = CHAINS[tx.chain];
  const pillClass = tx.status === 'Settled' ? 'pill-success' : tx.status === 'Pending' ? 'pill-warn' : 'pill-danger';

  return (
    <div className="lift" style={{ padding: '12px 10px', borderRadius: 10, cursor: 'pointer', marginBottom: 4 }} onClick={onToggle}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <div style={{ position: 'relative' }}>
          <TokenLogo sym={tx.token} />
          <div style={{ position: 'absolute', bottom: -2, right: -2 }}><ChainLogo id={tx.chain} size="sm" /></div>
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ fontSize: 13, fontWeight: 500 }} className="tnum">{tx.amount} {tx.token}</div>
          <div className="mono" style={{ fontSize: 11, color: 'var(--text-3)' }}>{chain.name} · {relTime(tx.ts)}</div>
        </div>
        <div style={{ textAlign: 'right' }}>
          <span className={`pill ${pillClass}`}><span className="dot" />{tx.status}</span>
          <div className="mono tnum" style={{ fontSize: 11, color: 'var(--text-2)', marginTop: 4 }}>→ {tx.initAmount.toFixed(3)} INIT</div>
        </div>
      </div>
      {expanded && (
        <div style={{ marginTop: 10, padding: 12, background: 'rgba(255,255,255,0.02)', border: '1px solid var(--hairline)', borderRadius: 8, fontSize: 12, display: 'flex', flexDirection: 'column', gap: 8 }}>
          <HashLink hash={tx.srcHash} label={`${chain.name} tx ·`} />
          {tx.initHash
            ? <HashLink hash={tx.initHash} label="Initia settlement ·" />
            : <span className="mono" style={{ color: 'var(--text-3)' }}>Initia settlement · pending…</span>}
          <div style={{ display: 'flex', justifyContent: 'space-between', color: 'var(--text-2)' }}>
            <span>USD value</span>
            <span className="mono tnum" style={{ color: 'var(--text-0)' }}>{fmtUSD(tx.amount * (TOKENS[tx.token]?.price || 1))}</span>
          </div>
          <div style={{ display: 'flex', justifyContent: 'space-between', color: 'var(--text-2)' }}>
            <span>Fee</span>
            <span className="mono tnum" style={{ color: 'var(--text-0)' }}>{fmtUSD(tx.amount * 0.002)}</span>
          </div>
        </div>
      )}
    </div>
  );
}
