# Utils

A collection of common utilities for uniPay Rust applications.

## Overview

This utility crate provides reusable components for uniPay Rust applications, including:

- Hex string to bytes conversion
- SHA-256 hashing functionality
- Environment variable interpolation for configuration
- Webhook integration for logging and monitoring
- Secure random secret generation

## Usage

The `utils` crate is designed to be used as a sub-crate within the uniPay package:

```rust
use unipay::utils::{ToBytes, Hashable, WebhookLayer};
```

## Features

### Hex to Bytes Conversion

Convert hexadecimal strings to byte arrays using the `ToBytes` trait:

```rust
use unipay::utils::ToBytes;

fn example() -> eyre::Result<()> {
    let hex_string = "0xabcdef1234567890";
    let bytes = hex_string.hex_to_bytes()?;
    println!("Converted bytes: {:?}", bytes);
    Ok(())
}
```

### SHA-256 Hashing

Compute SHA-256 hashes using the `Hashable` trait:

```rust
use unipay::utils::Hashable;
use alloy::primitives::FixedBytes;

fn example() -> eyre::Result<()> {
    let input = "Hello, world!";
    let hash: FixedBytes<32> = input.sha256()?;
    println!("SHA-256 hash: {:?}", hash);
    Ok(())
}
```

### Secret Generation

Generate cryptographically secure random secrets:

```rust
use unipay::utils::gen_secret;

fn example() {
    let (secret, hash) = gen_secret();
    assert_eq!(secret.len(), 32);
}
```

### Environment Variable Interpolation

Use environment variables in your configuration files:

```rust
use serde::Deserialize;
use unipay::utils::deserialize_env_field;

#[derive(Deserialize)]
struct Config {
    #[serde(deserialize_with = "unipay::utils::deserialize_env_field")]
    api_key: String,
}

// Config JSON: {"api_key": "#ENV:API_KEY"}
// Will pull value from API_KEY environment variable
```

### Webhook Logging

Send log events to a webhook for monitoring:

```rust
use tracing::Level;
use unipay::utils::setup_tracing_with_webhook;

fn main() -> eyre::Result<()> {
    // Setup tracing with webhook for ERROR level events
    // Limited to 5 messages per minute
    setup_tracing_with_webhook(
        "https://discord.com/api/webhooks/your/webhook/url",
        "MyuniPayApp",
        Level::ERROR,
        None
    )?;
    
    // Now ERROR events will be sent to Discord
    tracing::error!("Critical issue occurred: {}", "connection failed");
    
    Ok(())
}
```

#### Custom Message Formatting

You can provide a custom formatter for webhook messages:

```rust
use unipay::utils::{WebhookLayer, FieldVisitor};
use tracing::{Event, Level};

let custom_formatter = |event: &Event, visitor: &FieldVisitor, name: &str| {
    serde_json::json!({
        "content": format!("Alert from {}: {}", name, event.metadata().target()),
        "embeds": [{
            "title": "Custom Alert",
            "description": format!("Level: {}", event.metadata().level())
        }]
    })
};

let webhook_layer = WebhookLayer::new(
    "https://discord.com/api/webhooks/your/url",
    "CustomApp",
    Level::WARN,
    custom_formatter
)?;
```