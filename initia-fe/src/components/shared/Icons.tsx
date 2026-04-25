import type { CSSProperties } from 'react';
import {
  IconArrowRight,
  IconArrowsLeftRight,
  IconBarChartInitia,
  IconCheck,
  IconChevronDown,
  IconClose,
  IconCopy,
  IconExternalLink,
  IconHistory,
  IconPay,
  IconPlus,
  IconPoolSparkles,
  IconQrCode,
  IconRefresh,
  IconSignOut,
  IconStarFilled,
  IconSwapVert,
  IconWallet,
} from '@initia/icons-react';

type P = {
  className?: string;
  color?: string;
  style?: CSSProperties;
};

function iconProps(p: P) {
  return {
    className: p.className,
    color: p.color ?? 'currentColor',
    style: p.style,
  };
}

export const Icons = {
  arrow:      (p: P) => <IconArrowRight size={16} {...iconProps(p)} />,
  arrowRight: (p: P) => <IconArrowRight size={18} {...iconProps(p)} />,
  bridge:     (p: P) => <IconArrowsLeftRight size={16} {...iconProps(p)} />,
  check:      (p: P) => <IconCheck size={14} {...iconProps(p)} />,
  x:          (p: P) => <IconClose size={14} {...iconProps(p)} />,
  wallet:     (p: P) => <IconWallet size={16} {...iconProps(p)} />,
  history:    (p: P) => <IconHistory size={16} {...iconProps(p)} />,
  ext:        (p: P) => <IconExternalLink size={12} {...iconProps(p)} />,
  chevron:    (p: P) => <IconChevronDown size={12} {...iconProps(p)} />,
  copy:       (p: P) => <IconCopy size={12} {...iconProps(p)} />,
  spinner:    (p: P) => (
    <svg width="14" height="14" viewBox="0 0 14 14" fill="none" style={{ animation: 'spin 900ms linear infinite', ...p.style }} className={p.className}>
      <circle cx="7" cy="7" r="5" stroke={p.color ?? 'currentColor'} strokeWidth="1.5" strokeOpacity="0.2" />
      <path d="M12 7a5 5 0 00-5-5" stroke={p.color ?? 'currentColor'} strokeWidth="1.8" strokeLinecap="round" />
    </svg>
  ),
  timer:      (p: P) => (
    <svg width="14" height="14" viewBox="0 0 16 16" fill="none" className={p.className} style={p.style}>
      <circle cx="8" cy="9" r="5.25" stroke={p.color ?? 'currentColor'} strokeWidth="1.4" strokeOpacity="0.25" />
      <path
        d="M8 3.75A5.25 5.25 0 0113.25 9"
        stroke={p.color ?? 'currentColor'}
        strokeWidth="1.6"
        strokeLinecap="round"
        style={{ transformOrigin: '8px 9px', animation: 'spin 1.2s linear infinite' }}
      />
      <path d="M8 6v3.2l2 1.2" stroke={p.color ?? 'currentColor'} strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
      <path d="M6.5 1.75h3" stroke={p.color ?? 'currentColor'} strokeWidth="1.4" strokeLinecap="round" />
      <path d="M8 1.75v2" stroke={p.color ?? 'currentColor'} strokeWidth="1.4" strokeLinecap="round" />
    </svg>
  ),
  logo:       (p: P) => (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" className={p.className} style={p.style}>
      <path
        d="M5 4h4v9a3 3 0 006 0V4h4v9a7 7 0 01-14 0V4z"
        fill={p.color ?? 'currentColor'}
      />
    </svg>
  ),
  qr:         (p: P) => <IconQrCode size={14} {...iconProps(p)} />,
  coin:       (p: P) => <IconPay size={16} {...iconProps(p)} />,
  leaf:       (p: P) => <IconPoolSparkles size={16} {...iconProps(p)} />,
  flip:       (p: P) => <IconSwapVert size={18} {...iconProps(p)} />,
  bars:       (p: P) => <IconBarChartInitia size={16} {...iconProps(p)} />,
  plus:       (p: P) => <IconPlus size={12} {...iconProps(p)} />,
  sparkle:    (p: P) => <IconStarFilled size={14} {...iconProps(p)} />,
  power:      (p: P) => <IconSignOut size={14} {...iconProps(p)} />,
  refresh:    (p: P) => <IconRefresh size={14} {...iconProps(p)} />,
};
