# UniSat Wallet Integration Design

**Date:** 2026-04-25  
**Status:** Approved  

---

## Overview

Add UniSat Bitcoin wallet support to the app, giving Bitcoin swaps the same UX quality as EVM (MetaMask) and Solana (Phantom) flows. When UniSat is connected:

- BTC refund address is auto-populated from the wallet (no manual entry)
- After the deposit address is returned by the backend, UniSat signs and sends BTC directly (no QR code)
- The sidebar shows a persistent Bitcoin wallet entry alongside the EVM wallet

The existing QR + manual deposit flow remains as a fallback when UniSat is not installed or not connected.

---

## Architecture

```
src/lib/unisat.ts                        raw window.unisat API wrapper + types
src/hooks/useUnisat.ts                   UnisatContext + UnisatProvider + useUnisat()
src/main.tsx                             wrap app in <UnisatProvider>
src/components/shell/Sidebar.tsx         Bitcoin wallet section
src/hooks/useBitcoinSwap.ts             consume useUnisat() — send BTC + auto-fill refund addr
src/components/customer/CustomerView.tsx auto-populate btcRefundAddress, conditional QR
```

---

## Components

### `src/lib/unisat.ts`

**Types:**

```ts
interface UnisatProvider {
  requestAccounts(): Promise<string[]>
  getAccounts(): Promise<string[]>
  getNetwork(): Promise<'livenet' | 'testnet'>
  switchNetwork(network: 'livenet' | 'testnet'): Promise<void>
  sendBitcoin(toAddress: string, satoshis: number, options?: { feeRate?: number }): Promise<string>
  on(event: 'accountsChanged' | 'networkChanged', handler: (data: unknown) => void): void
  removeListener(event: string, handler: (data: unknown) => void): void
}
```

**Exports:**
- `getUnisat(): UnisatProvider | null` — returns `window.unisat` or null
- `sendBitcoinViaUnisat(to: string, satoshis: number): Promise<string>` — switches to testnet if needed, calls `sendBitcoin`, returns txid. Throws descriptive errors (user rejected, insufficient funds, wrong network).

Network target: `'testnet'` (matches `bitcoin_testnet` strategy).

---

### `src/hooks/useUnisat.ts`

**Context shape:**

```ts
interface UnisatContextValue {
  unisatAddress: string | null
  connecting: boolean
  error: string | null
  isInstalled: boolean
  connectUnisat(): Promise<void>
  disconnectUnisat(): void
  sendBitcoin(to: string, satoshis: number): Promise<string>
}
```

**Behaviour:**
- `UnisatProvider` wraps children; reads persisted address from `localStorage('unisat.address')` on mount
- `connectUnisat()` — calls `requestAccounts()`, stores first account in state + localStorage
- `disconnectUnisat()` — clears state + localStorage key
- Registers `accountsChanged` listener on mount: updates address or clears if array is empty
- Registers `networkChanged` listener: no-op for now (network switched in `sendBitcoinViaUnisat`)
- `sendBitcoin` delegates to `sendBitcoinViaUnisat` from lib; surfaces errors via `error` state
- `isInstalled` — `getUnisat() !== null`, checked once on mount

---

### `src/main.tsx`

Wrap `<App>` (inside existing providers) with `<UnisatProvider>`.

---

### `src/components/shell/Sidebar.tsx`

Add a **Bitcoin wallet section** below the EVM wallet block:

- **Not installed:** small grey "UniSat not detected" label (links to unisat.io)
- **Installed, not connected:** "Connect Bitcoin" button with UniSat logo
- **Connected:** UniSat logo + truncated address (`tb1p…xxxx`), click opens a small dropdown with "Disconnect"

The section only renders when `srcChain` is not available in sidebar — it is always visible regardless of selected chain, matching how the EVM wallet entry works.

---

### `src/hooks/useBitcoinSwap.ts`

**New step:** Add `'sending'` to `BitcoinSwapStep` union, between `'creating'` and `'awaiting'`.

**`startBitcoinSwap` changes:**
1. Create order as before → receive `depositAddress` + `amountSats`
2. **If UniSat connected:**
   - Set step to `'sending'`
   - Call `sendBitcoin(depositAddress, amountSats)` — triggers UniSat popup
   - On success: store txid, advance to `'awaiting'`, begin polling
   - On rejection/error: set step to `'error'`, surface message
3. **If UniSat not connected:** advance to `'awaiting'` as today (QR display)

Hook accepts `sendBitcoin` and `unisatAddress` as parameters (passed from `CustomerView` via `useUnisat()`), keeping the hook free of direct context dependency (consistent with how `useSolanaSwap` works).

---

### `src/components/customer/CustomerView.tsx`

- Call `useUnisat()` at top of component
- Effect: when `srcChain === 'bitcoin_testnet' && unisatAddress`, set `btcRefundAddress` to `unisatAddress`. Clear when chain changes away from bitcoin or UniSat disconnects.
- Hide the BTC refund address input row in `PayCard` when `unisatAddress` is set (address is auto-filled, no need to show it)
- Pass `sendBitcoin` and `unisatAddress` into `useBitcoinSwap`
- QR card: still rendered when UniSat not connected or when step falls back to `'awaiting'` without a wallet send

---

### `src/components/customer/PayCard.tsx`

- When `isBitcoin && unisatConnected` (new prop `btcWalletConnected: boolean`): skip the "BTC refund address" detail row and the refund address field in the addr panel — it's auto-filled
- `ctaLabel`: remove "Enter BTC refund address" case when wallet is connected

---

### `src/components/customer/PayProgress.tsx` (or equivalent Bitcoin progress UI)

Add display for the `'sending'` step:
- Label: "Sending BTC"
- Description: "Confirm the transaction in your UniSat wallet…"
- Step indicator: sits between "Creating order" and "Awaiting deposit"

---

## Data Flow

```
User clicks "Connect Bitcoin" in Sidebar
  → connectUnisat() → window.unisat.requestAccounts()
  → unisatAddress stored in context + localStorage

User selects BTC as source in CustomerView
  → useEffect: btcRefundAddress ← unisatAddress (auto-fill)
  → PayCard hides refund address row

User clicks Pay
  → order created (refund pubkey derived from btcRefundAddress as before)
  → depositAddress returned
  → step = 'sending'
  → sendBitcoinViaUnisat(depositAddress, amountSats)
  → UniSat popup shown to user
  → on confirm: txid stored, step = 'awaiting'
  → polling begins (unchanged from today)
```

---

## Fallback Behaviour

| Condition | Behaviour |
|-----------|-----------|
| UniSat not installed | Sidebar shows "not detected" note; BTC flow unchanged (manual refund addr + QR) |
| UniSat installed but not connected | Sidebar shows Connect button; BTC flow unchanged |
| UniSat connected, user rejects send | Step → `'error'` with message "Transaction rejected in wallet" |
| UniSat connected, wrong network | `sendBitcoinViaUnisat` switches to testnet automatically before sending |

---

## Error Handling

- `connectUnisat()` — catches and surfaces error in `error` state; does not throw
- `sendBitcoinViaUnisat()` — maps known UniSat error codes to friendly messages:
  - User rejected → "Transaction rejected in UniSat wallet"
  - Insufficient funds → "Insufficient BTC balance"
  - Unknown → raw error message
- All errors displayed in the existing Bitcoin progress error UI

---

## Storage Keys

| Key | Value |
|-----|-------|
| `unisat.address` | Connected BTC address (string or absent) |

Existing BTC keys (`btc.pending_order`, `btc.secret.*`) unchanged.

---

## Out of Scope

- Mainnet support (testnet only, matching the rest of the app)
- PSBT signing
- Other Bitcoin wallets (Xverse, OKX) — same `window.unisat` API shape means they could be added later with minimal changes
- Balance display in sidebar
