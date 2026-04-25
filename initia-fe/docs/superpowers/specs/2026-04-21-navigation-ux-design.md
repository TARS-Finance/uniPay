# Navigation & UX Redesign — Unified Sticky Header

**Date:** 2026-04-21  
**Status:** Approved  
**Scope:** Replace fragmented navigation (floating home button, CornerFlip, per-view top bars) with a single `AppHeader` component present on all views.

---

## Problem

The current navigation has two critical discoverability failures:

1. **Going home** — a floating icon button in the bottom-left corner. No label, no visual prominence. Users don't find it.
2. **Switching persona** — two separate triggers (`CornerFlip` in top-right corner + a flip button inside each view's header). They look different, live in different places, and give no hint of what they do.

Both newcomers and crypto-native users are equally affected.

---

## Solution: Unified Sticky Header

A single `AppHeader` component replaces all existing navigation entry points. It appears on every view (Landing, Customer, Merchant).

### Layout

```
┌─────────────────────────────────────────────────────────────────┐
│  ⬡ Initia Pay    [  Customer  |  Merchant  ]    [History] [Wallet] │
└─────────────────────────────────────────────────────────────────┘
```

| Zone | Element | Behaviour |
|------|---------|-----------|
| Left | Logo + "Initia Pay" wordmark | Clicking returns to Landing (`persona → null`), no animation |
| Center | Persona toggle pill | Two labeled tabs: Customer / Merchant. Active persona is highlighted. Clicking the inactive tab triggers the existing flip animation. On Landing, both tabs are unselected. |
| Right | Context actions | Customer: History icon + Wallet button. Merchant: New Invoice button + wallet chip. Landing: empty. |

### Header Styling

- Height: ~56px
- Background: glassmorphism (`backdrop-filter: blur(22px)`, dark base, border gradient) matching existing `.glass` class
- Position: `fixed` top, full width, `z-index` above content
- During flip animation: toggle tabs disabled, opacity dimmed to signal transition in progress

---

## Per-View Changes

### Landing Page

- Header present with both toggle tabs unselected
- Existing persona cards remain as primary CTA
- Clicking a card OR clicking the corresponding toggle tab both call `onPick(persona)`
- "Docs" external link stays — moved to subtle text link below the cards

### Customer View

- Internal top bar removed
- Header right zone: History icon button (triggers existing `HistoryDrawer`) + Connect Wallet / wallet address chip
- Body layout: `padding-top: 56px` to account for fixed header
- `PayCard` and `PayProgress` fill remaining vertical space

### Merchant View

- Internal top bar removed
- Header right zone: "New Invoice" button + wallet address chip
- Store name (`crescent-coffee.init`) displayed as a subtitle/page title within the main content area
- Body layout: `padding-top: 56px` to account for fixed header

---

## Persona Switching

- **Single trigger:** clicking the inactive tab in the center toggle calls `onSelectPersona(target)`
- From a persona view: `App.tsx` triggers the existing flip animation → resolves to the target persona view with correct tab highlighted
- From Landing: `App.tsx` sets persona directly (no animation needed)
- Toggle disabled during animation (existing `disabled={flipping}` behaviour preserved)
- `CornerFlip.tsx` component deleted — no longer needed

---

## Components

### New

- `src/components/shell/AppHeader.tsx`
  - Props: `persona: PersonaType`, `flipping: boolean`, `wallet: string | null`, `onHome: () => void`, `onSelectPersona: (target: PersonaType) => void`, `onOpenHistory: () => void`, `onOpenWallet: () => void`, `onNewInvoice: () => void`
  - `onSelectPersona` is called for all toggle tab clicks. `App.tsx` handles the distinction: if `persona === null` (Landing), call `setPersona(target)` directly; otherwise trigger the flip animation.

### Removed

- `src/components/shell/CornerFlip.tsx` — deleted
- Floating home button `div` in `App.tsx` — removed
- Internal top bar JSX in `CustomerView.tsx` — removed
- Internal top bar JSX in `MerchantView.tsx` — removed

### Modified

- `App.tsx` — render `AppHeader`, remove `CornerFlip` and floating home button, pass required props
- `CustomerView.tsx` — remove top bar, add `padding-top` layout adjustment
- `MerchantView.tsx` — remove top bar, add `padding-top` layout adjustment, relocate store name

---

## Out of Scope

- Changes to `HistoryDrawer`, `WalletModal`, `PayCard`, `PayProgress`, or `MerchantView` content
- Any changes to the flip animation itself
- Responsive/mobile-specific breakpoints (no scope change requested)
- Settings, notifications, or additional nav items beyond what currently exists
