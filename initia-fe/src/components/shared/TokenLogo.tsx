import React, { useEffect, useState } from 'react';
import { TOKENS, CHAINS } from '../../data';
import { getChainIcon, getTokenIcon, subscribeIcons } from '../../hooks/useStrategies';

type Size = 'sm' | 'md' | 'lg';
interface TokenLogoProps { sym: string; size?: Size; chain?: string; }
interface ChainLogoProps { id: string; size?: Size; }

const PX: Record<Size, number> = { sm: 22, md: 28, lg: 40 };
const BADGE_PX: Record<Size, number> = { sm: 11, md: 14, lg: 18 };

function useIconVersion() {
  const [, setV] = useState(0);
  useEffect(() => subscribeIcons(() => setV((n) => n + 1)), []);
}

function IconImg({ src, alt, size, rounded }: { src: string; alt: string; size: Size; rounded: boolean }) {
  const px = PX[size];
  return (
    <img
      src={src}
      alt={alt}
      width={px}
      height={px}
      style={{
        width: px,
        height: px,
        borderRadius: rounded ? '50%' : 7,
        objectFit: 'cover',
        background: 'rgba(255,255,255,0.05)',
        flexShrink: 0,
      }}
    />
  );
}

export function TokenLogo({ sym, size = 'md', chain }: TokenLogoProps) {
  useIconVersion();
  const tokenIcon = getTokenIcon(sym);
  const chainIcon = chain ? getChainIcon(chain) : undefined;

  let tokenNode: React.ReactNode;
  if (tokenIcon) {
    tokenNode = <IconImg src={tokenIcon} alt={sym} size={size} rounded />;
  } else {
    const t = TOKENS[sym];
    if (!t) tokenNode = null;
    else {
      const cls = `logo-tile ${size === 'sm' ? 'sm' : size === 'lg' ? 'lg' : ''} round ${t.klass}`;
      tokenNode = <div className={cls}>{sym.slice(0, 3)}</div>;
    }
  }

  // Show chain badge only when we have *both* a token logo and a distinct chain icon.
  if (tokenNode && chainIcon && chainIcon !== tokenIcon) {
    const badgePx = BADGE_PX[size];
    return (
      <span style={{ position: 'relative', display: 'inline-flex', flexShrink: 0 }}>
        {tokenNode}
        <img
          src={chainIcon}
          alt={chain}
          width={badgePx}
          height={badgePx}
          style={{
            position: 'absolute',
            right: -2,
            bottom: -2,
            width: badgePx,
            height: badgePx,
            borderRadius: '50%',
            objectFit: 'cover',
            background: 'var(--bg-1, #0b0b0d)',
            boxShadow: '0 0 0 1.5px var(--bg-1, #0b0b0d)',
          }}
        />
      </span>
    );
  }

  return <>{tokenNode}</>;
}

export function ChainLogo({ id, size = 'sm' }: ChainLogoProps) {
  useIconVersion();
  const icon = getChainIcon(id);
  if (icon) return <IconImg src={icon} alt={id} size={size} rounded={false} />;
  const c = CHAINS[id];
  if (!c) return null;
  const cls = `logo-tile ${size === 'sm' ? 'sm' : size === 'lg' ? 'lg' : ''} ${c.klass}`;
  return <div className={cls}>{c.short}</div>;
}
