import { createContext, useContext, useEffect, useState, type ReactNode } from 'react';
import { useInterwovenKit } from '@initia/interwovenkit-react';
import { useAccount } from 'wagmi';
import { INITIA_EVM_CHAIN_ID_HEX, INITIA_EVM_CHAIN } from './config';

// Re-export the EIP-1193 provider type used by htlc.ts
export type EIP1193Provider = {
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>;
};

interface WalletContextValue {
  address: string | null;
  initiaAddress: string | null;
  provider: EIP1193Provider | null;
  isReady: boolean;
  isAuthenticated: boolean;
  connect: () => void;
  disconnect: () => void;
  switchToInitia: () => Promise<void>;
  switchToChain: (chainIdHex: string, chainConfig?: { chainName: string; nativeCurrency: { name: string; symbol: string; decimals: number }; rpcUrls: string[] }) => Promise<void>;
  isConnectModalOpen: boolean;
  closeConnectModal: () => void;
  openBridge: (defaultValues?: { srcChainId?: string; srcDenom?: string }) => void;
}

const WalletContext = createContext<WalletContextValue>({
  address: null,
  initiaAddress: null,
  provider: null,
  isReady: false,
  isAuthenticated: false,
  connect: () => {},
  disconnect: () => {},
  switchToInitia: async () => {},
  switchToChain: async () => {},
  isConnectModalOpen: false,
  closeConnectModal: () => {},
  openBridge: () => {},
});

/**
 * Thin wrapper around useInterwovenKit() that exposes the same interface
 * the rest of the app expects. This keeps downstream components unchanged
 * while routing all wallet operations through InterwovenKit.
 */
function WalletContextBridge({ children }: { children: ReactNode }) {
  const {
    initiaAddress,
    address,
    isConnected,
    openConnect,
    disconnect,
    openBridge,
  } = useInterwovenKit();

  // `address` is the EVM hex (0x…) address; `initiaAddress` is the bech32.
  const evmAddress = isConnected && address ? address : null;
  const cosmosAddress = isConnected && initiaAddress ? initiaAddress : null;

  // Resolve the EIP-1193 provider from the actual wagmi connector that the
  // user selected in InterwovenKit's modal. Reading `window.ethereum` directly
  // is unsafe when multiple injected wallets are installed (MetaMask + Core
  // Wallet, Phantom, Rabby, etc.) — they race for `window.ethereum`, so the
  // user can connect with MetaMask but signing then routes to whichever
  // extension last clobbered the global. The connector's own provider is
  // bound to the wallet the user actually picked.
  const { connector } = useAccount();
  const [provider, setProvider] = useState<EIP1193Provider | null>(null);

  useEffect(() => {
    if (!isConnected || !connector?.getProvider) {
      setProvider(null);
      return;
    }
    let cancelled = false;
    connector
      .getProvider()
      .then((p) => {
        if (!cancelled) setProvider((p as EIP1193Provider) ?? null);
      })
      .catch(() => {
        if (!cancelled) setProvider(null);
      });
    return () => {
      cancelled = true;
    };
  }, [isConnected, connector]);

  const ensureEvmAuthorization = async () => {
    if (!provider) throw new Error('No EVM provider available');

    const accounts = await provider.request({ method: 'eth_accounts' });
    if (Array.isArray(accounts) && accounts.some((value) => typeof value === 'string' && value.length > 0)) {
      return;
    }

    await provider.request({ method: 'eth_requestAccounts' });
  };

  const switchToChain = async (
    chainIdHex: string,
    chainConfig?: { chainName: string; nativeCurrency: { name: string; symbol: string; decimals: number }; rpcUrls: string[] },
  ) => {
    if (!provider) throw new Error('No EVM provider available');
    await ensureEvmAuthorization();
    try {
      await provider.request({
        method: 'wallet_switchEthereumChain',
        params: [{ chainId: chainIdHex }],
      });
    } catch (err: unknown) {
      const code = (err as { code?: number })?.code;
      if ((code === 4902 || code === -32603) && chainConfig) {
        await provider.request({
          method: 'wallet_addEthereumChain',
          params: [{ chainId: chainIdHex, ...chainConfig }],
        });
      } else {
        throw err;
      }
    }
  };

  const switchToInitia = () =>
    switchToChain(INITIA_EVM_CHAIN_ID_HEX, {
      chainName: INITIA_EVM_CHAIN.chainName,
      nativeCurrency: INITIA_EVM_CHAIN.nativeCurrency,
      rpcUrls: INITIA_EVM_CHAIN.rpcUrls,
    });

  return (
    <WalletContext.Provider
      value={{
        address: evmAddress,
        initiaAddress: cosmosAddress,
        provider,
        isReady: true, // InterwovenKit is always ready once mounted
        isAuthenticated: isConnected,
        connect: openConnect,
        disconnect,
        switchToInitia,
        switchToChain,
        // InterwovenKit manages its own modal — these are no-ops kept for API compat
        isConnectModalOpen: false,
        closeConnectModal: () => {},
        openBridge,
      }}
    >
      {children}
    </WalletContext.Provider>
  );
}

export function WalletProvider({ children }: { children: ReactNode }) {
  return <WalletContextBridge>{children}</WalletContextBridge>;
}

export function useWallet(): WalletContextValue {
  return useContext(WalletContext);
}
