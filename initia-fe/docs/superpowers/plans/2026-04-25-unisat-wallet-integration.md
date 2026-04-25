# UniSat Wallet Integration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add UniSat Bitcoin wallet support so users can connect their Bitcoin wallet in the sidebar, have their refund address auto-filled, and send BTC directly via a wallet popup instead of a manual QR-code deposit.

**Architecture:** A new `UnisatContext` (mirroring the Phantom/Solana pattern) provides `unisatAddress`, `connectUnisat`, `disconnectUnisat`, and `sendBitcoin` globally. `useBitcoinSwap` gains optional `sendBitcoin`/`unisatAddress` params to trigger a wallet send after order creation. The Sidebar renders a persistent Bitcoin wallet section. The QR flow is preserved as a fallback when UniSat is absent.

**Tech Stack:** React context, TypeScript, `window.unisat` injected wallet API, localStorage for persistence.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `src/lib/unisat.ts` | **Create** | Raw `window.unisat` types + `getUnisat()` + `sendBitcoinViaUnisat()` |
| `src/hooks/useUnisat.ts` | **Create** | `UnisatContext`, `UnisatProvider`, `useUnisat()` hook |
| `src/main.tsx` | **Modify** | Wrap app in `<UnisatProvider>` |
| `src/hooks/useBitcoinSwap.ts` | **Modify** | Add `'sending'` step, accept `sendBitcoin`/`unisatAddress` params |
| `src/components/shell/Sidebar.tsx` | **Modify** | Bitcoin wallet section (connect/disconnect/address display) |
| `src/components/customer/CustomerView.tsx` | **Modify** | Auto-fill refund address, pass `sendBitcoin` to hook, conditional QR |
| `src/components/customer/PayCard.tsx` | **Modify** | Accept `btcWalletConnected` prop, hide refund row + adjust `ctaLabel` |

---

## Task 1: UniSat lib — raw API wrapper

**Files:**
- Create: `src/lib/unisat.ts`

- [ ] **Step 1: Create `src/lib/unisat.ts`**

```ts
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
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
cd /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors in `src/lib/unisat.ts`.

- [ ] **Step 3: Commit**

```bash
git add src/lib/unisat.ts
git commit -m "feat: add UniSat wallet lib — getUnisat, sendBitcoinViaUnisat"
```

---

## Task 2: UniSat context + hook

**Files:**
- Create: `src/hooks/useUnisat.ts`

- [ ] **Step 1: Create `src/hooks/useUnisat.ts`**

```ts
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
    } catch {
      return null;
    }
  });
  const [connecting, setConnecting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [isInstalled, setIsInstalled] = useState(false);

  useEffect(() => {
    setIsInstalled(getUnisat() !== null);
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
      } catch { /* ignore */ }
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
      } catch { /* ignore */ }
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
    } catch { /* ignore */ }
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
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/hooks/useUnisat.ts
git commit -m "feat: add UnisatContext, UnisatProvider, useUnisat hook"
```

---

## Task 3: Wire UnisatProvider into app root

**Files:**
- Modify: `src/main.tsx`

- [ ] **Step 1: Add `UnisatProvider` to `src/main.tsx`**

Replace the render block:

```tsx
import { UnisatProvider } from './hooks/useUnisat';

// inside the render:
createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <WagmiProvider config={wagmiConfig}>
      <QueryClientProvider client={queryClient}>
        <InterwovenKitProvider
          {...TESTNET}
          defaultChainId="tars-1"
          customChain={customChain}
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
```

- [ ] **Step 2: Verify TypeScript compiles and dev server starts**

```bash
npx tsc --noEmit 2>&1 | head -20
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/main.tsx
git commit -m "feat: wrap app in UnisatProvider"
```

---

## Task 4: Bitcoin wallet section in Sidebar

**Files:**
- Modify: `src/components/shell/Sidebar.tsx`

- [ ] **Step 1: Add UniSat section to Sidebar**

Add the import at the top:

```tsx
import { useUnisat } from '../../hooks/useUnisat';
```

Add inside the `Sidebar` function body (after existing `const { connect, disconnect } = useWallet();`):

```tsx
const {
  unisatAddress,
  connecting: unisatConnecting,
  isInstalled: unisatInstalled,
  connectUnisat,
  disconnectUnisat,
} = useUnisat();
const [unisatMenuOpen, setUnisatMenuOpen] = useState(false);
```

Add the Bitcoin wallet section inside `<div className="sidebar-footer">`, just above the EVM wallet block (before the `{wallet ? (` ternary):

```tsx
{/* Bitcoin wallet */}
{!collapsed && (
  <div className="sidebar-wallet-card" style={{ position: 'relative' }}>
    <div style={{ width: 8, height: 8, borderRadius: 999, background: unisatAddress ? '#f7931a' : 'var(--text-3)', flexShrink: 0 }} />
    <div className="sidebar-brand-text" style={{ flex: 1, minWidth: 0 }}>
      <div style={{ fontSize: 10, color: 'var(--text-3)', letterSpacing: '0.05em', textTransform: 'uppercase', marginBottom: 2 }}>
        Bitcoin
      </div>
      {unisatAddress ? (
        <div className="mono" style={{ fontSize: 12, color: 'var(--text-0)', overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}>
          {unisatAddress.slice(0, 6)}…{unisatAddress.slice(-4)}
        </div>
      ) : (
        <div style={{ fontSize: 12, color: 'var(--text-3)' }}>
          {!unisatInstalled ? 'UniSat not detected' : 'Not connected'}
        </div>
      )}
    </div>
    {unisatAddress ? (
      <>
        <button
          className="sidebar-icon-action sidebar-icon-danger"
          onClick={() => { disconnectUnisat(); setUnisatMenuOpen(false); }}
          title="Disconnect UniSat"
        >
          <Icons.power />
        </button>
      </>
    ) : unisatInstalled ? (
      <button
        className="btn"
        onClick={connectUnisat}
        disabled={unisatConnecting}
        style={{ fontSize: 11, padding: '4px 10px', minWidth: 0 }}
      >
        {unisatConnecting ? '…' : 'Connect'}
      </button>
    ) : null}
  </div>
)}
{collapsed && unisatInstalled && (
  <button
    className="sidebar-icon-action"
    onClick={unisatAddress ? disconnectUnisat : connectUnisat}
    title={unisatAddress ? 'Disconnect UniSat' : 'Connect UniSat'}
    style={{ color: unisatAddress ? '#f7931a' : undefined }}
  >
    <Icons.wallet />
  </button>
)}
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/components/shell/Sidebar.tsx
git commit -m "feat: add Bitcoin wallet section to Sidebar using useUnisat"
```

---

## Task 5: Add `'sending'` step to useBitcoinSwap + UniSat send

**Files:**
- Modify: `src/hooks/useBitcoinSwap.ts`

- [ ] **Step 1: Add `'sending'` to the step union and update params**

Change the `BitcoinSwapStep` type (line 34):

```ts
export type BitcoinSwapStep =
  | 'idle'
  | 'loading'
  | 'creating'
  | 'sending'           // UniSat popup open — waiting for user to confirm send
  | 'awaiting'          // order created, no on-chain progress yet
  | 'user_initiated'    // user's BTC deposit detected (source initiate tx)
  | 'cobi_initiated'    // executor locked destination asset
  | 'user_redeemed'     // user received destination funds (destination redeem)
  | 'fulfilled'         // executor claimed source BTC with secret — fully done
  | 'refunded'          // either leg was refunded
  | 'done'              // alias kept for legacy callers — equivalent to fulfilled
  | 'error';
```

Change `BitcoinSwapActions` interface to accept optional UniSat helpers:

```ts
export interface BitcoinSwapActions {
  startBitcoinSwap: (params: {
    quoteMode: QuoteMode;
    sourceAmountRaw: string;
    destinationAmountRaw: string;
    destinationAsset: string;
    receiverAddress: string;
    sourceAsset?: string;
    btcRefundAddress?: string;
    strategyId?: string;
    // UniSat integration — if provided, wallet sends BTC instead of QR
    unisatSendBitcoin?: (to: string, satoshis: number) => Promise<string>;
  }) => Promise<void>;
  reset: () => void;
}
```

- [ ] **Step 2: Update `startBitcoinSwap` to conditionally use UniSat**

In the `startBitcoinSwap` callback, replace the block starting at `setOrderId(oid);` (after the order is created):

```ts
const startBitcoinSwap = useCallback(async ({
  quoteMode,
  sourceAmountRaw,
  destinationAmountRaw,
  destinationAsset,
  receiverAddress,
  sourceAsset = 'bitcoin_testnet:btc',
  btcRefundAddress,
  strategyId,
  unisatSendBitcoin,
}: {
  quoteMode: QuoteMode;
  sourceAmountRaw: string;
  destinationAmountRaw: string;
  destinationAsset: string;
  receiverAddress: string;
  sourceAsset?: string;
  btcRefundAddress?: string;
  strategyId?: string;
  unisatSendBitcoin?: (to: string, satoshis: number) => Promise<string>;
}) => {
  abortRef.current = false;
  setError(null);
  setStep('creating');

  try {
    let refundXOnlyPubkey: string;
    try {
      refundXOnlyPubkey = taprootAddressToXOnlyPubkey(btcRefundAddress ?? '');
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Invalid Bitcoin refund address');
      setStep('error');
      return;
    }

    const secretBytes = generateSecret();
    const secretHex = toHex(secretBytes);
    const secretHashHex = await sha256Hex(secretBytes);
    const secretHash = secretHashHex.replace(/^0x/, '');

    localStorage.setItem(`btc.secret.${secretHash}`, secretHex);

    const matched = await createOrder({
      from: sourceAsset,
      to: destinationAsset,
      from_amount: quoteMode === 'exact-in' ? sourceAmountRaw : undefined,
      to_amount: quoteMode === 'exact-out' ? destinationAmountRaw : undefined,
      initiator_source_address: refundXOnlyPubkey,
      initiator_destination_address: normalizeToEvmAddress(receiverAddress),
      secret_hash: secretHash,
      strategy_id: strategyId,
    });

    const oid: string = matched.create_order?.create_id;
    const addr: string = matched.source_swap?.swap_id;
    const createdSourceAmount = matched.source_swap?.amount || sourceAmountRaw;

    if (!oid) throw new Error('No order id returned');
    if (!addr) throw new Error('No deposit address returned from order');

    localStorage.setItem(STORAGE_KEY, JSON.stringify({
      oid,
      addr,
      secretHash,
      sourceAmountSats: createdSourceAmount,
    }));

    setOrderId(oid);
    setDepositAddress(addr);
    setSourceAmountSats(createdSourceAmount);

    // If UniSat is connected, send BTC directly from the wallet
    if (unisatSendBitcoin) {
      setStep('sending');
      try {
        await unisatSendBitcoin(addr, Number(createdSourceAmount));
      } catch (e: unknown) {
        if (abortRef.current) return;
        setError(e instanceof Error ? e.message : 'UniSat send failed');
        setStep('error');
        localStorage.removeItem(STORAGE_KEY);
        return;
      }
    }

    setStep('awaiting');
    startPolling(oid, secretHash);
  } catch (e: unknown) {
    if (abortRef.current) return;
    setError(e instanceof Error ? e.message : String(e));
    setStep('error');
  }
}, [startPolling]);
```

- [ ] **Step 3: Verify TypeScript compiles**

```bash
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add src/hooks/useBitcoinSwap.ts
git commit -m "feat: add 'sending' step to useBitcoinSwap, accept unisatSendBitcoin param"
```

---

## Task 6: Update CustomerView — auto-fill refund address, pass UniSat to hook

**Files:**
- Modify: `src/components/customer/CustomerView.tsx`

- [ ] **Step 1: Import `useUnisat` and wire auto-fill effect**

Add the import near the top of `CustomerView.tsx`:

```tsx
import { useUnisat } from '../../hooks/useUnisat';
```

Inside `CustomerView`, after the existing `useSwap`/`useBitcoinSwap`/`useSolanaSwap` declarations, add:

```tsx
const { unisatAddress, sendBitcoin: unisatSendBitcoin } = useUnisat();

// Auto-fill BTC refund address from UniSat when Bitcoin source is selected
useEffect(() => {
  if (srcChain === 'bitcoin_testnet' && unisatAddress) {
    setBtcRefundAddress(unisatAddress);
  } else if (srcChain !== 'bitcoin_testnet') {
    // Clear the auto-filled value when switching away from Bitcoin
    setBtcRefundAddress((prev) => (prev === unisatAddress ? '' : prev));
  }
}, [srcChain, unisatAddress]);
```

- [ ] **Step 2: Pass `unisatSendBitcoin` into `startBitcoinSwap` call**

In the `onPay` function, find the `btcSwap.startBitcoinSwap({...})` call and add the new param:

```tsx
btcSwap.startBitcoinSwap({
  quoteMode: payment.quoteMode,
  sourceAmountRaw: payment.sourceAmountRaw,
  destinationAmountRaw: payment.destinationAmountRaw,
  destinationAsset: payment.destinationAsset,
  receiverAddress,
  sourceAsset: payment.sourceAsset,
  btcRefundAddress,
  strategyId: payment.strategyId,
  unisatSendBitcoin: unisatAddress ? unisatSendBitcoin : undefined,
});
```

- [ ] **Step 3: Add `'sending'` step to `BitcoinDepositCard` `stepLabels`**

In `CustomerView.tsx`, find the `stepLabels` object inside `BitcoinDepositCard` and add the entry:

```tsx
const stepLabels: Record<BitcoinStep, string> = {
  idle: 'Idle',
  loading: 'Loading…',
  creating: 'Creating order',
  sending: 'Sending BTC',    // <-- add this
  awaiting: 'Awaiting deposit',
  user_initiated: 'BTC deposit detected',
  cobi_initiated: 'Executor locked funds',
  user_redeemed: 'Funds delivered',
  fulfilled: 'Complete',
  done: 'Complete',
  refunded: 'Refunded',
  error: 'Failed',
};
```

Also update `displayStep` mapping so `'sending'` maps to `'awaiting'` for the step indicator display (it sits in the same visual slot until on-chain confirmation):

```tsx
const displayStep: BitcoinStep =
  step === 'loading' || step === 'creating' || step === 'sending'
    ? 'awaiting'
    : step === 'done'
    ? 'fulfilled'
    : step;
```

Add a `'sending'` status message block alongside the other status messages inside `BitcoinDepositCard`:

```tsx
{step === 'sending' && (
  <div style={{ marginTop: 14, padding: '10px 14px', background: 'rgba(247,147,26,0.08)', border: '1px solid rgba(247,147,26,0.2)', borderRadius: 8, fontSize: 12, color: '#f7931a', display: 'flex', alignItems: 'center', gap: 8 }}>
    <span style={{ animation: 'spin 1.5s linear infinite', display: 'inline-block' }}>⏳</span>
    Confirm the transaction in your UniSat wallet…
  </div>
)}
```

- [ ] **Step 4: Pass `btcWalletConnected` to `PayCard`**

In `CustomerView.tsx`, find the `<PayCard ... />` JSX and add the prop:

```tsx
<PayCard
  {/* ...existing props... */}
  btcWalletConnected={isBitcoin && !!unisatAddress}
/>
```

- [ ] **Step 5: Verify TypeScript compiles**

```bash
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add src/components/customer/CustomerView.tsx
git commit -m "feat: auto-fill BTC refund address from UniSat, pass wallet send to useBitcoinSwap"
```

---

## Task 7: Update PayCard — hide refund row when UniSat connected

**Files:**
- Modify: `src/components/customer/PayCard.tsx`

- [ ] **Step 1: Add `btcWalletConnected` prop**

Add to the `Props` interface:

```ts
btcWalletConnected?: boolean;
```

Add to the destructured params in `PayCard`:

```ts
btcWalletConnected = false,
```

- [ ] **Step 2: Hide BTC refund address detail row when wallet is connected**

Find the `{isBitcoin && ( <DetailRow label="BTC refund address" ... /> )}` block and wrap it:

```tsx
{isBitcoin && !btcWalletConnected && (
  <DetailRow
    label="BTC refund address"
    value={
      btcRefundAddress ? (
        <span style={{ color: 'var(--text-0)' }}>
          {btcRefundAddress.slice(0, 6)}…{btcRefundAddress.slice(-4)}
        </span>
      ) : (
        <button
          onClick={() => setAddrOpen((v) => !v)}
          style={{ color: 'var(--accent)', background: 'none', border: 'none', padding: 0, cursor: 'pointer', font: 'inherit' }}
        >
          Set address →
        </button>
      )
    }
  />
)}
```

- [ ] **Step 3: Remove refund address requirement from `ctaReady` when wallet connected**

Update `ctaReady`:

```ts
const ctaReady =
  !payDisabled &&
  !!receiverAddress &&
  (!isBitcoin || btcWalletConnected || !!btcRefundAddress.trim()) &&
  (isExactOut
    ? !!requestedDestinationAmount
    : !!amount && parseFloat(amount) > 0) &&
  quoteReady;
```

- [ ] **Step 4: Remove "Enter BTC refund address" label when wallet connected**

Update `ctaLabel`:

```ts
const ctaLabel =
  payDisabled && !wallet
    ? 'Loading chain data…'
    : !receiverAddress
    ? 'Enter merchant address'
    : isBitcoin && !btcWalletConnected && !btcRefundAddress.trim()
    ? 'Enter BTC refund address'
    : !isExactOut && (!amount || parseFloat(amount) <= 0)
    ? 'Enter amount'
    : isExactOut && !requestedDestinationAmount
    ? 'Invalid invoice amount'
    : quote.loading
    ? 'Fetching route…'
    : wallet || isNonEvm
    ? `Pay ${fmtUSD(liveSourceUsdValue)}`
    : `Connect wallet & pay ${fmtUSD(liveSourceUsdValue)}`;
```

- [ ] **Step 5: Update CTA click handler — skip addr panel when wallet connected**

```tsx
onClick={() => {
  if (!receiverAddress || (isBitcoin && !btcWalletConnected && !btcRefundAddress.trim())) {
    setAddrOpen(true);
    return;
  }
  triggerPay();
}}
```

- [ ] **Step 6: Hide refund field inside addr panel when wallet connected**

In the `addrOpen` block, wrap the BTC refund `Field` with a condition:

```tsx
{isBitcoin && !btcWalletConnected && (
  <Field
    label="Your Bitcoin refund address"
    required
    value={btcRefundAddress}
    onChange={(v) => {
      setBtcRefundAddress(v.trim());
      setBtcRefundError(false);
    }}
    placeholder="tb1p… Taproot address"
    error={btcRefundError}
  />
)}
```

- [ ] **Step 7: Verify TypeScript compiles**

```bash
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 8: Commit**

```bash
git add src/components/customer/PayCard.tsx
git commit -m "feat: hide BTC refund address row in PayCard when UniSat wallet connected"
```

---

## Task 8: Reset effect — handle `'sending'` step

**Files:**
- Modify: `src/components/customer/CustomerView.tsx`

- [ ] **Step 1: Add `'sending'` to the reset guard in CustomerView**

Find the reset effect that fires on page change (around line 198):

```tsx
useEffect(() => {
  if (page !== 'pay' || activePayment) return;
  if (swap.step !== 'idle' && swap.step !== 'error') swap.reset();
  if (
    btcSwap.step !== 'idle' &&
    btcSwap.step !== 'error' &&
    btcSwap.step !== 'fulfilled' &&
    btcSwap.step !== 'done' &&
    btcSwap.step !== 'sending'   // don't reset mid-wallet-popup
  ) btcSwap.reset();
  if (solSwap.step !== 'idle' && solSwap.step !== 'error' && solSwap.step !== 'fulfilled') solSwap.reset();
// eslint-disable-next-line react-hooks/exhaustive-deps
}, [page]);
```

- [ ] **Step 2: Verify TypeScript compiles**

```bash
npx tsc --noEmit 2>&1 | head -30
```

Expected: no errors.

- [ ] **Step 3: Commit**

```bash
git add src/components/customer/CustomerView.tsx
git commit -m "fix: preserve 'sending' step when navigating — don't reset mid-wallet-popup"
```

---

## Self-Review

**Spec coverage check:**
- ✅ `src/lib/unisat.ts` — Task 1
- ✅ `src/hooks/useUnisat.ts` — Task 2  
- ✅ `src/main.tsx` `UnisatProvider` wrap — Task 3
- ✅ Sidebar Bitcoin wallet section (installed/not/connected states) — Task 4
- ✅ `'sending'` step in `useBitcoinSwap` — Task 5
- ✅ UniSat send after deposit address returned — Task 5
- ✅ Fallback to QR when UniSat absent — Task 5 (param optional, QR shows when not passed)
- ✅ Auto-fill `btcRefundAddress` from `unisatAddress` — Task 6
- ✅ `'sending'` display in `BitcoinDepositCard` — Task 6
- ✅ `btcWalletConnected` prop in PayCard — Tasks 6 + 7
- ✅ Hide refund row in PayCard detail + addr panel — Task 7
- ✅ `ctaLabel` / `ctaReady` / click handler updated — Task 7
- ✅ Reset guard for `'sending'` step — Task 8
- ✅ localStorage persistence (`unisat.address`) — Task 2
- ✅ `accountsChanged` event listener — Task 2
- ✅ Network auto-switch to testnet — Task 1
- ✅ Error messages (rejected, insufficient funds) — Task 1

**Type consistency check:**
- `sendBitcoin(to: string, satoshis: number): Promise<string>` — defined in Task 2, consumed in Tasks 5 + 6 ✅
- `unisatSendBitcoin` param name consistent across Tasks 5 + 6 ✅
- `btcWalletConnected: boolean` prop — defined in Task 7, passed in Task 6 ✅
- `BitcoinSwapStep` `'sending'` — added in Task 5, consumed in Tasks 6 + 8 ✅
- `displayStep` mapping covers `'sending'` — Task 6 ✅
