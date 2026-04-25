import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import type { GeneratedType } from '@cosmjs/proto-signing';
import { WagmiProvider, createConfig, http } from 'wagmi';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MsgInitiateTokenWithdrawal } from '@initia/opinit.proto/opinit/opchild/v1/tx';
import { MsgExecute } from '@initia/initia.proto/initia/move/v1/tx';
import { InterwovenKitProvider, TESTNET, injectStyles } from '@initia/interwovenkit-react';
import interwovenKitStyles from '@initia/interwovenkit-react/styles.js';
import '@initia/interwovenkit-react/styles.css';
import { WalletProvider } from './lib/wallet-context';
import { UnisatProvider } from './hooks/useUnisat';
import { customChain, initiaEvmChain } from './lib/config';
import App from './App';

injectStyles(interwovenKitStyles);

const protoTypes: Array<[string, GeneratedType]> = [
  ['/opinit.opchild.v1.MsgInitiateTokenWithdrawal', MsgInitiateTokenWithdrawal as unknown as GeneratedType],
  ['/initia.move.v1.MsgExecute', MsgExecute as unknown as GeneratedType],
];

const interwovenKitConfig = {
  ...TESTNET,
  defaultChainId: customChain.chain_id,
  customChain,
  customChains: [customChain],
  protoTypes,
};

const wagmiConfig = createConfig({
  chains: [initiaEvmChain],
  transports: { [initiaEvmChain.id]: http() },
});

const queryClient = new QueryClient();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <WagmiProvider config={wagmiConfig}>
      <QueryClientProvider client={queryClient}>
        <InterwovenKitProvider
          {...interwovenKitConfig}
        >
          <WalletProvider>
            <UnisatProvider>
              <App />
            </UnisatProvider>
          </WalletProvider>
        </InterwovenKitProvider>
      </QueryClientProvider>
    </WagmiProvider>
  </StrictMode>
);
