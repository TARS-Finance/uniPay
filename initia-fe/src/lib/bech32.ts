const CHARSET = 'qpzry9x8gf2tvdw0s3jn54khce6mua7l';
const CHAR_MAP: Record<string, number> = {};
for (let i = 0; i < CHARSET.length; i++) CHAR_MAP[CHARSET[i]] = i;

function convertBits(data: number[], fromBits: number, toBits: number, pad: boolean): number[] | null {
  let acc = 0, bits = 0;
  const result: number[] = [];
  const maxv = (1 << toBits) - 1;
  for (const value of data) {
    if (value < 0 || value >> fromBits !== 0) return null;
    acc = (acc << fromBits) | value;
    bits += fromBits;
    while (bits >= toBits) {
      bits -= toBits;
      result.push((acc >> bits) & maxv);
    }
  }
  if (pad) {
    if (bits > 0) result.push((acc << (toBits - bits)) & maxv);
  } else if (bits >= fromBits || ((acc << (toBits - bits)) & maxv)) {
    return null;
  }
  return result;
}

// Decode a bech32 string, returns { prefix, words }
function decodeBech32(str: string): { prefix: string; words: number[] } | null {
  const s = str.toLowerCase();
  const pos = s.lastIndexOf('1');
  if (pos < 1 || pos + 7 > s.length) return null;
  const prefix = s.slice(0, pos);
  const words: number[] = [];
  for (let i = pos + 1; i < s.length - 6; i++) {
    const d = CHAR_MAP[s[i]];
    if (d === undefined) return null;
    words.push(d);
  }
  return { prefix, words };
}

/**
 * Convert a bech32 cosmos address (init1...) to a 0x EVM hex address.
 * Returns null if the input is not a valid bech32 address.
 */
export function bech32ToHex(addr: string): string | null {
  try {
    const decoded = decodeBech32(addr);
    if (!decoded) return null;
    const bytes = convertBits(decoded.words, 5, 8, false);
    if (!bytes || bytes.length !== 20) return null;
    return '0x' + bytes.map(b => b.toString(16).padStart(2, '0')).join('');
  } catch {
    return null;
  }
}

/** If addr is init1... bech32, convert to 0x hex. Otherwise return as-is. */
export function normalizeToEvmAddress(addr: string): string {
  const trimmed = addr.trim();
  if (trimmed.startsWith('0x') || trimmed.startsWith('0X')) return trimmed;
  const converted = bech32ToHex(trimmed);
  return converted ?? trimmed;
}
