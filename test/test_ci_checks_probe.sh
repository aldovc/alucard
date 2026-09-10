#!/bin/bash
# Fine-grained PATs cannot reach statusCheckRollup. The CI gate must probe
# that surface once per run, remember the outcome, and stop retrying a call
# it has already learned will fail — without blaming a permission that does
# not exist to grant.
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

assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    fail "$label (output unexpectedly contains '$needle')"
  else
    pass "$label"
  fi
}

TEST_DIR=$(mktemp -d /tmp/alucard_test_ci_checks.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
GH_TRACE="$TEST_DIR/gh-trace"
EVENTS="$TEST_DIR/events"
REPO="$TEST_DIR/repo"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN"

git init -q --bare "$REMOTE"
mkdir -p "$REPO"
git -C "$REPO" init -q -b main
git -C "$REPO" config user.email test@example.invalid
git -C "$REPO" config user.name test
printf 'hello\n' > "$REPO/README.md"
git -C "$REPO" add .
git -C "$REPO" commit -qm initial
git -C "$REPO" remote add origin "$REMOTE"
git -C "$REPO" push -qu origin main
git -C "$REPO" checkout -qb feature-a
printf 'a\n' >> "$REPO/README.md"
git -C "$REPO" commit -qam a
git -C "$REPO" push -qu origin feature-a
git -C "$REPO" checkout -q main
git -C "$REPO" checkout -qb feature-b
printf 'b\n' >> "$REPO/README.md"
git -C "$REPO" commit -qam b
git -C "$REPO" push -qu origin feature-b
git -C "$REPO" checkout -q main

HEAD_A=$(git -C "$REPO" rev-parse origin/feature-a)
HEAD_B=$(git -C "$REPO" rev-parse origin/feature-b)

# Mode driven by ALUCARD_TEST_CHECKS_MODE:
#   denied  — pr checks always fails with the fine-grained PAT error
#   allowed — pr checks reports no checks (API reachable)
#   fail    — pr checks reports failure (API reachable, CI red)

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "$ALUCARD_TEST_GH_TRACE"
case "$1 $2" in
  "pr list")
    case "$*" in
      *feature-a*) printf '101\n' ;;
      *feature-b*) printf '102\n' ;;
      *)           printf '101\n' ;;
    esac ;;
  "pr view")
    case "$*" in
      *101*) printf '%s\n' "$ALUCARD_TEST_HEAD_A" ;;
      *102*) printf '%s\n' "$ALUCARD_TEST_HEAD_B" ;;
      *)     printf '%s\n' "$ALUCARD_TEST_HEAD_A" ;;
    esac ;;
  "pr checks")
    case "${ALUCARD_TEST_CHECKS_MODE:-denied}" in
      denied)
        echo "GraphQL: Resource not accessible by personal access token (node.statusCheckRollup.nodes.0.commit.statusCheckRollup)" >&2
        exit 1 ;;
      allowed)
        echo "no checks reported on the branch" >&2
        exit 1 ;;
      fail)
        echo "some checks were not successful" >&2
        exit 1 ;;
    esac ;;
  "run list")
    _sha="$ALUCARD_TEST_HEAD_A"
    case "$*" in
      *feature-b*) _sha="$ALUCARD_TEST_HEAD_B" ;;
    esac
    printf '[{"databaseId":99,"status":"completed","conclusion":"%s","headSha":"%s"}]\n' \
      "${ALUCARD_TEST_RUN_CONCLUSION:-success}" "$_sha" ;;
  "run view")
    printf 'FAILED something\n' ;;
  *)
    echo "unexpected gh: $*" >&2
    exit 1 ;;
esac
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then
  shift 3
  # fix-agent docker path — succeed instantly
  printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"ok"}}}'
  exit 0
fi
shift
exec "$@"
MOCK

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
exit 0
MOCK
chmod +x "$MOCK_BIN/gh" "$MOCK_BIN/timeout" "$MOCK_BIN/docker"

# shellcheck disable=SC1090
source "$ALUCARD"

export PATH="$MOCK_BIN:$PATH"
export ALUCARD_TEST_GH_TRACE="$GH_TRACE"
export ALUCARD_TEST_HEAD_A="$HEAD_A"
export ALUCARD_TEST_HEAD_B="$HEAD_B"

REPO_ABS="$REPO"
WT_ROOT="$TEST_DIR/wt"
LOG_DIR="$TEST_DIR/logs"
mkdir -p "$WT_ROOT" "$LOG_DIR"
ENV_FILE="$SCRIPT_DIR/fixtures/credentials.env"
IMAGE="test-image"
TOOL_DIR="$SCRIPT_DIR/.."
TIMEOUT_MIN=1
stream_text='.event.delta.text'
CREATED_WORKTREES=()
CREATED_CONTAINERS=()
DEFAULT_CI_FIX_MAX_TURNS=5
DEFAULT_CI_FIX_MAX_BUDGET=1
MEASURE_BASE_SHA=""
MEASURE_PREV_SHA=""
MEASURE_CI_RESULT=""

log_event() { printf '%s\n' "$*" >> "$EVENTS"; }
measure_stage() { :; }
make_agent_clone() { mkdir -p "$2"; }

run_two_gates() {
  : > "$GH_TRACE"
  : > "$EVENTS"
  CI_CHECKS_AVAILABLE=""
  export ALUCARD_TEST_CHECKS_MODE="$1"
  export ALUCARD_TEST_RUN_CONCLUSION="${2:-success}"
  set +e
  ci_gate 1 feature-a "$REMOTE" >"$TEST_DIR/out-a" 2>&1
  ci_gate 2 feature-b "$REMOTE" >"$TEST_DIR/out-b" 2>&1
  set -e
  GH=$(<"$GH_TRACE")
  EV=$(<"$EVENTS")
}

# ── Fine-grained PAT: probe once, then skip ──────────────────────────────────
echo "── fine-grained PAT: one probe ──"
run_two_gates denied success

checks_calls=$(grep -c 'pr checks' <<<"$GH" || true)
assert_eq "pr checks is called at most once across two gates" "1" "$checks_calls"
assert_eq "the degradation is remembered" "false" "$CI_CHECKS_AVAILABLE"
degrade_logs=$(grep -c 'checks API unavailable to this token type' <<<"$EV" || true)
assert_eq "the degradation is logged once per run" "1" "$degrade_logs"
assert_contains "the message names a token-type limit" \
  "fine-grained PAT" "$EV"
assert_not_contains "the message does not name a grant that cannot exist" \
  "lacks 'checks' read permission" "$EV"
assert_not_contains "the old comment wording is gone from events" \
  "token lacks" "$EV"
assert_contains "both gates still reach a green verdict via run list" \
  "green — ready for review" "$EV"
green_count=$(grep -c 'green — ready for review' <<<"$EV" || true)
assert_eq "both PRs are reported green" "2" "$green_count"
run_list_calls=$(grep -c 'run list' <<<"$GH" || true)
if [ "$run_list_calls" -ge 2 ]; then
  pass "run list is used for both gates after the probe"
else
  fail "run list is used for both gates after the probe (got $run_list_calls)"
fi

# ── Classic path: checks API works ───────────────────────────────────────────
echo ""
echo "── classic PAT: pr checks still used ──"
run_two_gates allowed success

checks_calls=$(grep -c 'pr checks' <<<"$GH" || true)
assert_eq "pr checks is used on every gate when available" "2" "$checks_calls"
assert_eq "availability is remembered as true" "true" "$CI_CHECKS_AVAILABLE"
assert_not_contains "no degradation log when the API works" \
  "checks API unavailable" "$EV"
assert_contains "no-checks still skips as green" \
  "no checks configured" "$EV"

# ── Failure still detected via run list ──────────────────────────────────────
echo ""
echo "── fine-grained PAT: failing run still triggers fix ──"
: > "$GH_TRACE"
: > "$EVENTS"
CI_CHECKS_AVAILABLE=""
export ALUCARD_TEST_CHECKS_MODE=denied
export ALUCARD_TEST_RUN_CONCLUSION=failure
# Short-circuit the 30s poll sleep: poll_ci_via_run_list sleeps only when
# total==0 or in_progress>0; our mock returns a completed failure immediately.
set +e
ci_gate 1 feature-a "$REMOTE" >"$TEST_DIR/out-fail" 2>&1
set -e
EV=$(<"$EVENTS")
assert_contains "a failed run list conclusion launches the fix agent" \
  "launching fix agent" "$EV"
assert_contains "exhausted attempts still leave the PR open" \
  "still failing after 3 fix attempts" "$EV"
assert_eq "MEASURE_CI_RESULT records the failure" "failed" "$MEASURE_CI_RESULT"

# ── Pre-seeded false skips the probe entirely ────────────────────────────────
echo ""
echo "── remembered false never calls pr checks ──"
: > "$GH_TRACE"
: > "$EVENTS"
CI_CHECKS_AVAILABLE=false
export ALUCARD_TEST_CHECKS_MODE=denied
export ALUCARD_TEST_RUN_CONCLUSION=success
set +e
ci_gate 1 feature-a "$REMOTE" >"$TEST_DIR/out-skip" 2>&1
set -e
GH=$(<"$GH_TRACE")
EV=$(<"$EVENTS")
assert_eq "a remembered false makes zero pr checks calls" \
  "0" "$(grep -c 'pr checks' <<<"$GH" || true)"
assert_not_contains "no re-log when already known" \
  "checks API unavailable" "$EV"
assert_contains "run list still greens the PR" \
  "green — ready for review" "$EV"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
