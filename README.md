# uniPay

Consolidated monorepo for the uniPay stack: frontend, Initia rollup tooling, merchant automation, quoting, and chain-specific watcher/executor services.

## Initia Hackathon Submission

- **Project Name**: uniPay

### Project Overview

uniPay is a universal checkout and settlement system built around an Initia rollup. Customers can pay from multiple source ecosystems, while merchants settle into an Initia-native flow with a dedicated frontend, rollup bridge tooling, and backend services for routing, execution, and merchant yield operations.

The repo combines the full service stack in one place so the rollup, frontend, order routing, and chain adapters can be reviewed together. The local demo centers on the Initia rollup flow in `initia-rollup` and the hackathon frontend in `initia-fe`.

### Implementation Detail

- **The Custom Implementation**: uniPay is not a single dApp scaffold. It combines a dedicated Initia EVM rollup launcher, bridge seeding and HTLC deployment scripts, a customer/merchant frontend, merchant earning flows in `stacker`, quote and order routing in `orderbook` and `solver-orders`, and per-chain watcher/executor services for Bitcoin, EVM, Solana, and Initia settlement paths.
- **The Native Feature**: `interwoven-bridge`. The frontend uses `InterwovenKit` for wallet connection and Initia transaction flows, and the merchant experience is wired to Initia bridge actions so users can move value between the rollup and Initia L1 with the expected Initia UX.

### How to Run Locally

1. Install dependencies for the demo surfaces you need: `cd initia-rollup && npm install`, `cd ../initia-fe && npm install`. If you want the merchant API and earning flows live too, also run `cd ../stacker && pnpm install`.
2. Copy `initia-rollup/.env.example` to `initia-rollup/.env`, then fill the small required set: `MERCHANT_PRIVATE_KEY`, `MERCHANT_INIT_ADDRESS`, `MERCHANT_HEX_ADDRESS`, and `TARGET_POOL_ID`. After that, run `cd initia-rollup && npm run preflight`.
3. From `initia-rollup`, run `npm run launch`, `npm run seed-bridge`, and `npm run redeploy-htlc`. These scripts launch the local Initia rollup, seed bridged USDC, deploy the HTLC contract, and write the needed `VITE_*` values into `initia-fe/.env.local`.
4. Start the frontend with `cd initia-fe && npm run dev`, then open the printed Vite URL in a browser. If you want the merchant earn flows instead of frontend-only UI work, start the supporting backend services as well, especially `stacker`, and point `VITE_STACKER_API_URL` at the running API.

## System Flow

The system is split into three layers:

- `initia-fe` handles checkout, merchant UX, wallet connection, and Interwoven bridge flows.
- `orderbook` is the source of truth for quotes, matched orders, and source/destination swap state.
- Watchers observe chain state, while executors act on chain state to lock, redeem, refund, and finalize cross-chain HTLC legs.

### 1. Platform Architecture

```mermaid
flowchart TD
    User["Customer / Merchant"] --> FE["initia-fe<br/>checkout, invoices, merchant dashboard"]
    FE --> OB["orderbook<br/>quotes, routing, matched source/destination swaps"]
    OB --> SO["solver-orders<br/>pending intent cache for solvers and executors"]

    subgraph ChainServices["Chain-specific watchers and executors"]
        BW["btc-watcher"]
        BE["btc-executor"]
        EW["evm-watcher"]
        EE["evm-executor"]
        SW["solana-watcher"]
        SE["solana-executor"]
        IW["initia-watcher"]
        IE["initia-executor"]
    end

    OB <--> BW
    OB <--> EW
    OB <--> SW
    OB <--> IW

    SO --> BE
    SO --> EE
    SO --> SE
    SO --> IE

    BE --> OB
    EE --> OB
    SE --> OB
    IE --> OB

    subgraph InitiaSettlement["Initia settlement surface"]
        Rollup["initia-rollup<br/>Universal Pay appchain + HTLC contracts"]
        Bridge["Interwoven bridge"]
        L1["Initia L1"]
        Stacker["stacker<br/>merchant balances, staking, withdrawals"]
    end

    FE <--> Bridge
    Bridge <--> Rollup
    Bridge <--> L1

    IW <--> Rollup
    IE <--> Rollup

    FE <--> Stacker
    Stacker <--> Rollup
    Stacker <--> L1
```

### 2. Example Trade: Bitcoin -> Initia

```mermaid
sequenceDiagram
    autonumber
    participant U as Customer
    participant FE as initia-fe
    participant OB as orderbook
    participant SO as solver-orders
    participant BTC as Bitcoin network
    participant BW as btc-watcher
    participant IE as initia-executor
    participant IR as Initia rollup
    participant BE as btc-executor
    participant M as Merchant / receiver
    participant ST as stacker

    U->>FE: Choose BTC as source asset and Initia as destination
    FE->>FE: Generate secret + secretHash locally
    FE->>OB: Create order(from=BTC, to=Initia asset, secretHash)
    OB->>OB: Quote best route and persist matched source_swap + destination_swap
    OB-->>FE: Return orderId, BTC HTLC deposit address, destination terms
    OB-->>SO: Publish pending intent for solver / executor consumption

    U->>BTC: Send BTC to the source HTLC deposit address
    BW->>BTC: Watch mempool and blocks for HTLC funding
    BW->>OB: Record source initiate_tx_hash and confirmations

    IE->>SO: Pull pending Initia destination orders
    IE->>IR: Lock destination-side liquidity in the Initia HTLC
    IE->>OB: Persist destination initiate_tx_hash

    FE->>OB: Poll order status
    FE->>IE: POST /secret once destination leg is locked
    IE->>IR: Redeem the Initia destination HTLC with the revealed secret
    IE->>OB: Persist destination redeem_tx_hash and secret

    BE->>OB: Read the now-revealed secret from the fulfilled destination leg
    BE->>BTC: Redeem the source BTC HTLC using the same secret
    BE->>OB: Persist source redeem_tx_hash and mark order fulfilled

    IR-->>M: Merchant receives the Initia-side asset

    alt Merchant keeps funds on the rollup
        M-->>M: Hold balance and continue accepting payments
    else Merchant uses earn / withdrawal flows
        M->>ST: Open merchant earn or withdrawal flow
        ST->>IR: Use settled rollup funds for staking / pool operations
    end
```

### Repository Layout

| Path | Purpose |
| --- | --- |
| `initia-fe` | Hackathon frontend with customer and merchant flows, `InterwovenKit`, and Initia wallet/bridge integration |
| `initia-rollup` | Local Initia rollup launcher, bridge seeding, HTLC deployment, and demo orchestration scripts |
| `stacker` | Merchant automation backend for positions, rewards, withdrawals, and keeper jobs |
| `orderbook` | Quote, policy, pricing, and routing logic |
| `solver-orders` | Solver-facing order service |
| `btc-watcher` / `btc-executor` | Bitcoin source-chain detection and execution |
| `evm-watcher` / `evm-executor` | EVM source-chain detection and execution |
| `solana-watcher` / `solana-executor` | Solana source-chain detection and execution |
| `initia-watcher` / `initia-executor` | Initia-side monitoring and execution |

### Notes For Reviewers

- This repository is the single consolidated repo for the latest committed `HEAD` of every service that currently powers uniPay.
- The Initia-specific demo path starts in `initia-rollup` and `initia-fe`; the other services remain in-repo so the full architecture and chain adapters can be reviewed together.
