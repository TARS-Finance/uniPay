use std::fs;

use tars_orderbook::config::settings::Settings;
use tempfile::tempdir;

#[test]
fn parses_settings_with_policy_path_and_without_strategy_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("Settings.toml");

    fs::write(
        &path,
        r#"
addr = "0.0.0.0:6969"
db_url = "postgres://postgres:postgres@localhost:5432/postgres"
chain_json_path = "local/chain.json"
policy_path = "local/policy.toml"

[chain_ids]
base_sepolia = "84532"
arbitrum_sepolia = "421614"
bitcoin_testnet = "1"
"#,
    )
    .unwrap();

    let settings = Settings::from_toml(path.to_str().unwrap()).unwrap();

    assert_eq!(settings.policy_path, "local/policy.toml");
}
