# uniPay — DoraHacks BUIDL Submission

Copy each field below directly into the corresponding field on the DoraHacks BUIDL "Edit" form.

---

## Project Name

uniPay

---

## Logo

Upload the UniPay mark from the design bundle (the U-shape with the inward arrow, primary color #5B8DEF).

---

## One-line Description / Tagline

Pay from any chain. Settle and earn on Initia.

---

## Category / Tags

DeFi, Payments, Cross-chain, Appchain, Initia, HTLC, Atomic Swap

---

## Project Description (long form)

### What uniPay is

uniPay is a universal merchant checkout that lets a customer pay from any chain — Bitcoin, Solana, Ethereum, Arbitrum, Base, BNB, Citrea, Hyperliquid — and the merchant always receives clean, settled USDC on Initia. One invoice link, one destination token, one dashboard.

A merchant creates an invoice by fixing only the things they care about: the asset they want to receive and the amount. They share the link. Whoever opens it pays in whatever they're already holding — no bridging, no swapping, no second wallet. The merchant's invoice is honored exactly, every time, from every chain.

### Why this is only possible on Initia

Most payment processors take a merchant's funds and park them. uniPay does the opposite. The moment funds settle, the merchant can flip a single switch — auto-stake — and from that point on, every dollar that arrives is put to work.

Idle merchant USDC is deposited into an Initia DEX liquidity pool, earning swap fees on every trade that flows through it. Thanks to Initia's Enshrined Liquidity, the LP token from that position is also a valid staking asset, so it gets delegated to a validator and earns staking rewards in parallel. Same dollar, two yields, simultaneously.

On any other chain, capital has to choose: be liquidity, or be security. On Initia, it's both. uniPay productizes that difference and turns it into a yield layer for merchants.

### How settlement works

Cross-chain settlement is fully trustless. uniPay uses Hashed Timelock Contracts (HTLCs): the customer locks the source asset behind a secret hash, a solver locks the destination asset on the Initia rollup behind the same hash, the customer reveals the secret to claim the destination side, and the solver replays the now-public secret to claim the source side. Atomic — both sides settle, or neither does. If the solver ever fails to lock the destination side, the customer's funds refund automatically after the timelock. Funds cannot get stuck.

Settlement completes in under thirty seconds.

### The flywheel

Every dollar that flows through uniPay deepens Initia's DEX liquidity and adds to its economic security. The more merchants we onboard, the stronger the host chain becomes. uniPay isn't just a payment rail — it's a security and liquidity bootstrap for the chain it runs on.

---

## What it does (short summary, ~50 words)

uniPay is a multi-chain merchant checkout. Customers pay in BTC, SOL, or USDC on any major EVM, and merchants receive USDC on Initia within ~30 seconds via trustless HTLC atomic swaps. Optional auto-stake routes idle merchant balances into Initia DEX LP positions that simultaneously earn swap fees and staking rewards via Enshrined Liquidity.

---

## Problem Statement

Merchants accepting crypto today face three compounding problems:

**Fragmentation.** Each chain requires a separate wallet, a separate bridge, and a separate settlement window. A merchant accepting eight chains is operating eight payment rails.

**Customer friction.** Customers are forced to bridge or swap into the merchant's preferred token before paying — adding fees, minutes, and abandonment risk to every checkout.

**Idle float.** Once a merchant is paid, their stablecoin balance earns nothing. Stripe, Coinbase Commerce, and every existing crypto rail treat merchant treasury as dead weight between sale and payroll.

The conventional fix to (3) — staking the float — destroys liquidity. The fix to fragmentation — bridging — introduces custodians and trust assumptions. No existing payment processor solves all three at once.

---

## Solution

uniPay solves all three simultaneously by exploiting two primitives:

**HTLC atomic swaps** for trustless cross-chain settlement, eliminating the need for bridges or custodians and making any-chain-in possible.

**Initia's Enshrined Liquidity** for productive merchant float, eliminating the staking-vs-liquidity tradeoff and making earning-while-idle possible from a single LP position.

The result is a checkout where the customer pays with what they already hold, the merchant receives exactly the invoice in a single token, and the merchant's float earns two yield streams from the moment funds land — without any second deposit, manual rebalancing, or active management.

---

## Native Initia Feature Used

**Interwoven Bridge (InterwovenKit)**

The frontend uses InterwovenKit for wallet connection and Initia transaction flows, and the merchant experience is wired to Initia bridge actions so users can move value between the uniPay rollup and Initia L1 with the native Initia UX. Merchant earn flows additionally exploit Enshrined Liquidity — LP positions on the Initia DEX are simultaneously delegated to validators, giving merchants two reward streams from a single position.

---

## Tech Stack

### Settlement & Smart Contracts

- Custom Initia EVM rollup (`initia-rollup`, chain ID `tars-1`) with deployed HTLC contracts (`0x127d91a5898e6138bfbec9ab87ef11026db87cdb`)
- Hashed Timelock Contracts on Bitcoin, EVM, Solana, and Initia for atomic cross-chain settlement
- Initia Interwoven Bridge for L1 ↔ rollup value movement

### Frontend

- React + Vite (`initia-fe`)
- InterwovenKit for Initia wallet connection and transaction flows
- Customer checkout, merchant dashboard, and invoice generation

### Backend Services

- `orderbook` — quoting, routing, and matched source/destination swap state (source of truth)
- `solver-orders` — pending intent cache feeding solvers and executors
- `stacker` — merchant balance, staking, withdrawal, and keeper operations

### Per-chain Watcher / Executor Services

- `btc-watcher` / `btc-executor` — Bitcoin source detection and execution
- `evm-watcher` / `evm-executor` — EVM source detection and execution (Ethereum, Arbitrum, Base, BNB, Citrea, Hyperliquid)
- `solana-watcher` / `solana-executor` — Solana source detection and execution
- `initia-watcher` / `initia-executor` — Initia destination monitoring, lock, redeem, refund

---

## How It Works (technical flow)

1. Customer selects source asset on the uniPay frontend; frontend generates a secret + secretHash locally.
2. Frontend hits `orderbook` to create an order with source asset, destination terms, and the secret hash. `orderbook` quotes the route and persists matched source/destination swap legs, then publishes a pending intent to `solver-orders`.
3. Customer sends the source-side asset to the HTLC deposit address.
4. The relevant `*-watcher` detects the source HTLC funding and records `initiate_tx_hash` to `orderbook`.
5. `initia-executor` pulls the pending intent and locks destination liquidity in the Initia HTLC.
6. Frontend polls until the destination leg is locked, then POSTs the secret to `initia-executor`.
7. `initia-executor` redeems the destination HTLC with the secret, releasing USDC on Initia to the merchant.
8. The source-side `*-executor` reads the now-revealed secret from `orderbook` and redeems the source HTLC, claiming the customer's deposit.
9. Merchant receives USDC on the Initia rollup. If auto-stake is enabled, `stacker` immediately routes the balance into an Initia DEX LP position which is concurrently delegated to a validator via Enshrined Liquidity.

If the solver fails to lock the destination leg within the timelock window, the customer's source-side funds refund automatically. No funds can be stuck.

---

## Challenges We Ran Into

- Synchronizing HTLC parameters (timelock windows, hash function variants) across four very different settlement environments — Bitcoin Script, EVM, SVM, and Initia EVM — required a careful state machine in `orderbook` that treats each leg as a first-class object with its own confirmations, retries, and refund clock.
- Building a solver economics model that incentivizes destination-side liquidity locking even when source-side confirmation latency is variable (Bitcoin in particular).
- Wiring InterwovenKit into a merchant-facing dashboard while keeping the customer-facing checkout chain-agnostic — two very different UX surfaces sharing one wallet primitive.
- Auto-stake routing: getting merchant USDC into a paired LP position and delegating the resulting LP token to a validator atomically, in a single user-visible action, instead of three separate transactions.

---

## Accomplishments We're Proud Of

- End-to-end live demo across four ecosystems: real BTC, real SOL, real EVM USDC, settling as USDC on the uniPay Initia rollup.
- Sub-30-second settlement across all four source chains under typical mainnet conditions.
- Two-yield merchant float working live — every dollar in the merchant balance is simultaneously providing liquidity to the Initia DEX and securing the chain through validator delegation.
- The system is fully trustless — no custodian holds customer or merchant funds at any point.
- One invoice, multiple payers — the same merchant invoice can be paid in parallel from different chains in different source amounts, and the merchant receives identical destination credits.

---

## What We Learned

Productive payments aren't a feature — they're a category. Most payment processors are optimized for the moment of payment; uniPay treats the entire lifecycle of a merchant's balance as the product, with settlement as the entry point and yield as the steady state. That reframe is only economically possible on a chain where staking and liquidity don't compete for the same capital, which is why Initia's Enshrined Liquidity isn't a "nice-to-have" for us — it's the substrate the entire merchant value proposition rests on.

---

## What's Next

- Mainnet launch with onboarded merchant pilots
- Additional source chains (Tron, Litecoin, Monad, MegaETH, Botanix, Starknet — already scaffolded in the chain-logos library)
- Solver network expansion with a public solver SDK
- Merchant API and SDK for embedded checkout
- Yield analytics dashboard for merchants (APR breakdown, swap fees vs. staking rewards)

---

## Links

| Field | Value |
|-------|-------|
| GitHub | https://github.com/TARS-Finance/uniPay |
| Demo Link | <!-- TODO: live frontend URL --> |
| Demo Video | <!-- TODO: Loom / YouTube 3-minute pitch video --> |
| Pitch Deck | <!-- TODO: hosted deck link --> |
| Twitter / X | <!-- TODO: @unipay or team handle --> |
| Website | <!-- TODO: unipay.money or landing page --> |

> ⚠️ DoraHacks requires URLs in `https://` format. Bare domains will fail validation.

---

## Team

<!-- Fill in 2–4 members. Include name/handle, role, and GitHub or Twitter link per member. -->

| Name / Handle | Role | Link |
|---------------|------|------|
| <!-- name --> | <!-- role --> | <!-- link --> |

---

## Track Selection

Select the track most aligned with: **DeFi / Payments / Appchain**.

If a "Best use of Enshrined Liquidity" or "Interwoven Bridge" track exists, apply to that one — uniPay's strongest case is on Initia-native primitives, not generic DeFi.

---

## Submission Checklist

Before clicking "Submit BUIDL":

- [ ] Project Name: uniPay
- [ ] Logo uploaded
- [ ] Long description pasted
- [ ] GitHub link added: https://github.com/TARS-Finance/uniPay
- [ ] Demo video link added
- [ ] Live demo URL added
- [ ] Tech stack listed
- [ ] Native Initia feature stated (InterwovenKit / Enshrined Liquidity)
- [ ] Track selected
- [ ] Team members added (2–4 people)
- [ ] "I agree to User Agreement" checked
- [ ] Apply to INITIATE hackathon from "Apply with Existing BUIDL"

> After submission, your BUIDL needs Initia Labs approval before it appears in the public gallery. Reach out via the official INITIATE Telegram or Discord if approval takes >24 h.
