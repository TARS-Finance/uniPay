#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
CONFIG="$SCRIPT_DIR/../weave/launch_config.json"
ENV_FILE="$SCRIPT_DIR/../.env"

if [ ! -f "$CONFIG" ]; then
  echo "missing $CONFIG (regenerate with: npm run derive-addresses && bash scripts/regen-launch-config.sh)"
  exit 1
fi

if [ -f "$ENV_FILE" ]; then
  set -a
  # shellcheck disable=SC1090
  source "$ENV_FILE"
  set +a
fi

update_env_value() {
  local key="$1"
  local value="$2"

  python3 - "$ENV_FILE" "$key" "$value" <<'PY'
import os
import sys

path, key, value = sys.argv[1:]
existing = open(path).read() if os.path.exists(path) else ""
lines = [line for line in existing.splitlines() if not line.startswith(f"{key}=")]
lines.append(f"{key}={value}")
with open(path, "w") as fh:
    fh.write("\n".join(line for line in lines if line).rstrip() + "\n")
PY
}

query_uinit_balance() {
  local addr="$1"
  local rpc="$2"

  initiad query bank balances "$addr" --node "$rpc" -o json 2>/dev/null \
    | jq -r '([.balances[]? | select(.denom == "uinit") | .amount | tonumber] | add) // 0'
}

wait_for_tx() {
  local tx_hash="$1"
  local rpc="$2"
  local attempt

  for attempt in $(seq 1 20); do
    if initiad query tx "$tx_hash" --node "$rpc" -o json >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done

  return 1
}

ensure_l1_system_funds() {
  local l1_chain_id
  local l1_rpc
  local gas_prices
  local min_balance
  local fund_amount
  local merchant_addr
  local merchant_balance
  local tmp_home
  local tx_file
  local tx_hash
  local balance
  local recipient
  local -a recipients=()
  local -a needs_funding=()

  l1_chain_id="$(jq -r '.l1_config.chain_id' "$CONFIG")"
  l1_rpc="$(jq -r '.l1_config.rpc_url' "$CONFIG")"
  gas_prices="$(jq -r '.l1_config.gas_prices' "$CONFIG")"
  min_balance="${SYSTEM_KEY_MIN_UINIT:-1000000}"
  fund_amount="${SYSTEM_KEY_FUND_AMOUNT:-2000000uinit}"

  while IFS= read -r recipient; do
    recipients+=("$recipient")
  done < <(
    jq -r '
      .system_keys.bridge_executor.l1_address,
      .system_keys.output_submitter.l1_address,
      .system_keys.batch_submitter.da_address,
      .system_keys.challenger.l1_address
    ' "$CONFIG" | awk 'NF' | sort -u
  )

  echo ">> Checking L1 system-account balances..."
  for recipient in "${recipients[@]}"; do
    balance="$(query_uinit_balance "$recipient" "$l1_rpc")"
    echo "   $recipient: ${balance}uinit"
    if [ "$balance" -lt "$min_balance" ]; then
      needs_funding+=("$recipient")
    fi
  done

  if [ "${#needs_funding[@]}" -eq 0 ]; then
    return
  fi

  if [ -z "${MERCHANT_PRIVATE_KEY:-}" ]; then
    echo ">> FAIL: some L1 system accounts are unfunded and MERCHANT_PRIVATE_KEY is not set in .env"
    echo "   fund these accounts with uinit and rerun:"
    printf '   - %s\n' "${needs_funding[@]}"
    exit 1
  fi

  tmp_home="$(mktemp -d)"
  trap 'rm -rf "$tmp_home"' RETURN

  initiad keys import-hex merchant "${MERCHANT_PRIVATE_KEY#0x}" \
    --key-type eth_secp256k1 \
    --keyring-backend test \
    --home "$tmp_home" >/dev/null

  merchant_addr="$(initiad keys show merchant -a --keyring-backend test --home "$tmp_home")"
  merchant_balance="$(query_uinit_balance "$merchant_addr" "$l1_rpc")"

  echo ">> Funding missing L1 system accounts from $merchant_addr (${merchant_balance}uinit)..."
  for recipient in "${needs_funding[@]}"; do
    if [ "$recipient" = "$merchant_addr" ]; then
      continue
    fi

    tx_file="$(mktemp)"
    echo "   funding $recipient with $fund_amount"
    if ! initiad tx bank send merchant "$recipient" "$fund_amount" \
      --chain-id "$l1_chain_id" \
      --node "$l1_rpc" \
      --gas-prices "$gas_prices" \
      --keyring-backend test \
      --home "$tmp_home" \
      --yes \
      -o json >"$tx_file"; then
      cat "$tx_file"
      rm -f "$tx_file"
      exit 1
    fi

    if [ "$(jq -r '.code // 0' "$tx_file")" != "0" ]; then
      cat "$tx_file"
      rm -f "$tx_file"
      exit 1
    fi

    tx_hash="$(jq -r '.txhash // empty' "$tx_file")"
    if [ -n "$tx_hash" ] && ! wait_for_tx "$tx_hash" "$l1_rpc"; then
      echo ">> FAIL: timed out waiting for funding tx $tx_hash to be committed"
      cat "$tx_file"
      rm -f "$tx_file"
      exit 1
    fi

    rm -f "$tx_file"
  done

  rm -rf "$tmp_home"
  trap - RETURN
}

read_bridge_id() {
  local artifacts_json="$HOME/.minitia/artifacts/artifacts.json"
  local config_toml="$HOME/.minitia/config/config.toml"

  if [ -f "$artifacts_json" ]; then
    jq -r '.BRIDGE_ID // empty' "$artifacts_json"
    return
  fi

  if [ -f "$config_toml" ]; then
    sed -n 's/^bridge_id = \([0-9][0-9]*\)$/\1/p' "$config_toml" | head -n 1
    return
  fi

  printf ''
}

ensure_l1_system_funds

# weave reads from ~/.weave/launch_config.json
mkdir -p ~/.weave
cp "$CONFIG" ~/.weave/launch_config.json

echo ">> Launching rollup (vm=evm) with config:"
jq '{l2_chain_id: .l2_config.chain_id, denom: .l2_config.denom, finalization: .op_bridge.output_finalization_period, merchant: .genesis_accounts[0].address}' "$CONFIG"
echo

echo ">> weave rollup launch (interactive — accept defaults if prompted)"
weave rollup launch --with-config ~/.weave/launch_config.json --vm evm -f

echo
echo ">> Ensuring rollup daemon is running…"
if ! curl -sf http://localhost:26657/status >/dev/null; then
  weave rollup start -d
fi

BRIDGE_ID="$(read_bridge_id)"
if [ -z "$BRIDGE_ID" ] || [ "$BRIDGE_ID" = "0" ]; then
  echo ">> FAIL: weave rollup launch did not produce a valid BRIDGE_ID"
  echo "   check: weave rollup log -n 100"
  exit 1
fi

sleep 5
echo
echo ">> Tail of rollup log:"
if [ -f "$HOME/.weave/log/minitiad.stdout.log" ]; then
  tail -n 30 "$HOME/.weave/log/minitiad.stdout.log" || true
else
  echo "   WARN: $HOME/.weave/log/minitiad.stdout.log not found yet"
fi

echo
echo ">> Initialising and starting OPInit bots…"
weave opinit init executor -f
weave opinit start executor -d
weave opinit init challenger -f
weave opinit start challenger -d

sleep 3
echo
echo ">> opinit executor log tail:"
for log_file in "$HOME/.weave/log/opinitd.executor.stdout.log" "$HOME/.weave/log/opinitd.executor.stderr.log"; do
  if [ -f "$log_file" ]; then
    echo "   $(basename "$log_file"):"
    tail -n 10 "$log_file" || true
  fi
done

echo
echo ">> Health check:"
HEIGHT=$(curl -s http://localhost:26657/status | jq -r '.result.sync_info.latest_block_height // "n/a"')
echo "   latest block height: $HEIGHT"

echo
echo ">> Capturing endpoints into .env"
ROLLUP_CHAIN_ID="$(jq -r '.l2_config.chain_id' "$CONFIG")"
ROLLUP_NATIVE_DENOM="$(jq -r '.l2_config.denom' "$CONFIG")"

update_env_value "ROLLUP_CHAIN_ID" "$ROLLUP_CHAIN_ID"
update_env_value "ROLLUP_RPC_URL" "http://localhost:26657"
update_env_value "ROLLUP_REST_URL" "http://localhost:1317"
update_env_value "ROLLUP_JSON_RPC_URL" "http://localhost:8545"
update_env_value "ROLLUP_NATIVE_DENOM" "$ROLLUP_NATIVE_DENOM"
update_env_value "BRIDGE_ID" "$BRIDGE_ID"
echo ">> bridge_id = $BRIDGE_ID"

echo
echo ">> Done. Verify rollup is producing blocks:"
echo "     curl -s http://localhost:26657/status | jq -r '.result.sync_info.latest_block_height'"
