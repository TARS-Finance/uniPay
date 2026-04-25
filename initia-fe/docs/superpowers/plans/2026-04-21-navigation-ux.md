# Navigation UX — Unified Sticky Header Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the fragmented navigation (floating home button, CornerFlip corner widget, duplicate per-view flip buttons) with a single `AppHeader` component that appears on every screen.

**Architecture:** A new `AppHeader` component renders at the top of `App.tsx` above all views. Wallet state is lifted from `CustomerView` to `App.tsx` so the header can show wallet status. `CornerFlip.tsx` is deleted. Internal top bars in `CustomerView`, `MerchantView`, and `Landing` are removed.

**Tech Stack:** React 19, Vite, TypeScript, inline styles + existing CSS classes (`.glass`, `.btn`, `.iconbtn`, `.chip`, `.pill`, `.mono`)

---

## File Map

| Action | File |
|--------|------|
| Create | `src/components/shell/AppHeader.tsx` |
| Modify | `src/App.tsx` |
| Modify | `src/components/customer/CustomerView.tsx` |
| Modify | `src/components/merchant/MerchantView.tsx` |
| Modify | `src/components/landing/Landing.tsx` |
| Delete | `src/components/shell/CornerFlip.tsx` |

---

## Task 1: Create AppHeader component

**Files:**
- Create: `src/components/shell/AppHeader.tsx`

- [ ] **Step 1: Create the file with full implementation**

Write `src/components/shell/AppHeader.tsx`:

```tsx
import { shortAddr } from '../../data';
import type { PersonaType } from '../../types';
import { Icons } from '../shared/Icons';

const MERCHANT_WALLET = '0xA4e2bDf091fd';

interface Props {
  persona: PersonaType;
  flipping: boolean;
  wallet: string | null;
  onHome: () => void;
  onSelectPersona: (target: 'customer' | 'merchant') => void;
  onOpenHistory: () => void;
  onOpenWallet: () => void;
  onNewInvoice: () => void;
}

export function AppHeader({ persona, flipping, wallet, onHome, onSelectPersona, onOpenHistory, onOpenWallet, onNewInvoice }: Props) {
  return (
    <header style={{
      position: 'fixed', top: 0, left: 0, right: 0, height: 56, zIndex: 40,
      display: 'flex', alignItems: 'center', justifyContent: 'space-between',
      padding: '0 24px',
      backdropFilter: 'blur(22px)',
      WebkitBackdropFilter: 'blur(22px)',
      background: 'rgba(5,7,14,0.72)',
      borderBottom: '1px solid var(--hairline)',
    }}>
      {/* Logo / Home */}
      <button
        onClick={onHome}
        style={{
          display: 'flex', alignItems: 'center', gap: 10,
          background: 'none', border: 'none', cursor: 'pointer',
          padding: '4px 8px', borderRadius: 8, color: 'var(--text-0)',
        }}
      >
        <div style={{ width: 26, height: 26, borderRadius: 7, background: 'linear-gradient(135deg, #e8ecf3, #8792a6)', display: 'grid', placeItems: 'center', color: '#06090f', flexShrink: 0 }}>
          <Icons.logo />
        </div>
        <span style={{ fontWeight: 600, fontSize: 13 }}>Initia Pay</span>
      </button>

      {/* Persona toggle */}
      <div style={{
        display: 'flex', background: 'rgba(255,255,255,0.04)',
        borderRadius: 10, padding: 3, border: '1px solid var(--hairline)', gap: 2,
        opacity: flipping ? 0.5 : 1, transition: 'opacity 200ms',
      }}>
        {(['customer', 'merchant'] as const).map((p) => (
          <button
            key={p}
            onClick={() => onSelectPersona(p)}
            disabled={flipping}
            style={{
              padding: '5px 18px', borderRadius: 7, fontSize: 13, fontWeight: 500,
              border: 'none', cursor: flipping ? 'wait' : 'pointer',
              background: persona === p ? 'rgba(255,255,255,0.09)' : 'transparent',
              color: persona === p ? 'var(--text-0)' : 'var(--text-3)',
              transition: 'background 200ms, color 200ms',
              textTransform: 'capitalize',
            }}
          >
            {p}
          </button>
        ))}
      </div>

      {/* Context actions */}
      <div style={{ display: 'flex', gap: 8, alignItems: 'center', minWidth: 200, justifyContent: 'flex-end' }}>
        {persona === null && (
          <>
            <span className="pill" style={{ color: 'var(--text-2)' }}>
              <span className="dot" style={{ background: 'var(--success)' }} />Testnet · v0.4
            </span>
            <button className="btn btn-ghost">Docs <Icons.ext /></button>
          </>
        )}
        {persona === 'customer' && (
          <>
            <button className="iconbtn" onClick={onOpenHistory} title="History">
              <Icons.history />
            </button>
            {wallet ? (
              <div className="chip" style={{ cursor: 'default' }}>
                <span className="logo lt-eth">W</span>
                <span className="mono" style={{ fontSize: 12 }}>{shortAddr(wallet)}</span>
              </div>
            ) : (
              <button className="btn" onClick={onOpenWallet}><Icons.wallet /> Connect wallet</button>
            )}
          </>
        )}
        {persona === 'merchant' && (
          <>
            <button className="btn btn-ghost" onClick={onNewInvoice}><Icons.plus /> New invoice</button>
            <div className="chip" style={{ cursor: 'default' }}>
              <span className="logo lt-initia">M</span>
              <span className="mono" style={{ fontSize: 12 }}>{shortAddr(MERCHANT_WALLET)}</span>
              <Icons.chevron style={{ color: 'var(--text-3)' }} />
            </div>
          </>
        )}
      </div>
    </header>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/components/shell/AppHeader.tsx
git commit -m "feat: add AppHeader unified navigation component"
```

---

## Task 2: Wire AppHeader into App.tsx, lift wallet state, remove CornerFlip

**Files:**
- Modify: `src/App.tsx`

- [ ] **Step 1: Replace the full contents of `src/App.tsx`**

```tsx
import { useState, useCallback } from 'react';
import './styles.css';
import type { PersonaType } from './types';
import { Landing } from './components/landing/Landing';
import { CustomerView } from './components/customer/CustomerView';
import { MerchantView } from './components/merchant/MerchantView';
import { WalletModal } from './components/shell/WalletModal';
import { AppHeader } from './components/shell/AppHeader';

type WalletCb = (addr: string) => void;

export default function App() {
  const [persona, setPersona] = useState<PersonaType>(() => {
    try { return (localStorage.getItem('initia.persona') as PersonaType) || null; } catch { return null; }
  });
  const [flipping, setFlipping] = useState(false);
  const [flipDir, setFlipDir] = useState(1);
  const [walletCb, setWalletCb] = useState<WalletCb | null>(null);
  const [openHistory, setOpenHistory] = useState(false);
  const [wallet, setWallet] = useState<string | null>(null);

  const transition = (next: PersonaType, dir: number) => {
    setFlipping(true);
    setFlipDir(dir);
    setTimeout(() => {
      setPersona(next);
      try { next ? localStorage.setItem('initia.persona', next) : localStorage.removeItem('initia.persona'); } catch { /* ignore */ }
    }, 300);
    setTimeout(() => setFlipping(false), 700);
  };

  const goHome = () => transition(null, -1);

  const selectPersona = (target: 'customer' | 'merchant') => {
    if (flipping) return;
    if (persona === null) {
      transition(target, 1);
    } else if (persona !== target) {
      transition(target, persona === 'customer' ? -1 : 1);
    }
  };

  const openWallet = useCallback((cb: WalletCb) => {
    setWalletCb(() => cb);
  }, []);

  const handleHeaderWalletOpen = useCallback(() => {
    openWallet(setWallet);
  }, [openWallet]);

  const handleCustomerWalletOpen = useCallback((cb: WalletCb) => {
    openWallet((addr) => { setWallet(addr); cb(addr); });
  }, [openWallet]);

  const flipStyle: React.CSSProperties = {
    position: 'absolute', inset: 0,
    transformStyle: 'preserve-3d',
    transition: 'transform 600ms cubic-bezier(.65,0,.35,1), opacity 600ms ease',
    transform: flipping
      ? `perspective(2000px) rotateY(${flipDir * 14}deg) rotateX(-6deg) scale(0.96)`
      : 'none',
    transformOrigin: flipDir > 0 ? 'right center' : 'left center',
  };

  return (
    <div className={`persona-${persona || 'customer'}`} style={{ position: 'absolute', inset: 0, overflow: 'hidden', background: 'var(--bg-0)' }}>
      <AppHeader
        persona={persona}
        flipping={flipping}
        wallet={wallet}
        onHome={goHome}
        onSelectPersona={selectPersona}
        onOpenHistory={() => setOpenHistory(true)}
        onOpenWallet={handleHeaderWalletOpen}
        onNewInvoice={() => {}}
      />

      <div style={flipStyle}>
        {!persona && <Landing onPick={(p) => transition(p, 1)} />}
        {persona === 'customer' && (
          <CustomerView
            wallet={wallet}
            onOpenWallet={handleCustomerWalletOpen}
            openHistory={openHistory}
            setOpenHistory={setOpenHistory}
          />
        )}
        {persona === 'merchant' && (
          <MerchantView />
        )}
      </div>

      {flipping && (
        <div style={{ position: 'absolute', inset: 0, pointerEvents: 'none', zIndex: 20, overflow: 'hidden' }}>
          <div style={{
            position: 'absolute', top: 0, right: 0, width: '200%', height: '200%',
            background: 'linear-gradient(135deg, rgba(20,30,50,0.0) 45%, rgba(100,130,180,0.35) 50%, rgba(10,14,24,0.85) 55%, rgba(5,7,14,0.95) 100%)',
            transform: 'translateX(-10%) translateY(-10%)',
            animation: 'curl-sweep 600ms ease-in-out forwards',
          }} />
        </div>
      )}

      {walletCb && (
        <WalletModal
          onClose={() => setWalletCb(null)}
          onPick={(addr) => { walletCb(addr); setWalletCb(null); }}
        />
      )}
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add src/App.tsx
git commit -m "feat: wire AppHeader into App, lift wallet state, remove CornerFlip usage"
```

---

## Task 3: Strip CustomerView top bar

**Files:**
- Modify: `src/components/customer/CustomerView.tsx`

- [ ] **Step 1: Replace the Props interface and component signature**

Replace lines 10–19:
```tsx
interface Props {
  onOpenWallet: (cb: (addr: string) => void) => void;
  onOpenFlip: () => void;
  openHistory: boolean;
  setOpenHistory: (v: boolean) => void;
}

type FlowState = 'idle' | 'paying' | 'done';

export function CustomerView({ onOpenWallet, onOpenFlip, openHistory, setOpenHistory }: Props) {
```
with:
```tsx
interface Props {
  wallet: string | null;
  onOpenWallet: (cb: (addr: string) => void) => void;
  openHistory: boolean;
  setOpenHistory: (v: boolean) => void;
}

type FlowState = 'idle' | 'paying' | 'done';

export function CustomerView({ wallet, onOpenWallet, openHistory, setOpenHistory }: Props) {
```

- [ ] **Step 2: Remove the local wallet state declaration**

Remove line:
```tsx
  const [wallet, setWallet] = useState<string | null>(null);
```

- [ ] **Step 3: Update the onPay function to not call setWallet**

Replace:
```tsx
  const onPay = () => {
    if (!wallet) {
      onOpenWallet((addr) => { setWallet(addr); setShowQR(true); setFlowState('paying'); });
      return;
    }
    setShowQR(true);
    setFlowState('paying');
  };
```
with:
```tsx
  const onPay = () => {
    if (!wallet) {
      onOpenWallet((_addr) => { setShowQR(true); setFlowState('paying'); });
      return;
    }
    setShowQR(true);
    setFlowState('paying');
  };
```

- [ ] **Step 4: Remove the top bar JSX and add paddingTop to outer div**

Replace the outer div opening tag and top bar block:
```tsx
  return (
    <div className="persona-customer" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column' }}>
      <div className="ambient" style={{ '--g1': 'rgba(80,120,220,0.20)', '--g2': 'rgba(40,70,150,0.16)' } as React.CSSProperties} />
      <div className="gridlines" />

      {/* Top bar */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '18px 28px', position: 'relative', zIndex: 2 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <div style={{ width: 28, height: 28, borderRadius: 8, background: 'linear-gradient(135deg, #e8ecf3, #8792a6)', display: 'grid', placeItems: 'center', color: '#06090f' }}>
            <Icons.logo />
          </div>
          <div style={{ fontWeight: 600, fontSize: 13 }}>Initia Pay</div>
          <span className="pill" style={{ marginLeft: 8 }}><span className="dot" style={{ background: 'var(--accent)' }} />Customer</span>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <button className="iconbtn" onClick={() => setOpenHistory(true)} title="History">
            <Icons.history />
          </button>
          {wallet ? (
            <div className="chip" style={{ cursor: 'default' }}>
              <span className="logo lt-eth">W</span>
              <span className="mono" style={{ fontSize: 12 }}>{shortAddr(wallet)}</span>
            </div>
          ) : (
            <button className="btn" onClick={() => onOpenWallet(setWallet)}><Icons.wallet /> Connect wallet</button>
          )}
          <button className="iconbtn" onClick={onOpenFlip} title="Switch to Merchant" style={{ color: 'var(--accent)' }}>
            <Icons.flip />
          </button>
        </div>
      </div>

      {/* Main */}
      <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: '0 28px 28px', position: 'relative', zIndex: 2 }}>
```
with:
```tsx
  return (
    <div className="persona-customer" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column', paddingTop: 56 }}>
      <div className="ambient" style={{ '--g1': 'rgba(80,120,220,0.20)', '--g2': 'rgba(40,70,150,0.16)' } as React.CSSProperties} />
      <div className="gridlines" />

      {/* Main */}
      <div style={{ flex: 1, display: 'grid', placeItems: 'center', padding: '0 28px 28px', position: 'relative', zIndex: 2 }}>
```

- [ ] **Step 5: Verify TypeScript compiles**

```bash
cd /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend && npx tsc --noEmit
```
Expected: no errors

- [ ] **Step 6: Commit**

```bash
git add src/components/customer/CustomerView.tsx
git commit -m "feat: remove CustomerView top bar, accept wallet as prop"
```

---

## Task 4: Strip MerchantView top bar

**Files:**
- Modify: `src/components/merchant/MerchantView.tsx`

- [ ] **Step 1: Replace Props interface and component signature**

Replace lines 9–14:
```tsx
interface Props {
  onOpenWallet: (cb: (addr: string) => void) => void;
  onOpenFlip: () => void;
}

export function MerchantView({ onOpenFlip }: Props) {
```
with:
```tsx
export function MerchantView() {
```

- [ ] **Step 2: Remove the top bar JSX, add paddingTop, and move store name**

Replace the outer div opening and top bar block:
```tsx
  return (
    <div className="persona-merchant" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column' }}>
      <div className="ambient" style={{ '--g1': 'rgba(140,170,110,0.16)', '--g2': 'rgba(180,150,90,0.14)' } as React.CSSProperties} />
      <div className="gridlines" />

      {/* Top bar */}
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '18px 28px', position: 'relative', zIndex: 2 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
          <div style={{ width: 28, height: 28, borderRadius: 8, background: 'linear-gradient(135deg, #e8ecf3, #8792a6)', display: 'grid', placeItems: 'center', color: '#06090f' }}>
            <Icons.logo />
          </div>
          <div style={{ fontWeight: 600, fontSize: 13 }}>Initia Pay</div>
          <span className="pill pill-accent" style={{ marginLeft: 8 }}><span className="dot" />Merchant</span>
          <span className="mono" style={{ fontSize: 11, color: 'var(--text-3)', marginLeft: 6 }}>/ crescent-coffee.init</span>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <button className="btn btn-ghost"><Icons.plus /> New invoice</button>
          <div className="chip" style={{ cursor: 'default' }}>
            <span className="logo lt-initia">M</span>
            <span className="mono" style={{ fontSize: 12 }}>{shortAddr(WALLET)}</span>
            <Icons.chevron style={{ color: 'var(--text-3)' }} />
          </div>
          <button className="iconbtn" onClick={onOpenFlip} title="Switch to Customer" style={{ color: 'var(--accent)' }}>
            <Icons.flip />
          </button>
        </div>
      </div>

      <div style={{ flex: 1, display: 'grid', gridTemplateColumns: '1fr 360px', gap: 20, padding: '0 28px 24px', overflow: 'hidden', position: 'relative', zIndex: 2 }}>
```
with:
```tsx
  return (
    <div className="persona-merchant" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column', paddingTop: 56 }}>
      <div className="ambient" style={{ '--g1': 'rgba(140,170,110,0.16)', '--g2': 'rgba(180,150,90,0.14)' } as React.CSSProperties} />
      <div className="gridlines" />

      <div style={{ flex: 1, display: 'grid', gridTemplateColumns: '1fr 360px', gap: 20, padding: '8px 28px 24px', overflow: 'hidden', position: 'relative', zIndex: 2 }}>
```

- [ ] **Step 3: Add store name subtitle inside the main content area**

Inside the main content `<div>` (the one with `overflowY: 'auto'`), add the store name before the stat row. Replace:
```tsx
        <div style={{ overflowY: 'auto', paddingRight: 4, display: 'flex', flexDirection: 'column', gap: 18 }}>
          {/* Stat row */}
```
with:
```tsx
        <div style={{ overflowY: 'auto', paddingRight: 4, display: 'flex', flexDirection: 'column', gap: 18 }}>
          <div style={{ display: 'flex', alignItems: 'baseline', gap: 10 }}>
            <span style={{ fontSize: 17, fontWeight: 500 }}>Dashboard</span>
            <span className="mono" style={{ fontSize: 12, color: 'var(--text-3)' }}>crescent-coffee.init</span>
          </div>
          {/* Stat row */}
```

- [ ] **Step 4: Verify TypeScript compiles**

```bash
cd /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend && npx tsc --noEmit
```
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/components/merchant/MerchantView.tsx
git commit -m "feat: remove MerchantView top bar, move store name to content area"
```

---

## Task 5: Strip Landing top bar and update hint text

**Files:**
- Modify: `src/components/landing/Landing.tsx`

- [ ] **Step 1: Remove the Landing internal top bar**

Remove lines 21–35 (the entire internal header `<div>`):
```tsx
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', padding: '22px 32px', position: 'relative', zIndex: 2 }}>
        <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
          <div style={{ width: 30, height: 30, borderRadius: 8, background: 'linear-gradient(135deg, #e8ecf3, #8792a6)', display: 'grid', placeItems: 'center', color: '#06090f' }}>
            <Icons.logo />
          </div>
          <div>
            <div style={{ fontWeight: 600, fontSize: 14, letterSpacing: '-0.01em' }}>Initia</div>
            <div className="mono" style={{ fontSize: 10, color: 'var(--text-3)', letterSpacing: '0.08em', textTransform: 'uppercase' }}>Universal Pay</div>
          </div>
        </div>
        <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
          <span className="pill" style={{ color: 'var(--text-2)' }}><span className="dot" style={{ background: 'var(--success)' }} />Testnet · v0.4</span>
          <button className="btn btn-ghost">Docs <Icons.ext /></button>
        </div>
      </div>
```

- [ ] **Step 2: Add paddingTop to the Landing outer div**

Replace:
```tsx
    <div className="landing" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column' }}>
```
with:
```tsx
    <div className="landing" style={{ position: 'absolute', inset: 0, display: 'flex', flexDirection: 'column', paddingTop: 56 }}>
```

- [ ] **Step 3: Update the hint text**

Replace:
```tsx
          <span>switch anytime via corner flip</span>
```
with:
```tsx
          <span>switch roles anytime via the header</span>
```

- [ ] **Step 4: Verify TypeScript compiles**

```bash
cd /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend && npx tsc --noEmit
```
Expected: no errors

- [ ] **Step 5: Commit**

```bash
git add src/components/landing/Landing.tsx
git commit -m "feat: remove Landing internal header, add paddingTop for AppHeader"
```

---

## Task 6: Delete CornerFlip.tsx

**Files:**
- Delete: `src/components/shell/CornerFlip.tsx`

- [ ] **Step 1: Delete the file**

```bash
rm /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend/src/components/shell/CornerFlip.tsx
```

- [ ] **Step 2: Verify no remaining imports**

```bash
grep -r "CornerFlip" /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend/src
```
Expected: no output

- [ ] **Step 3: Verify TypeScript compiles cleanly**

```bash
cd /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend && npx tsc --noEmit
```
Expected: no errors

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "chore: delete CornerFlip component (replaced by AppHeader toggle)"
```

---

## Task 7: Browser verification

**Files:** none

- [ ] **Step 1: Start dev server**

```bash
cd /Users/svssathvik/Desktop/sathvik/my-projects/initia/frontend && npm run dev
```
Expected: server starts on `http://localhost:5173` (or similar)

- [ ] **Step 2: Verify Landing page**

Open browser to localhost. Check:
- AppHeader is visible at the top with logo, both tabs unselected, Testnet pill + Docs button on right
- Landing hero and persona cards are visible below the header (not hidden behind it)
- Clicking "Customer" card navigates to customer view
- Clicking "Merchant" card navigates to merchant view
- Clicking either tab in the header also navigates to that view

- [ ] **Step 3: Verify Customer view**

In customer view check:
- AppHeader shows "Customer" tab highlighted
- History icon and "Connect wallet" button appear on the right
- No duplicate top bar inside the view
- Clicking "Connect wallet" opens WalletModal
- After connecting wallet, header shows abbreviated address
- Clicking "Merchant" tab in header triggers flip animation and shows merchant view
- Clicking logo returns to Landing

- [ ] **Step 4: Verify Merchant view**

In merchant view check:
- AppHeader shows "Merchant" tab highlighted
- "New invoice" button and wallet address chip appear on the right
- "Dashboard / crescent-coffee.init" subtitle is visible in the content area
- No duplicate top bar inside the view
- Clicking "Customer" tab triggers flip animation and shows customer view
- Clicking logo returns to Landing

- [ ] **Step 5: Verify flip animation**

Flip between views multiple times. Check:
- Toggle tabs dim during animation
- Only one flip trigger exists (the header toggle)
- No ghost buttons or layout shifts during animation
