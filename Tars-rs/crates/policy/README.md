# uniPay Policy

A flexible, thread-safe policy management library for handling trading rules, fees, and asset pair validation across multiple solvers.

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
policy = { path = "path/to/policy" }
primitives = { path = "path/to/primitives" }
```

## Quick Start

### Basic Policy Usage

```rust
use policy::Policy;
use primitives::AssetId;
use std::str::FromStr;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize policy from solver aggregator (default 30s refresh interval)
    let policy = Policy::new("http://localhost:4466", None).await?;

    // Get all solvers that support a specific trading pair
    let source = AssetId::from_str("ethereum:usdc")?;
    let destination = AssetId::from_str("bitcoin:btc")?;

    let solvers = policy.get_solvers_for_pair(&source, &destination);
    for (solver_id, fee) in solvers {
        println!("Solver {}: Fixed fee {}, Percentage fee {} bips",
                 solver_id, fee.fixed, fee.percent_bips);
    }

    // Validate a route for a specific solver
    let fee = policy.validate_route_and_get_fee(
        &source,
        &destination,
        "solver-1"
    )?;

    println!("Trade allowed with fee: fixed={}, percent_bips={}",
             fee.fixed, fee.percent_bips);

    Ok(())
}
```

### Background Policy Refresh

The `Policy` struct supports automatic background refresh to keep policies synchronized with the solver aggregator:

```rust
use policy::Policy;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create policy with custom 60-second refresh interval
    let mut policy = Policy::new("http://localhost:4466", Some(60)).await?;

    // Clone for use in application
    let policy_ref = policy.clone();

    // Spawn background refresh loop
    tokio::spawn(async move {
        policy.start().await;
    });

    // Use policy_ref in your application
    // Policy updates happen automatically in the background

    Ok(())
}
```

**Note**: Policy updates use `ArcSwap` for lock-free atomic operations, ensuring zero contention between readers and the refresh task. This means:

- Read operations are never blocked during updates
- All readers see a consistent snapshot of policies
- Updates are performed atomically

## Core Components

### Policy

The `Policy` struct provides centralized management for multiple solvers:

**Key Methods:**

- `new(solver_aggregator_url, refresh_interval_secs)` - Initialize from solver aggregator
- `start()` - Run background refresh loop
- `get_solvers_for_pair(source, destination)` - Get all supporting solvers with fees
- `validate_route_and_get_fee(source, destination, solver_id)` - Validate specific solver route
- `solvers()` - Access all solver policies
- `solver_aggregator_url()` - Get the solver aggregator URL

### SolverPolicy

The `SolverPolicy` struct enforces trading rules for a single solver:

**Key Methods:**

- `new(config, supported_assets)` - Create from configuration
- `validate_and_get_fee(source, destination)` - Validate and get fee
- `validate_asset_pair(source, destination)` - Validate only
- `get_fee(source, destination)` - Get fee without validation
- `is_asset_supported(asset)` - Check asset support
- `supported_assets()` - Get supported assets set
- `default_fee()` - Get default fee

## Working with SolverPolicy Directly

For fine-grained control over a single solver's policy:

```rust
use policy::{SolverPolicy, SolverPolicyConfig, DefaultPolicy, Fee};
use primitives::AssetId;
use std::str::FromStr;

fn example() -> Result<(), Box<dyn std::error::Error>> {
    // Create a solver policy configuration
    let config = SolverPolicyConfig {
        default: DefaultPolicy::Open,
        isolation_groups: vec![
            "arbitrum:seed <-> ethereum:seed".to_string(),
        ],
        blacklist_pairs: vec![
            "starknet:* -> arbitrum:*".to_string(),
        ],
        whitelist_overrides: vec![
            "starknet:wbtc -> arbitrum:wbtc".to_string(),
        ],
        default_fee: Fee {
            fixed: 10.0,
            percent_bips: 50,
        },
        overrides: vec![],
    };

    // Specify supported assets
    let supported_assets = vec![
        "ethereum:usdc".to_string(),
        "bitcoin:btc".to_string(),
        "arbitrum:seed".to_string(),
        "ethereum:seed".to_string(),
        "starknet:wbtc".to_string(),
        "arbitrum:wbtc".to_string(),
    ];

    // Create the solver policy
    let solver_policy = SolverPolicy::new(config, supported_assets)?;

    // Validate a trade and get the fee
    let source = AssetId::from_str("ethereum:usdc")?;
    let destination = AssetId::from_str("bitcoin:btc")?;

    let fee = solver_policy.validate_and_get_fee(&source, &destination)?;
    println!("Fee: fixed={}, percent_bips={}", fee.fixed, fee.percent_bips);

    Ok(())
}
```

## Policy Rules

The `policy` crate supports three types of rules that govern trading:

### 1. Isolation Groups

Isolation groups restrict which assets can trade with each other. When an asset has isolation rules defined, it can **only** trade with assets specified in those rules.

**Format:**

- Forward: `"source -> destination"`
- Bidirectional: `"asset1 <-> asset2"`

**Examples:**

```rust
let isolation_groups = vec![
    "arbitrum:seed <-> ethereum:seed",  // Bidirectional: seed tokens isolated to each other
    "starknet:* -> bitcoin:btc",        // Any starknet token can go to bitcoin BTC
    "starknet:usdc -> *:usdc",          // Starknet USDC can go to USDC on any chain
    "starknet:wbtc -> arbitrum:wbtc",   // Specific: starknet wbtc to arbitrum wbtc only
];
```

**Behavior:**

- If an asset has isolation rules, it can **only** trade with the specified assets
- More specific rules override less specific wildcard rules
- Assets without isolation rules can trade freely (subject to other restrictions)
- The first matching rule (most specific) determines allowed destinations

### 2. Blacklist Pairs

Blacklist rules explicitly block specific asset pairs from trading.

**Examples:**

```rust
let blacklist_pairs = vec![
    "starknet:* -> arbitrum:*",         // Block all starknet to arbitrum trades
    "starknet:stark <-> solana:*",      // Bidirectional block
];
```

**Behavior:**

- Blacklisted pairs are blocked from trading
- Can be overridden by whitelist overrides
- Supports wildcards for flexible matching

### 3. Whitelist Overrides

Whitelist overrides allow specific pairs that would otherwise be blacklisted. These have the highest precedence.

**Examples:**

```rust
let whitelist_overrides = vec![
    "solana:usdc -> starknet:stark",    // Allow this specific trade
    "starknet:* <-> solana:wbtc",       // Allow starknet-solana wbtc trades
];
```

**Behavior:**

- Whitelist overrides take precedence over blacklist rules
- Enable exceptions to blacklist restrictions
- Support wildcards for pattern-based overrides

## Wildcard Support

Rules support wildcards (`*`) for both chain and token fields, enabling flexible policy definitions:

```rust
// Wildcard examples
"ethereum:* -> bitcoin:btc"      // Any Ethereum token to Bitcoin BTC
"*:usdc -> *:usdc"               // Any USDC to any USDC
"starknet:* -> arbitrum:*"       // Any starknet token to any arbitrum token
```

### Wildcard Precedence

When multiple rules match, more specific rules take precedence:

1. **Exact match** (both chain and token specified): `starknet:wbtc -> arbitrum:wbtc`
2. **Single wildcard**: `starknet:* -> arbitrum:wbtc` or `starknet:wbtc -> arbitrum:*`
3. **Double wildcard**: `starknet:* -> arbitrum:*`
4. **Full wildcard**: `*:usdc -> *:usdc`

**Example:**

```rust
// Given these isolation rules (automatically sorted by specificity):
let rules = vec![
    "starknet:* -> bitcoin:btc",          // Less specific
    "starknet:usdc -> *:usdc",            // More specific for usdc
    "starknet:wbtc -> arbitrum:wbtc",     // Most specific
];

// Results:
// starknet:wbtc -> arbitrum:wbtc ✓ (matches most specific rule)
// starknet:wbtc -> bitcoin:btc   ✗ (blocked by specific rule)
// starknet:usdc -> ethereum:usdc ✓ (matches usdc wildcard rule)
// starknet:eth -> bitcoin:btc    ✓ (matches wildcard rule)
```

## Rule Precedence

The policy system follows a clear validation order:

1. **Asset Support**: Both assets must be in the solver's supported assets list
2. **Same Asset Check**: Source and destination must be different
3. **Isolation Rules**: If the source asset has isolation rules, the destination must be allowed
4. **Blacklist Check**: The pair must not be blacklisted
5. **Whitelist Override**: Whitelist overrides can allow blacklisted pairs

**Precedence Hierarchy:**

```
Whitelist Override (highest)
    ↓
Specific Rules (by asset specificity)
    ↓
Blacklist Rules
    ↓
Isolation Rules
    ↓
Asset Support
    ↓
Same Asset Check (lowest)
```

**Example:**

```rust
// Configuration:
// - Supported: ["starknet:wbtc", "arbitrum:wbtc"]
// - Blacklist: "starknet:* -> arbitrum:*"
// - Whitelist: "starknet:wbtc -> arbitrum:wbtc"
// - Isolation: None

// Result: starknet:wbtc -> arbitrum:wbtc is ALLOWED
// Whitelist override takes precedence over blacklist
```

## Fee Structures

### Fee Definition

Fees consist of two components:

```rust
pub struct Fee {
    pub fixed: f64,           // Fixed fee amount in USD
    pub percent_bips: u32,    // Percentage fee in basis points (1 bips = 0.01%)
}
```

**Example:**

```rust
let fee = Fee {
    fixed: 10.0,           // Fixed fee of $10.00
    percent_bips: 50,      // 0.5% percentage fee (50 basis points)
};

// Also supports decimal values
let small_fee = Fee {
    fixed: 0.20,           // Fixed fee of $0.20
    percent_bips: 10,      // 0.1% percentage fee
};
```

### Default Fee

Every solver has a default fee that applies to all routes unless overridden:

```rust
let default_fee = Fee {
    fixed: 10.0,           // $10.00 in USD
    percent_bips: 50,      // 0.5%
};
```

### Fee Overrides

Route-specific fees can override the default fee:

```rust
use policy::FeeOverride;

let overrides = vec![
    FeeOverride {
        route: "bitcoin:btc -> ethereum:wbtc".to_string(),
        fee: Fee {
            fixed: 20.0,           // $20.00 in USD
            percent_bips: 75,      // 0.75%
        },
    },
    FeeOverride {
        route: "ethereum:eth <-> starknet:eth".to_string(),  // Bidirectional
        fee: Fee {
            fixed: 5.0,            // $5.00 in USD
            percent_bips: 25,      // 0.25%
        },
    },
];
```

### Retrieving Fees

```rust
// Get fee for a specific route (with validation)
let fee = solver_policy.validate_and_get_fee(&source, &destination)?;

// Get fee without validation
let fee = solver_policy.get_fee(&source, &destination);

// Get default fee
let default = solver_policy.default_fee();
```
