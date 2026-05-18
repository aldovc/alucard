#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"

PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    pass "$label"
  else
    fail "$label (expected '$expected', got '$actual')"
  fi
}

# Run COMMAND in a subshell; assert its exit code matches EXPECTED_EXIT.
assert_exit() {
  local label="$1" expected_exit="$2"
  shift 2
  local actual_exit=0
  ( "$@" ) >/dev/null 2>&1 || actual_exit=$?
  if [ "$actual_exit" -eq "$expected_exit" ]; then
    pass "$label"
  else
    fail "$label (expected exit $expected_exit, got $actual_exit)"
  fi
}

# Source alucard to load helper functions without running main.
# The BASH_SOURCE guard in alucard prevents main from executing when sourced.
# shellcheck disable=SC1090
source "$ALUCARD"

# ── Test group 1: all-claude (default — no env vars set) ─────────────────────

unset ALUCARD_PROVIDER ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
      ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER

assert_eq "default: WORKER resolves to claude"   "claude" "$(resolve_provider WORKER)"
assert_eq "default: CI_FIX resolves to claude"   "claude" "$(resolve_provider CI_FIX)"
assert_eq "default: REVIEWER resolves to claude" "claude" "$(resolve_provider REVIEWER)"
assert_eq "default: FEEDBACK resolves to claude" "claude" "$(resolve_provider FEEDBACK)"

# ── Test group 2: all-codex via global ALUCARD_PROVIDER ─────────────────────

export ALUCARD_PROVIDER=codex
unset ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
      ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER

assert_eq "global=codex: WORKER resolves to codex"   "codex" "$(resolve_provider WORKER)"
assert_eq "global=codex: CI_FIX resolves to codex"   "codex" "$(resolve_provider CI_FIX)"
assert_eq "global=codex: REVIEWER resolves to codex" "codex" "$(resolve_provider REVIEWER)"
assert_eq "global=codex: FEEDBACK resolves to codex" "codex" "$(resolve_provider FEEDBACK)"

# ── Test group 3: per-role override takes precedence over global ─────────────

export ALUCARD_PROVIDER=codex
export ALUCARD_WORKER_PROVIDER=claude

assert_eq "per-role override: WORKER=claude beats global codex" "claude" "$(resolve_provider WORKER)"
assert_eq "per-role override: REVIEWER still codex from global" "codex"  "$(resolve_provider REVIEWER)"
assert_eq "per-role override: CI_FIX still codex from global"   "codex"  "$(resolve_provider CI_FIX)"
assert_eq "per-role override: FEEDBACK still codex from global" "codex"  "$(resolve_provider FEEDBACK)"

# ── Test group 4: mixed config (no global, only one role pinned) ─────────────

unset ALUCARD_PROVIDER ALUCARD_CI_FIX_PROVIDER \
      ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER
export ALUCARD_WORKER_PROVIDER=codex

assert_eq "mixed: WORKER=codex"    "codex"  "$(resolve_provider WORKER)"
assert_eq "mixed: CI_FIX=claude"   "claude" "$(resolve_provider CI_FIX)"
assert_eq "mixed: REVIEWER=claude" "claude" "$(resolve_provider REVIEWER)"
assert_eq "mixed: FEEDBACK=claude" "claude" "$(resolve_provider FEEDBACK)"

unset ALUCARD_WORKER_PROVIDER

# ── Test group 5: unrecognized provider value exits non-zero ─────────────────

export ALUCARD_WORKER_PROVIDER=openai
assert_exit "unrecognized provider 'openai' exits non-zero" 1 resolve_provider WORKER
unset ALUCARD_WORKER_PROVIDER

export ALUCARD_PROVIDER=bedrock
assert_exit "unrecognized global provider 'bedrock' exits non-zero" 1 resolve_provider CI_FIX
unset ALUCARD_PROVIDER

# ── Test group 6: any_role_uses ──────────────────────────────────────────────

unset ALUCARD_PROVIDER ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
      ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER

any_role_uses claude   && pass "all-default: any_role_uses claude is true"  || fail "all-default: any_role_uses claude should be true"
any_role_uses codex    && fail "all-default: any_role_uses codex should be false" || pass "all-default: any_role_uses codex is false"

export ALUCARD_WORKER_PROVIDER=codex
any_role_uses codex    && pass "WORKER=codex: any_role_uses codex is true"  || fail "WORKER=codex: any_role_uses codex should be true"
any_role_uses claude   && pass "WORKER=codex, others default: any_role_uses claude is true" || fail "WORKER=codex, others default: any_role_uses claude should be true"
unset ALUCARD_WORKER_PROVIDER

export ALUCARD_PROVIDER=codex
unset ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
      ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER
any_role_uses codex    && pass "all-codex: any_role_uses codex is true"  || fail "all-codex: any_role_uses codex should be true"
any_role_uses claude   && fail "all-codex: any_role_uses claude should be false" || pass "all-codex: any_role_uses claude is false"
unset ALUCARD_PROVIDER

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
