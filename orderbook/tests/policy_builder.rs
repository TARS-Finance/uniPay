use std::fs;

use tempfile::tempdir;
use tars_orderbook::{
    config::policy::PolicySettings, metadata::MetadataIndex,
    registry::policy_builder::build_strategy_configs,
};

#[test]
fn builds_directed_cross_chain_strategies_from_local_policy() {
    let dir = tempdir().unwrap();
    let policy_path = dir.path().join("policy.toml");
    let chain_path = dir.path().join("chain.json");

    fs::write(
        &policy_path,
        r#"
solver_id = "solver-1"
solver_name = "local solver"

[chains.base_sepolia]
rpc_url = "https://base"
native_decimals = 18
native_asset_id = "base_sepolia:eth"
address = "0xbase"
supported_assets = ["base_sepolia:usdc"]

[chains.arbitrum_sepolia]
rpc_url = "https://arb"
native_decimals = 18
native_asset_id = "arbitrum_sepolia:eth"
address = "0xarb"
supported_assets = ["arbitrum_sepolia:usdc"]

[chains.bitcoin_testnet]
rpc_url = "https://btc"
native_decimals = 8
native_asset_id = "bitcoin_testnet:btc"
address = "tb1qbalance"
solver_account = "xonlypubkey"
supported_assets = ["bitcoin_testnet:btc"]

[policy]
solver_id = "solver-1"
default = "open"
isolation_groups = []
blacklist_pairs = []
whitelist_overrides = []
default_max_slippage = 0
default_confirmation_target = 1
max_limits = {}

[policy.default_fee]
fixed = 0.0
percent_bips = 30

[[policy.overrides]]
route = "base_sepolia:usdc -> bitcoin_testnet:btc"
max_slippage = 15
confirmation_target = 2

[policy.overrides.source_amount]
min = "2000000"
max = "9000000"
"#,
    )
    .unwrap();

    fs::write(
        &chain_path,
        serde_json::json!([
            {
                "chain": "base_sepolia",
                "id": "evm:84532",
                "icon": "icon",
                "explorer_url": "explorer",
                "confirmation_target": 1,
                "source_timelock": "3600",
                "destination_timelock": "3600",
                "supported_htlc_schemas": ["evm:htlc_erc20"],
                "supported_token_schemas": ["evm:erc20"],
                "assets": [
                    {
                        "id": "base_sepolia:usdc",
                        "name": "USD Coin:USDC",
                        "chain": "base_sepolia",
                        "icon": "icon",
                        "htlc": { "address": "0xbasehtlc", "schema": "evm:htlc_erc20" },
                        "token": { "address": "0xbasetoken", "schema": "evm:erc20" },
                        "decimals": 6,
                        "min_amount": "1000000",
                        "max_amount": "10000000",
                        "chain_icon": "icon",
                        "chain_id": "84532",
                        "chain_type": "evm",
                        "version": "v3",
                        "explorer_url": "explorer",
                        "min_timelock": 3600,
                        "token_ids": { "coingecko": "usd-coin", "aggregate": "USDC", "cmc": "3408" },
                        "solver": "solver"
                    }
                ]
            },
            {
                "chain": "arbitrum_sepolia",
                "id": "evm:421614",
                "icon": "icon",
                "explorer_url": "explorer",
                "confirmation_target": 1,
                "source_timelock": "36000",
                "destination_timelock": "36000",
                "supported_htlc_schemas": ["evm:htlc_erc20"],
                "supported_token_schemas": ["evm:erc20"],
                "assets": [
                    {
                        "id": "arbitrum_sepolia:usdc",
                        "name": "USD Coin:USDC",
                        "chain": "arbitrum_sepolia",
                        "icon": "icon",
                        "htlc": { "address": "0xarbhtlc", "schema": "evm:htlc_erc20" },
                        "token": { "address": "0xarbtoken", "schema": "evm:erc20" },
                        "decimals": 6,
                        "min_amount": "1000000",
                        "max_amount": "10000000",
                        "chain_icon": "icon",
                        "chain_id": "421614",
                        "chain_type": "evm",
                        "version": "v3",
                        "explorer_url": "explorer",
                        "min_timelock": 36000,
                        "token_ids": { "coingecko": "usd-coin", "aggregate": "USDC", "cmc": "3408" },
                        "solver": "solver"
                    }
                ]
            },
            {
                "chain": "bitcoin_testnet",
                "id": "bitcoin",
                "icon": "icon",
                "explorer_url": "explorer",
                "confirmation_target": 1,
                "source_timelock": "12",
                "destination_timelock": "12",
                "supported_htlc_schemas": [],
                "supported_token_schemas": [],
                "assets": [
                    {
                        "id": "bitcoin_testnet:btc",
                        "name": "Bitcoin:BTC",
                        "chain": "bitcoin_testnet",
                        "icon": "icon",
                        "htlc": { "address": "primary", "schema": "primary" },
                        "token": { "address": "primary", "schema": "primary" },
                        "decimals": 8,
                        "min_amount": "50000",
                        "max_amount": "1000000",
                        "chain_icon": "icon",
                        "chain_id": null,
                        "chain_type": "bitcoin",
                        "version": "v3",
                        "explorer_url": "explorer",
                        "min_timelock": 12,
                        "token_ids": { "coingecko": "bitcoin", "aggregate": "BTC", "cmc": "1" },
                        "solver": "solver"
                    }
                ]
            }
        ])
        .to_string(),
    )
    .unwrap();

    let policy = PolicySettings::from_toml(policy_path.to_str().unwrap()).unwrap();
    let metadata = MetadataIndex::load(chain_path.to_str().unwrap()).unwrap();

    let strategies = build_strategy_configs(&policy, &metadata).unwrap();

    assert_eq!(strategies.len(), 6);
    assert!(strategies.iter().any(|s| s.source_chain == "base_sepolia" && s.dest_chain == "arbitrum_sepolia"));
    assert!(strategies.iter().any(|s| s.source_chain == "base_sepolia" && s.dest_chain == "bitcoin_testnet"));
    assert!(strategies.iter().any(|s| s.source_chain == "bitcoin_testnet" && s.dest_chain == "base_sepolia"));
    assert!(!strategies.iter().any(|s| s.source_chain == s.dest_chain));

    let base_to_btc = strategies
        .iter()
        .find(|s| s.source_chain == "base_sepolia" && s.dest_chain == "bitcoin_testnet")
        .unwrap();
    assert_eq!(base_to_btc.max_slippage, 15);
    assert_eq!(base_to_btc.min_source_confirmations, 2);
    assert_eq!(base_to_btc.min_amount.to_string(), "2000000");
    assert_eq!(base_to_btc.max_amount.to_string(), "9000000");

    let btc_to_base = strategies
        .iter()
        .find(|s| s.source_chain == "bitcoin_testnet" && s.dest_chain == "base_sepolia")
        .unwrap();
    assert_eq!(btc_to_base.source_chain_address, "xonlypubkey");
    assert_eq!(btc_to_base.dest_chain_address, "0xbase");
}
