#!/usr/bin/env bash
set -euo pipefail

ok()   { echo "  [ok]  $1"; }
fail() { echo "  [FAIL] $1"; exit 1; }

echo ">> Preflight"

command -v weave    >/dev/null && ok "weave"    || fail "missing weave"
command -v initiad  >/dev/null && ok "initiad"  || fail "missing initiad"
command -v minitiad >/dev/null && ok "minitiad" || fail "missing minitiad"
command -v jq       >/dev/null && ok "jq"       || fail "missing jq (brew install jq)"
command -v curl     >/dev/null && ok "curl"     || fail "missing curl"

# Confirm minitiad is the minievm flavor (not minimove or miniwasm)
minitiad version --long 2>/dev/null | grep -q '^name: minievm' \
  && ok "minitiad is minievm" \
  || fail "minitiad is NOT minievm (rebuild from initia-labs/minievm)"

echo ">> All preflight checks passed."
