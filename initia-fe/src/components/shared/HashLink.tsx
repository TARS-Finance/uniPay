import { useState } from 'react';
import { Icons } from './Icons';

interface Props { hash: string; label: string; }

export function HashLink({ hash, label }: Props) {
  if (!hash) return null;

  const [copied, setCopied] = useState(false);

  const onCopy = (e: React.MouseEvent) => {
    e.stopPropagation();
    try { navigator.clipboard.writeText(hash); } catch { /* ignore */ }
    setCopied(true);
    setTimeout(() => setCopied(false), 1200);
  };

  return (
    <a
      href="#"
      onClick={(e) => e.preventDefault()}
      style={{ display: 'inline-flex', alignItems: 'center', gap: 6, color: 'var(--text-1)', textDecoration: 'none', fontFamily: 'var(--font-mono)', fontSize: 12 }}
    >
      <span style={{ color: 'var(--text-2)' }}>{label}</span>
      <span>{hash.slice(0, 6)}…{hash.slice(-4)}</span>
      <button onClick={onCopy} title="Copy" style={{ background: 'transparent', border: 'none', color: copied ? 'var(--success)' : 'var(--text-3)', cursor: 'pointer', padding: 2 }}>
        {copied ? <Icons.check /> : <Icons.copy />}
      </button>
      <span style={{ color: 'var(--text-3)' }}><Icons.ext /></span>
    </a>
  );
}
