# EVM Executor

Counterparty for EVM swaps on Htlc swaps

## What it does

1. Get pending orders
2. Map orders to actions
3. Filter cached requests
4. Execute dry run
5. Execute multicall
6. Wait for transaction
7. Update cache
8. Sleep

## Configuration

Create `Settings.toml` in the root directory:

```toml
pending_orders_url = "http://0.0.0.0:4596"
fiat_provider_url = "http://0.0.0.0:6969"
discord_webhook_url = "https://discord.com/api/webhooks/your_webhook_url"
private_key = "your_private_key_here"

[[chains]]
chain_identifier = "ethereum_localnet"
rpc_url = "http://localhost:8545"
multicall_address = "0x2279B7A0a67DB372996a5FaB50D91eAA73d2eBe6"
polling_interval = 5000
transaction_timeout = 60000

[[chains]]
chain_identifier = "arbitrum_localnet"
rpc_url = "http://localhost:8546"
multicall_address = "0xA51c1fc2f0D1a1b8494Ed1FE312d7C3a78Ed91C0"
polling_interval = 5000
transaction_timeout = 60000
```

## Configuration Options

-   `pending_orders_url`: API endpoint for fetching orders
-   `fiat_provider_url`: Fiat price provider endpoint
-   `chain_identifier`: Unique name for the chain
-   `rpc_url`: Blockchain RPC endpoint
-   `multicall_address`: Multicall contract address
-   `polling_interval`: Time between checks (milliseconds)
-   `private_key`: Wallet private key for transactions
-   `transaction_timeout`: Max wait time for confirmations (milliseconds)

## Local Development

For local development with merry (multichain localnet):

1. Clone the repository:

    ```bash
    git clone https://github.com/catalogfi/evm-executor.git
    ```

2. Navigate to directory:

    ```bash
    cd evm-executor
    ```

3. Copy local settings:

    ```bash
    cp local.settings.toml Settings.toml
    ```

4. Run the executor:
    ```bash
    cargo run
    ```

**Note**: Make sure merry is running before starting the executor.

## Security

⚠️ **Keep private keys secure and never commit them to version control**
