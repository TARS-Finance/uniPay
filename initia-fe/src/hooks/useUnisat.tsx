import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useState,
} from 'react';
import { getUnisat, sendBitcoinViaUnisat } from '../lib/unisat';

interface UnisatContextValue {
  unisatAddress: string | null;
  connecting: boolean;
  error: string | null;
  isInstalled: boolean;
  connectUnisat(): Promise<void>;
  disconnectUnisat(): void;
  sendBitcoin(to: string, satoshis: number): Promise<string>;
}

const UnisatContext = createContext<UnisatContextValue | null>(null);

const STORAGE_KEY = 'unisat.address';

export function UnisatProvider({ children }: { children: React.ReactNode }) {
  const [unisatAddress, setUnisatAddress] = useState<string | null>(() => {
    try {
      return localStorage.getItem(STORAGE_KEY) ?? null;
    } catch (_e) {
      return null;
    }
  });
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isInstalled, setIsInstalled] = useState(false);

  useEffect(() => {
    const installed = getUnisat() !== null;
    setIsInstalled(installed);
    if (!installed) {
      setUnisatAddress(null);
      try { localStorage.removeItem(STORAGE_KEY); } catch (_e) { /* ignore */ }
    }
  }, []);

  // Sync address from wallet events
  useEffect(() => {
    const unisat = getUnisat();
    if (!unisat) return;

    const handleAccountsChanged = (accounts: unknown) => {
      const arr = Array.isArray(accounts) ? (accounts as string[]) : [];
      const next = arr[0] ?? null;
      setUnisatAddress(next);
      try {
        if (next) {
          localStorage.setItem(STORAGE_KEY, next);
        } else {
          localStorage.removeItem(STORAGE_KEY);
        }
      } catch (_e) { /* ignore */ }
    };

    unisat.on('accountsChanged', handleAccountsChanged);
    return () => unisat.removeListener('accountsChanged', handleAccountsChanged);
  }, []);

  const connectUnisat = useCallback(async () => {
    const unisat = getUnisat();
    if (!unisat) {
      setError('UniSat wallet is not installed');
      return;
    }
    setConnecting(true);
    setError(null);
    try {
      const accounts = await unisat.requestAccounts();
      const addr = accounts[0] ?? null;
      setUnisatAddress(addr);
      try {
        if (addr) localStorage.setItem(STORAGE_KEY, addr);
      } catch (_e) { /* ignore */ }
    } catch (e: unknown) {
      const msg = e instanceof Error ? e.message : String(e);
      setError(msg || 'Failed to connect UniSat');
    } finally {
      setConnecting(false);
    }
  }, []);

  const disconnectUnisat = useCallback(() => {
    setUnisatAddress(null);
    setError(null);
    try {
      localStorage.removeItem(STORAGE_KEY);
    } catch (_e) { /* ignore */ }
  }, []);

  const sendBitcoin = useCallback(
    async (to: string, satoshis: number): Promise<string> => {
      setError(null);
      try {
        return await sendBitcoinViaUnisat(to, satoshis);
      } catch (e: unknown) {
        const msg = e instanceof Error ? e.message : String(e);
        setError(msg);
        throw e;
      }
    },
    [],
  );

  return (
    <UnisatContext.Provider
      value={{
        unisatAddress,
        connecting,
        error,
        isInstalled,
        connectUnisat,
        disconnectUnisat,
        sendBitcoin,
      }}
    >
      {children}
    </UnisatContext.Provider>
  );
}

export function useUnisat(): UnisatContextValue {
  const ctx = useContext(UnisatContext);
  if (!ctx) throw new Error('useUnisat must be used inside <UnisatProvider>');
  return ctx;
}
