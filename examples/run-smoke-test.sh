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
#   SKIP_DOCKER=1 ./examples/run-smoke-test.sh  # Skip Level 3

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

pass() { echo -e "  ${GREEN}PASS${NC}: $1"; PASSED=$((PASSED + 1)); }
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
  name="$(basename "$f")"
  if $EGRET validate -f "$f" >/dev/null 2>&1; then
    pass "validate $name"
  else
    fail "validate $name"
  fi
done

for dir in multi-container sidecar; do
  d="$SCRIPT_DIR/$dir"
  [ -d "$d" ] || continue
  cmd="$EGRET validate -f $d/task-definition.json"
  [ -f "$d/egret-override.json" ] && cmd="$cmd -o $d/egret-override.json"
  [ -f "$d/secrets.local.json" ] && cmd="$cmd -s $d/secrets.local.json"
  if eval "$cmd" >/dev/null 2>&1; then
    pass "validate $dir"
  else
    fail "validate $dir"
  fi
done

for f in "$SCRIPT_DIR"/level3/*.json; do
  name="$(basename "$f")"
  if $EGRET validate -f "$f" >/dev/null 2>&1; then
    pass "validate level3/$name"
  else
    fail "validate level3/$name"
  fi
done

# ═══════════════════════════════════════════════
# Level 2: Dry Run (no Docker required)
# ═══════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══ Level 2: Dry Run ═══${NC}"

for f in "$SCRIPT_DIR"/aws-samples/*.json; do
  name="$(basename "$f")"
  if $EGRET run --dry-run --no-metadata -f "$f" >/dev/null 2>&1; then
    pass "dry-run $name"
  else
    fail "dry-run $name"
  fi
done

for dir in multi-container sidecar; do
  d="$SCRIPT_DIR/$dir"
  [ -d "$d" ] || continue
  cmd="$EGRET run --dry-run --no-metadata -f $d/task-definition.json"
  [ -f "$d/egret-override.json" ] && cmd="$cmd -o $d/egret-override.json"
  [ -f "$d/secrets.local.json" ] && cmd="$cmd -s $d/secrets.local.json"
  if eval "$cmd" >/dev/null 2>&1; then
    pass "dry-run $dir"
  else
    fail "dry-run $dir"
  fi
done

for f in "$SCRIPT_DIR"/level3/*.json; do
  name="$(basename "$f")"
  if $EGRET run --dry-run --no-metadata -f "$f" >/dev/null 2>&1; then
    pass "dry-run level3/$name"
  else
    fail "dry-run level3/$name"
  fi
done

# ═══════════════════════════════════════════════
# Level 3: Container Run (Docker required)
# ═══════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══ Level 3: Container Run ═══${NC}"

if [ -n "${SKIP_DOCKER:-}" ]; then
  skip "all Docker tests (SKIP_DOCKER is set)"
elif ! command -v docker &>/dev/null || ! docker info &>/dev/null 2>&1; then
  skip "all Docker tests (Docker not available)"
else
  # Build local test image
  bash "$SCRIPT_DIR/test-image/build.sh" egret-test:latest

  # Helper: run a scenario, check ps, then stop
  run_scenario() {
    local name="$1"
    local family="$2"
    local taskdef="$3"
    local wait_secs="${4:-8}"

    echo "--- $name ---"

    $EGRET run --no-metadata -f "$taskdef" >/dev/null 2>&1 &
    local run_pid=$!
    sleep "$wait_secs"

    # Check containers are running
    if $EGRET ps 2>/dev/null | grep -q "$family"; then
      pass "$name: containers running"
    else
      fail "$name: containers not found via egret ps"
      kill "$run_pid" 2>/dev/null || true
      wait "$run_pid" 2>/dev/null || true
      return
    fi

    # Stop the task
    if $EGRET stop "$family" >/dev/null 2>&1; then
      pass "$name: stop succeeded"
    else
      fail "$name: stop failed"
    fi

    # Send SIGINT to the run process (it waits for Ctrl-C in stream_logs_until_signal)
    kill -INT "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true

    # Verify containers are gone
    sleep 1
    if ! docker ps --filter "label=egret.task=$family" --format '{{.Names}}' 2>/dev/null | grep -q .; then
      pass "$name: cleanup verified"
    else
      fail "$name: containers still running after stop"
      docker rm -f "$(docker ps -q --filter "label=egret.task=$family")" 2>/dev/null || true
    fi
  }

  # --- Scenario 1: Single container ---
  run_scenario "single-container" "smoke-single" \
    "$SCRIPT_DIR/level3/single-container.json" 5

  # --- Scenario 2: Multi-container (dependsOn + healthCheck) ---
  run_scenario "multi-container" "smoke-multi" \
    "$SCRIPT_DIR/level3/multi-container.json" 20

  # --- Scenario 3: Sidecar (essential=false + dependsOn START) ---
  run_scenario "sidecar" "smoke-sidecar" \
    "$SCRIPT_DIR/level3/sidecar.json" 8
fi

# ═══════════════════════════════════════════════
# Summary
# ═══════════════════════════════════════════════
echo ""
echo -e "${BOLD}═══ Results: ${GREEN}$PASSED passed${NC}, ${RED}$FAILED failed${NC}, ${YELLOW}$SKIPPED skipped${NC} ═══"
[ "$FAILED" -eq 0 ] || exit 1
