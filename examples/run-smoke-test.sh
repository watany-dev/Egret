#!/usr/bin/env bash
# Egret Dog Routing Smoke Test
# Tests Egret CLI against real AWS ECS task definition samples.
#
# Levels:
#   1: validate  (no Docker required)
#   2: dry-run   (no Docker required)
#   3: run       (Docker required, skipped if unavailable)
#
# Usage:
#   ./examples/run-smoke-test.sh            # Run all levels
#   EGRET_BIN=./target/release/egret ./examples/run-smoke-test.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EGRET="${EGRET_BIN:-cargo run --release --}"

PASSED=0
FAILED=0
SKIPPED=0

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
NC='\033[0m'

pass() { echo -e "  ${GREEN}PASS${NC}"; PASSED=$((PASSED + 1)); }
fail() { echo -e "  ${RED}FAIL${NC}: $1"; FAILED=$((FAILED + 1)); }
skip() { echo -e "  ${YELLOW}SKIP${NC}: $1"; SKIPPED=$((SKIPPED + 1)); }

cleanup() {
  if command -v docker &>/dev/null && docker info &>/dev/null 2>&1; then
    $EGRET stop --all 2>/dev/null || true
  fi
}
trap cleanup EXIT

# ═══════════════════════════════════════════════
# Level 1: Validate (no Docker required)
# ═══════════════════════════════════════════════
echo -e "${BOLD}═══ Level 1: Validate ═══${NC}"

for f in "$SCRIPT_DIR"/aws-samples/*.json; do
  echo "--- $(basename "$f") ---"
  if $EGRET validate -f "$f" 2>&1; then
    pass
  else
    fail "validate failed"
  fi
done

for dir in multi-container sidecar; do
  d="$SCRIPT_DIR/$dir"
  [ -d "$d" ] || continue
  echo "--- $dir ---"
  cmd="$EGRET validate -f $d/task-definition.json"
  [ -f "$d/egret-override.json" ] && cmd="$cmd -o $d/egret-override.json"
  [ -f "$d/secrets.local.json" ] && cmd="$cmd -s $d/secrets.local.json"
  if eval "$cmd" 2>&1; then
    pass
  else
    fail "validate failed"
  fi
done

# ═══════════════════════════════════════════════
# Level 2: Dry Run (no Docker required)
# ═══════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══ Level 2: Dry Run ═══${NC}"

for f in "$SCRIPT_DIR"/aws-samples/*.json; do
  echo "--- $(basename "$f") ---"
  if $EGRET run --dry-run --no-metadata -f "$f" 2>&1; then
    pass
  else
    fail "dry-run failed"
  fi
done

for dir in multi-container sidecar; do
  d="$SCRIPT_DIR/$dir"
  [ -d "$d" ] || continue
  echo "--- $dir ---"
  cmd="$EGRET run --dry-run --no-metadata -f $d/task-definition.json"
  [ -f "$d/egret-override.json" ] && cmd="$cmd -o $d/egret-override.json"
  [ -f "$d/secrets.local.json" ] && cmd="$cmd -s $d/secrets.local.json"
  if eval "$cmd" 2>&1; then
    pass
  else
    fail "dry-run failed"
  fi
done

# ═══════════════════════════════════════════════
# Level 3: Container Run (Docker required)
# ═══════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══ Level 3: Container Run ═══${NC}"

if ! docker info &>/dev/null 2>&1; then
  skip "Docker not available"
else
  # --- nginx-fargate ---
  echo "--- nginx-fargate (run + ps + stop) ---"
  $EGRET run --no-metadata -f "$SCRIPT_DIR/aws-samples/nginx-fargate.json" &
  RUN_PID=$!
  sleep 5

  if $EGRET ps 2>&1; then
    pass
  else
    fail "ps failed"
  fi

  if $EGRET stop nginx 2>&1; then
    pass
  else
    fail "stop failed"
  fi
  wait "$RUN_PID" 2>/dev/null || true

  # --- multi-container (dependsOn + healthCheck) ---
  echo "--- multi-container (run + ps + stop) ---"
  $EGRET run --no-metadata \
    -f "$SCRIPT_DIR/multi-container/task-definition.json" \
    -o "$SCRIPT_DIR/multi-container/egret-override.json" \
    -s "$SCRIPT_DIR/multi-container/secrets.local.json" &
  RUN_PID=$!
  sleep 20  # Wait for healthChecks to pass

  if $EGRET ps 2>&1; then
    pass
  else
    fail "ps failed"
  fi

  if $EGRET stop multi-webapp 2>&1; then
    pass
  else
    fail "stop failed"
  fi
  wait "$RUN_PID" 2>/dev/null || true

  # --- sidecar ---
  echo "--- sidecar (run + ps + stop) ---"
  mkdir -p /tmp/egret-app-logs
  $EGRET run --no-metadata \
    -f "$SCRIPT_DIR/sidecar/task-definition.json" \
    -o "$SCRIPT_DIR/sidecar/egret-override.json" &
  RUN_PID=$!
  sleep 5

  if $EGRET ps 2>&1; then
    pass
  else
    fail "ps failed"
  fi

  if $EGRET stop app-with-sidecar 2>&1; then
    pass
  else
    fail "stop failed"
  fi
  wait "$RUN_PID" 2>/dev/null || true
fi

# ═══════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══ Results: ${GREEN}$PASSED passed${NC}, ${RED}$FAILED failed${NC}, ${YELLOW}$SKIPPED skipped${NC} ═══"
[ "$FAILED" -eq 0 ] || exit 1
