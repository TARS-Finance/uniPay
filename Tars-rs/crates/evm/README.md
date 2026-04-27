# uniPay EVM

A Rust library for interacting with EVM-compatible blockchains for atomic swaps using Hash Time Locked Contracts (HTLCs).

## Overview

The uniPay EVM subcrate provides high-level abstractions and utilities for interacting with Ethereum-based blockchains. It focuses on atomic swap functionality through Hash Time Locked Contracts (HTLCs), ERC20 token operations, and transaction batching with multicall support.

## Features

- **HTLC Operations**: Initiate, redeem, and refund atomic swaps
- **Multicall Support**: Batch multiple contract calls into single transactions
- **ERC20 Integration**: Work with ERC20 tokens in swap operations
- **Order ID Generation**: Deterministic order ID calculation for swaps
- **Transaction Simulation**: Dry-run transactions before sending
- **EIP-712 Signing**: Support for typed data signing

## Installation

Add the EVM module to your project by enabling the `evm` feature in your `Cargo.toml`:

```toml
[dependencies]
unipay = { git = "https://github.com/catalogfi/unipay.rs", features = ["evm"] }
```

## Usage

### Creating a uniPay HTLC Instance

```rust
use unipay::evm::{uniPayHTLC, uniPayHTLCContract, ERC20Contract, Multicall3Contract};
use alloy::primitives::{Address, U256};
use std::str::FromStr;

async fn setup_htlc() -> eyre::Result<uniPayHTLC> {
    // Create contract instances with appropriate addresses
    let htlc_address = Address::from_str("0x...")?;
    let token_address = Address::from_str("0x...")?;
    let multicall_address = Address::from_str("0x...")?;
    
    // Initialize provider
    let provider = // Initialize your provider
    
    // Create contract instances
    let htlc_contract = uniPayHTLCContract::new(htlc_address, provider.clone());
    let erc20_contract = ERC20Contract::new(token_address, provider.clone());
    let multicall_contract = Multicall3Contract::new(multicall_address, provider);
    
    // Create HTLC instance
    let htlc = uniPayHTLC::new(htlc_contract, erc20_contract, multicall_contract);
    
    Ok(htlc)
}
```

### Initiating a Swap

```rust
use unipay::evm::htlc::uniPayHTLC;
use orderbook::primitives::EVMSwap;
use alloy::primitives::{Address, U256, FixedBytes};

async fn create_swap(htlc: &uniPayHTLC) -> eyre::Result<String> {
    let swap = EVMSwap {
        redeemer: Address::from_str("0x...")?,
        timelock: 10000, // Block height
        amount: U256::from(1000000000000000000u128), // 1 token with 18 decimals
        secret_hash: FixedBytes::from([/* 32 byte hash */]),
        initiator: Address::from_str("0x...")?,
    };
    
    // Initiate the swap
    let tx_hash = htlc.initiate(&swap).await?;
    
    Ok(tx_hash)
}
```

### Redeeming a Swap

```rust
use unipay::evm::htlc::uniPayHTLC;
use alloy::primitives::Bytes;

async fn redeem_swap(htlc: &uniPayHTLC, swap: &EVMSwap, secret: Vec<u8>) -> eyre::Result<String> {
    // Convert secret to bytes
    let secret_bytes = Bytes::from(secret);
    
    // Redeem the swap
    let tx_hash = htlc.redeem(swap, &secret_bytes).await?;
    
    Ok(tx_hash)
}
```

### Using Multicall for Batch Operations

```rust
use unipay::evm::{htlc::uniPayHTLC, primitives::{HTLCRequest, Method}};
use alloy::primitives::Bytes;

async fn batch_operations(htlc: &mut uniPayHTLC, swap1: EVMSwap, swap2: EVMSwap, secret: Vec<u8>) -> eyre::Result<String> {
    let requests = vec![
        HTLCRequest {
            method: Method::Initiate { 
                signature: Bytes::default() 
            },
            swap: swap1
        },
        HTLCRequest {
            method: Method::Redeem { 
                secret: Bytes::from(secret)
            },
            swap: swap2
        }
    ];
    
    // Execute multiple operations in a single transaction
    let tx_hash = htlc.multicall(requests).await?;
    
    Ok(tx_hash)
}
```

## Core Components

### uniPayHTLC

The main interface for HTLC operations. It provides methods for:

- Initiating new swaps
- Redeeming swaps with secrets
- Refunding expired swaps
- Executing multiple operations in one transaction
- Simulating transactions before sending

### Multicall

A utility for batching multiple contract calls into a single transaction:

- Support for calls with and without ETH value
- Transaction simulation before execution
- Error handling for failed batch operations

### Primitives

Common data structures and types:

- `HTLCRequest`: Represents a request to perform an HTLC operation
- `Method`: Enum for different HTLC operations (Initiate, Redeem, Refund)
- `AlloyProvider`: A wrapper around Alloy's provider

## Error Handling

The module provides a comprehensive error type (`HTLCError`) that covers various failure scenarios:

- Contract errors
- Failed transaction simulations
- Timelock constraints
- Multicall failures
- Transport errors

## Testing

The module includes test utilities for mocking contracts and simulating blockchain interactions.

```rust
use unipay::evm::test_utils::ethereum_provider;

#[tokio::test]
async fn test_htlc_operations() {
    let signer = PrivateKeySigner::random();
    fund(signer.address().to_string());

    let wallet = EthereumWallet::from(signer.clone());
    // Set up a ethereum localnet provider
    let eth_provider = ethereum_provider(Some(wallet));
    
    // Create contract instances with mock provider
    // ...
}