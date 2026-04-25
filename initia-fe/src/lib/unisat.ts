export interface UnisatProvider {
  requestAccounts(): Promise<string[]>;
  getAccounts(): Promise<string[]>;
  getNetwork(): Promise<'livenet' | 'testnet'>;
  switchNetwork(network: 'livenet' | 'testnet'): Promise<void>;
  sendBitcoin(
    toAddress: string,
    satoshis: number,
    options?: { feeRate?: number },
  ): Promise<string>;
  on(
    event: 'accountsChanged' | 'networkChanged',
    handler: (data: unknown) => void,
  ): void;
  removeListener(event: string, handler: (data: unknown) => void): void;
}

declare global {
  interface Window {
    unisat?: UnisatProvider;
  }
}

export function getUnisat(): UnisatProvider | null {
  return typeof window !== 'undefined' && window.unisat ? window.unisat : null;
}

export async function sendBitcoinViaUnisat(
  to: string,
  satoshis: number,
): Promise<string> {
  const unisat = getUnisat();
  if (!unisat) throw new Error('UniSat wallet not installed');

  const network = await unisat.getNetwork();
  if (network !== 'testnet') {
    await unisat.switchNetwork('testnet');
  }

  try {
    return await unisat.sendBitcoin(to, satoshis);
  } catch (e: unknown) {
    const msg =
      e instanceof Error ? e.message : typeof e === 'string' ? e : '';
    if (/reject|cancel|denied/i.test(msg)) {
      throw new Error('Transaction rejected in UniSat wallet');
    }
    if (/insufficient|balance/i.test(msg)) {
      throw new Error('Insufficient BTC balance');
    }
    throw new Error(msg || 'UniSat send failed');
  }
}
