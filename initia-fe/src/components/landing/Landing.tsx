import { useEffect, useState } from 'react';
import type { PersonaType } from '../../types';
import { useStrategies, type Strategy } from '../../hooks/useStrategies';
import { QUOTE_API } from '../../lib/config';
import { Icons } from '../shared/Icons';

interface Props { onPick: (p: PersonaType) => void; }

interface RecentOrder {
  created_at: string;
  source_swap?: {
    redeem_timestamp?: string | null;
  };
  destination_swap?: {
    redeem_timestamp?: string | null;
  };
}

interface RecentOrdersResponse {
  ok: boolean;
  data?: {
    data?: RecentOrder[];
  };
}

function formatRecentSettle(seconds: number | null): string {
  if (seconds === null || !Number.isFinite(seconds)) return '—';
  if (seconds < 60) return `~${Math.round(seconds)}s`;
  const minutes = seconds / 60;
  if (minutes < 10) return `~${minutes.toFixed(1)}m`;
  return `~${Math.round(minutes)}m`;
}

function buildCustomerStats(
  strategies: Strategy[],
  strategiesReady: boolean,
  recentSettleSeconds: number | null,
  recentSettleReady: boolean,
): { k: string; v: string }[] {
  const sourceChainCount = new Set(strategies.map((strategy) => strategy.sourceChain)).size;
  const payAssetCount = new Set(
    strategies.map((strategy) => `${strategy.sourceChain}:${strategy.sourceTokenId}`),
  ).size;

  return [
    { k: 'Chains', v: strategiesReady ? String(sourceChainCount) : '—' },
    { k: 'Assets', v: strategiesReady ? String(payAssetCount) : '—' },
    { k: 'Recent avg', v: recentSettleReady ? formatRecentSettle(recentSettleSeconds) : '—' },
  ];
}

export function Landing({ onPick }: Props) {
  const [hover, setHover] = useState<PersonaType>(null);
  const [buildBadge] = useState(() => `SSZ-${Math.floor(Math.random() * 9000 + 1000)}`);
  const { strategies, loading: strategiesLoading, error: strategiesError } = useStrategies();
  const [recentSettleSeconds, setRecentSettleSeconds] = useState<number | null>(null);
  const [recentSettleLoading, setRecentSettleLoading] = useState(true);

  useEffect(() => {
    const controller = new AbortController();

    async function fetchRecentSettles() {
      setRecentSettleLoading(true);
      try {
        const res = await fetch(`${QUOTE_API}/orders?page=1&per_page=5&status=completed`, {
          signal: controller.signal,
        });
        if (!res.ok) throw new Error(`${res.status} ${res.statusText}`);
        const json: RecentOrdersResponse = await res.json();
        const recentOrders = json.data?.data ?? [];
        const durations = recentOrders
          .map((order) => {
            const start = Date.parse(order.created_at);
            const settledAt = Date.parse(
              order.destination_swap?.redeem_timestamp
              ?? order.source_swap?.redeem_timestamp
              ?? '',
            );
            if (!Number.isFinite(start) || !Number.isFinite(settledAt) || settledAt <= start) {
              return null;
            }
            return (settledAt - start) / 1000;
          })
          .filter((value): value is number => value !== null);

        setRecentSettleSeconds(
          durations.length > 0
            ? durations.reduce((sum, value) => sum + value, 0) / durations.length
            : null,
        );
      } catch (error) {
        if (!controller.signal.aborted) setRecentSettleSeconds(null);
      } finally {
        if (!controller.signal.aborted) setRecentSettleLoading(false);
      }
    }

    void fetchRecentSettles();

    return () => controller.abort();
  }, []);

  const customerStats = buildCustomerStats(
    strategies,
    !strategiesLoading && !strategiesError,
    recentSettleSeconds,
    !recentSettleLoading,
  );

  return (
    <div className="landing" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column' }}>
      <div
        className="ambient"
        style={{
          '--g1': hover === 'merchant' ? 'rgba(120,180,120,0.16)' : 'rgba(80,120,220,0.18)',
          '--g2': hover === 'merchant' ? 'rgba(160,140,80,0.14)' : 'rgba(60,100,200,0.14)',
        } as React.CSSProperties}
      />
      <div className="gridlines" />

      <div className="landing-body">
        <div className="landing-hero">
          <div className="eyebrow" style={{ marginBottom: 12 }}>One protocol · Any chain · INIT settlement</div>
          <h1 className="landing-h1">
            Pay anything.<br />
            <span style={{ color: 'var(--text-2)' }}>Settle in INIT.</span>
          </h1>
          <p className="landing-copy">
            Customers pay from any wallet, any chain. Merchants receive INIT and optionally auto-stake into Initia DEX pools — all in one transaction.
          </p>
        </div>

        <div className="persona-cards-grid">
          <PersonaCard
            kind="customer"
            title="Pay"
            eyebrow="Customer"
            desc="Send any token from any chain. We handle conversion and settlement to the merchant in INIT."
            stats={customerStats}
            onHover={setHover}
            onClick={() => onPick('customer')}
          />
          <PersonaCard
            kind="merchant"
            title="Merchant"
            eyebrow="Merchant"
            desc="Accept universal payments. Auto-stake settlements into LP pools and earn passively on every transaction."
            stats={[{ k: 'Avg APY', v: '18.4%' }, { k: 'Pools', v: '24' }, { k: 'Merchants', v: '2,140' }]}
            onHover={setHover}
            onClick={() => onPick('merchant')}
          />
        </div>

        <div className="landing-hints mono">
          <span>↵  pick a role to continue</span>
          <span style={{ color: 'var(--text-4)' }}>—</span>
          <span>switch roles anytime via the header</span>
        </div>
      </div>

      <div className="landing-footer mono">
        <span>INITIA · UNIVERSAL PAY</span>
        <span>{buildBadge}</span>
      </div>
    </div>
  );
}

interface CardProps {
  kind: 'customer' | 'merchant';
  title: string;
  eyebrow: string;
  desc: string;
  stats: { k: string; v: string }[];
  onHover: (k: PersonaType) => void;
  onClick: () => void;
}

function PersonaCard({ kind, title, eyebrow, desc, stats, onHover, onClick }: CardProps) {
  return (
    <div
      className={`glass lift persona-${kind} persona-card`}
      onMouseEnter={() => onHover(kind)}
      onMouseLeave={() => onHover(null)}
      onClick={onClick}
    >
      <div style={{ position: 'absolute', top: -80, right: -80, width: 260, height: 260, borderRadius: '50%', background: 'var(--accent-glow)', filter: 'blur(50px)', pointerEvents: 'none' }} />
      <div style={{ position: 'relative', zIndex: 1 }}>
        <div className="persona-card-top">
          <span className="pill pill-accent">{eyebrow}</span>
          <div
            style={{
              width: 48,
              height: 48,
              borderRadius: 16,
              background: 'linear-gradient(180deg, rgba(255,255,255,0.08), rgba(255,255,255,0.03))',
              border: '1px solid color-mix(in oklch, var(--accent) 28%, var(--hairline-2))',
              boxShadow: '0 12px 32px rgba(0,0,0,0.18), inset 0 1px 0 rgba(255,255,255,0.06)',
              display: 'grid',
              placeItems: 'center',
            }}
            aria-label="UniPay"
          >
            <img src="/unipay-mark-white.svg" alt="" style={{ width: 24, height: 24 }} />
          </div>
        </div>
        <div className="persona-card-title">{title}</div>
        <p className="persona-card-desc">{desc}</p>
      </div>
      <div className="persona-card-stats">
        {stats.map((s) => (
          <div key={s.k} className="persona-card-stat">
            <div className="mono tnum" style={{ fontSize: 22, fontWeight: 500 }}>{s.v}</div>
            <div className="mono" style={{ fontSize: 10, color: 'var(--text-3)', letterSpacing: '0.1em', textTransform: 'uppercase' }}>{s.k}</div>
          </div>
        ))}
        <div className="persona-card-enter">
          Enter <Icons.arrowRight />
        </div>
      </div>
    </div>
  );
}
