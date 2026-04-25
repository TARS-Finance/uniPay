import { useMemo } from 'react';

interface Props { seed?: string; size?: number; }

export function QRCode({ seed = 'initia', size = 176 }: Props) {
  const cells = 25;
  const px = size / cells;

  const grid = useMemo(() => {
    let h = 2166136261;
    for (let i = 0; i < seed.length; i++) { h ^= seed.charCodeAt(i); h = Math.imul(h, 16777619); }
    const rand = () => { h ^= h << 13; h ^= h >>> 17; h ^= h << 5; return ((h >>> 0) % 1000) / 1000; };
    const m = Array.from({ length: cells }, () => Array.from({ length: cells }, () => rand() > 0.52));
    const mark = (r: number, c: number) => {
      for (let i = 0; i < 7; i++) for (let j = 0; j < 7; j++) {
        const on = (i === 0 || i === 6 || j === 0 || j === 6 || (i >= 2 && i <= 4 && j >= 2 && j <= 4));
        if (r + i < cells && c + j < cells) m[r + i][c + j] = on;
      }
    };
    mark(0, 0); mark(0, cells - 7); mark(cells - 7, 0);
    return m;
  }, [seed]);

  return (
    <div style={{ width: size, height: size, background: '#fff', padding: 10, borderRadius: 10, display: 'grid', placeItems: 'center' }}>
      <svg width={size - 20} height={size - 20} viewBox={`0 0 ${cells * px} ${cells * px}`}>
        {grid.flatMap((row, r) =>
          row.map((on, c) =>
            on ? <rect key={`${r}-${c}`} x={c * px} y={r * px} width={px} height={px} fill="#05070e" /> : null
          )
        )}
      </svg>
    </div>
  );
}
