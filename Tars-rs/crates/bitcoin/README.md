# Bitcoin

Contains common utilities and helper functions for Bitcoin operations in the unipay

## Available Functions

### Core Utility Operations

- `get_bitcoin_network(chain: &str) -> Result<Network>`

  - Returns Bitcoin network type (mainnet, testnet, regtest) for given chain name

- `validate_btc_address_for_network(addr: &str, network: Network) -> Result<Address>`

  - Validates a Bitcoin address for a specific network
  - Ensures address format is valid and matches the specified network

- `generate_instant_refund_hash(htlc_params: &HTLCParams, utxos: &[Utxo], recipient: &Address, network: Network, fee: Option<u64>) -> Result<Vec<String>>`

  - Generates hashes for instant refund operations

- `validate_schnorr_signature(verifying_key: &XOnlyPublicKey, signature: &[u8], message_hash: &[u8], hash_type: TapSighashType) -> Result<()>`

  - Validates Schnorr signature against message and public key

- `validate_instant_refund_sacp_tx(instant_refund_sacp_hex: &str, initiate_tx_hash: &str, htlc_params: &HTLCParams, utxos: &[Utxo], recipient: &Address, network: Network) -> Result<()>`

  - Verifies a Bitcoin SACP (Single Plus Anyone Can Pay) instant refund transaction
  - Checks input count, Schnorr signatures, and that at least one input references the expected initiate transaction hash

### Fee Estimation

The crate provides multiple fee estimation providers through the `FeeRateEstimator` trait:

- `FeeRateEstimator` trait
  - `get_fee_estimates() -> Result<FeeEstimate>` - Fetches current fee rate estimates
  - `name() -> &str` - Returns the name of the fee rate estimator

Available implementations:

- Blockstream fee estimator
- Mempool fee estimator
- Fixed fee estimator
- Multi-provider fee estimator (combines multiple providers)

### HTLC Operations

- `get_htlc_address(htlc_params: &HTLCParams, network: Network) -> Result<Address>`

  - Generates HTLC address from swap information

- `refund(htlc_params: &HTLCParams, recipient: &Address) -> Result<String>`

  - Refunds HTLC funds to the recipient
  - Returns the transaction ID

- `build_instant_refund_sacp(htlc_params: &HTLCParams, utxos: &[Utxo], signatures: Vec<InstantRefundSignatures>, recipient: &Address, fee: Option<u64>) -> Result<Transaction>`

  - Builds instant refund SACP transaction with provided UTXOs and signatures

### Transaction Utilities

- `sort_utxos(utxos: &[Utxo]) -> Vec<Utxo>`

  - Sorts UTXOs by txid and vout for deterministic transaction structure

- `create_inputs_from_utxos(utxos: &[Utxo]) -> Vec<TxIn>`

  - Creates unsigned transaction inputs from UTXOs with empty signatures

- `create_previous_outputs(utxos: &[Utxo], htlc_address: &Address) -> Vec<TxOut>`

  - Creates transaction outputs matching original HTLC outputs

- `create_transaction_from_utxos(utxos: &[Utxo], recipient: &Address, sighash_type: TapSighashType, sequence: Sequence, fee: Option<u64>) -> Result<Transaction>`
  - Builds unsigned transaction with input-output mapping determined by the provided sighash type
  - Allows customization of sighash type and sequence number for transaction inputs
  - Supports optional fee parameter for output amount calculation

### Script Operations

- `redeem_leaf(secret_hash: &[u8; 32], redeemer_pubkey: &XOnlyPublicKey) -> ScriptBuf`

  - Creates Bitcoin script for spending with secret preimage and redeemer's signature

- `refund_leaf(timelock: u64, initiator_pubkey: &XOnlyPublicKey) -> ScriptBuf`

  - Creates Bitcoin script for refunding after timelock expires

- `instant_refund_leaf(initiator_pubkey: &XOnlyPublicKey, redeemer_pubkey: &XOnlyPublicKey) -> ScriptBuf`
  - Creates Bitcoin script requiring both signatures for instant refund

### Indexer Operations

- `get_tx_hex(txid: &str) -> Result<Transaction>`

  - Retrieves a transaction by its transaction ID and returns it as a Transaction object

- `get_tx(txid: &str) -> Result<TransactionMetadata>`

  - Retrieves detailed transaction information including fee, inputs, and outputs

- `get_block_height() -> Result<u64>`

  - Fetches current block height from the network

- `submit_tx(tx: &Transaction) -> Result<()>`

  - Submits transaction to the Bitcoin network

- `get_utxos(address: &Address) -> Result<Vec<Utxo>>`

  - Retrieves unspent transaction outputs for given address

- `get_tx_outspends(txid: &str) -> Result<Vec<OutSpends>>`
  - Retrieves spending status of all outputs for a given transaction
  - Returns information about whether outputs have been spent and by which transactions

### Batcher Operations

The crate includes a transaction batching system that efficiently handles multiple Bitcoin transactions in a single batch. The main components are:

- `BitcoinTxBatcher` - The main batch processor that handles transaction building, signing, and submission

Key features:

- Batch transaction building

  - Combines multiple spend requests into a single transaction
  - Optimizes fee usage by sharing transaction overhead

- Transaction validation

  - Validates spend requests against network state
  - Ensures UTXOs are available and unspent

- Fee management

  - Uses configurable fee levels (e.g., conservative, normal, aggressive)
  - Integrates with fee rate estimators for dynamic fee adjustment

- Transaction signing
  - Handles Schnorr signatures for taproot outputs
  - Supports witness data for segwit transactions

Usage example:

```rust
use unipay::bitcoin::{
    batcher::{BitcoinTxBatcher, SpendRequest},
    ArcIndexer, ArcFeeRateEstimator, FeeLevel,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize batcher
    let indexer = ArcIndexer::new("http://localhost:3000");
    let fee_estimator = ArcFeeRateEstimator::new();

    let batcher = BitcoinTxBatcher::new(
        indexer,
        FeeLevel::Normal,
        fee_estimator
    );

    // Create spend requests
    let requests = vec![
        SpendRequest {
            id: "tx1".to_string(),
            utxos: vec![/* UTXOs here */],
            witness: /* Witness data here */,
            keypair: /* Keypair for signing */,
            recipient: /* Recipient address */,
            script: /* Script data */,
            htlc_address: /* HTLC address if applicable */,
        },
        // Add more spend requests as needed
    ];

    // Execute batch transaction
    let txid = batcher.execute(requests).await?;
}
```

### Merry Operations

- `fund_btc(address: &Address, indexer: &dyn Indexer) -> Result<()>`
  - Funds a Bitcoin address using merry faucet and waits for confirmation

## Type References

For detailed type definitions, refer to:

- `src/indexer/primitives.rs` - Indexer related types
- `src/htlc/primitives.rs` - HTLC related types
- `src/fee_providers/primitives.rs` - Fee estimation related types

## Usage

```rust
use unipay::bitcoin::{
    BitcoinIndexerClient,
    network::{get_bitcoin_network, validate_btc_address_for_network},
    htlc::{get_htlc_address, build_instant_refund_sacp},
    fee_providers::{FeeRateEstimator, BlockstreamFeeEstimator},
};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize network and indexer
    let network = get_bitcoin_network("bitcoin_testnet")?;
    let indexer = BitcoinIndexerClient::new("http://localhost:3000", None)?;

    // Validate address
    let address = validate_btc_address_for_network("tb1...", network)?;

    // Get fee estimates
    let fee_estimator = BlockstreamFeeEstimator::new();
    let fee_estimates = fee_estimator.get_fee_estimates().await?;

    // Get transaction details
    let tx = indexer.get_tx("txid").await?;

    // Get HTLC address
    let htlc_address = get_htlc_address(&htlc_params, network)?;

    // Build and submit instant refund transaction
    let refund_tx = build_instant_refund_sacp(
        utxos,
        &htlc_params,
        signatures,
        recipient,
        Some(fee)
    ).await?;
    indexer.submit_tx(&refund_tx).await?;
}
```
