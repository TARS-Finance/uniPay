#!/usr/bin/env bash
# Health-check (and optionally restart) the tars-rs services.
#
# Usage:
#   ./scripts/services.sh              # check all services, exit non-zero if any down
#   ./scripts/services.sh --restart    # restart all services, then check
#   ./scripts/services.sh -h|--help

set -uo pipefail

# ---- Configure your services here ------------------------------------------
# Format per service: "name|health_url|start_cmd|stop_cmd"
#   - health_url: HTTP(S) URL returning 2xx when healthy (use "" to skip HTTP check)
#   - start_cmd / stop_cmd: shell commands to (re)start / stop the service
#
# Replace these placeholders with your real services.
SERVICES=(
  "api|http://localhost:8080/health|cargo run -p api|pkill -f 'target/.*api'"
  "orderbook|http://localhost:8081/health|cargo run -p orderbook|pkill -f 'target/.*orderbook'"
  "quote|http://localhost:8082/health|cargo run -p quote|pkill -f 'target/.*quote'"
)

LOG_DIR="${LOG_DIR:-/tmp/tars-rs}"
HEALTH_TIMEOUT="${HEALTH_TIMEOUT:-3}"        # seconds per probe
HEALTH_RETRIES="${HEALTH_RETRIES:-20}"       # total probes after restart
HEALTH_INTERVAL="${HEALTH_INTERVAL:-1}"      # seconds between probes
# ---------------------------------------------------------------------------

RED=$'\e[31m'; GREEN=$'\e[32m'; YELLOW=$'\e[33m'; RESET=$'\e[0m'
log()  { printf '%s\n' "$*"; }
ok()   { printf '%s[ok]%s %s\n'   "$GREEN"  "$RESET" "$*"; }
warn() { printf '%s[..]%s %s\n'   "$YELLOW" "$RESET" "$*"; }
err()  { printf '%s[!!]%s %s\n'   "$RED"    "$RESET" "$*" >&2; }

usage() { sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'; }

probe() {
  local url=$1
  [[ -z "$url" ]] && return 0
  curl -fsS --max-time "$HEALTH_TIMEOUT" -o /dev/null "$url"
}

wait_healthy() {
  local name=$1 url=$2 i
  [[ -z "$url" ]] && { ok "$name (no health url, assumed up)"; return 0; }
  for ((i=1; i<=HEALTH_RETRIES; i++)); do
    if probe "$url"; then ok "$name healthy ($url)"; return 0; fi
    sleep "$HEALTH_INTERVAL"
  done
  err "$name failed health check after $((HEALTH_RETRIES * HEALTH_INTERVAL))s ($url)"
  return 1
}

stop_service() {
  local name=$1 stop_cmd=$2
  warn "stopping $name"
  bash -c "$stop_cmd" >/dev/null 2>&1 || true
}

start_service() {
  local name=$1 start_cmd=$2
  mkdir -p "$LOG_DIR"
  warn "starting $name (logs: $LOG_DIR/$name.log)"
  nohup bash -c "$start_cmd" >>"$LOG_DIR/$name.log" 2>&1 &
  disown || true
}

check_all() {
  local failed=0
  for svc in "${SERVICES[@]}"; do
    IFS='|' read -r name url _ _ <<<"$svc"
    if probe "$url"; then ok "$name up"
    else err "$name DOWN"; failed=1; fi
  done
  return $failed
}

restart_all() {
  for svc in "${SERVICES[@]}"; do
    IFS='|' read -r name _ _ stop_cmd <<<"$svc"
    stop_service "$name" "$stop_cmd"
  done
  for svc in "${SERVICES[@]}"; do
    IFS='|' read -r name _ start_cmd _ <<<"$svc"
    start_service "$name" "$start_cmd"
  done
  local failed=0
  for svc in "${SERVICES[@]}"; do
    IFS='|' read -r name url _ _ <<<"$svc"
    wait_healthy "$name" "$url" || failed=1
  done
  return $failed
}

main() {
  local restart=0
  for arg in "$@"; do
    case "$arg" in
      --restart) restart=1 ;;
      -h|--help) usage; exit 0 ;;
      *) err "unknown arg: $arg"; usage; exit 2 ;;
    esac
  done

  if (( restart )); then
    restart_all || exit 1
  else
    check_all   || exit 1
  fi
  ok "all services up"
}

main "$@"
