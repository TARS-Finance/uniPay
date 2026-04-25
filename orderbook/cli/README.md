# BTC/EVM CLI Coordinator

This `cli/` directory contains two command paths:

- RFQ flow: existing `npm run start` behavior (legacy `/rfq` + `/trade`)
- Orderbook flow: new `npm run orderbook-quote` flow (`/quote` + `/orders`)

Use the orderbook flow when you want quote and order creation from this repository’s unified API.

## Quick start

1. Install dependencies:

```bash
cd /Users/diwakarmatsaa/Desktop/Tars/orderbook/cli
npm install
```

2. Copy and edit the example config:

```bash
cp config.example.json config.json
```

3. Fill chain endpoints, private keys, and addresses:

- `api_url`: Munger API base URL
- chain RPC / Esplora URLs
- `refund_address` / `destination_address` for quote accept payload
- source and destination chain entries in `chains`

4. Run RFQ flow:

```bash
npm run start -- --config ./config.json
```

You can override RFQ inputs inline:

```bash
npm run start -- --config ./config.json --source base_sepolia:usdc --target bitcoin_testnet:btc --amount 1000000 --slippage 25
```

5. Run orderbook flow:

```bash
npm run orderbook-quote -- --config ./config.json
```

Orderbook options:

```bash
npm run orderbook-quote -- --config ./config.json \
  --source base_sepolia:usdc \
  --target bitcoin_testnet:btc \
  --amount 1000000 \
  --slippage 25 \
  --watch \
  --poll-timeout-ms 180000
```

Useful flags:

- `--amount`: source amount (defaults to `trade.source_amount`)
- `--to-amount`: request exact-out quotes instead of exact-in
- `--strategy-id`: force a specific strategy from the returned route list
- `--affiliate-fee`: pass affiliate fee bps
- `--source-recipient` / `--destination-recipient`: override initiator addresses
- `--no-create`: print quote only without creating an order
- `--watch`: poll `/orders/:id` until destination initiate tx hash appears
- `--secret-hash`: supply your own 64-char hex secret hash

## Notes

- `npm run start` and `npm run orderbook-quote` both use `./cli/config.json` as default.
- Use `--config` to point at a different file path.
