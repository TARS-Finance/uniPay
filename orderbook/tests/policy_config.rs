use std::fs;

use tempfile::tempdir;
use tars_orderbook::config::policy::PolicySettings;

#[test]
fn parses_trimmed_policy_toml_and_preserves_bitcoin_split_identity() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("policy.toml");

    fs::write(
        &path,
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
overrides = []
max_limits = {}

[policy.default_fee]
fixed = 0.0
percent_bips = 30
"#,
    )
    .unwrap();

    let settings = PolicySettings::from_toml(path.to_str().unwrap()).unwrap();

    assert_eq!(settings.solver_id, "solver-1");
    assert_eq!(settings.policy.default_confirmation_target, 1);

    let bitcoin = settings.chains.get("bitcoin_testnet").unwrap();
    assert_eq!(bitcoin.liquidity_account(), "tb1qbalance");
    assert_eq!(bitcoin.order_identity(), "xonlypubkey");
}
