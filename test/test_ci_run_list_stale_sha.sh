#!/bin/bash
# poll_ci_via_run_list SHA-filters to PR HEAD. When the branch has runs for
# older commits and nothing for HEAD, waiting 45 minutes then blaming Actions
# permissions is wrong: the token listed runs, none of them were for this SHA.
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

assert_le() {
  local label="$1" max="$2" actual="$3"
  if [ "$actual" -le "$max" ]; then
    pass "$label"
  else
    fail "$label (expected <= $max, got $actual)"
  fi
}

TEST_DIR=$(mktemp -d /tmp/alucard_test_ci_stale.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
GH_TRACE="$TEST_DIR/gh-trace"
SLEEP_TRACE="$TEST_DIR/sleep-trace"
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
git -C "$REPO" checkout -qb feature
printf 'old\n' >> "$REPO/README.md"
git -C "$REPO" commit -qam old
git -C "$REPO" push -qu origin feature
OLD_SHA=$(git -C "$REPO" rev-parse HEAD)
printf 'new\n' >> "$REPO/README.md"
git -C "$REPO" commit -qam new
git -C "$REPO" push -qu origin feature
HEAD_SHA=$(git -C "$REPO" rev-parse HEAD)
git -C "$REPO" checkout -q main

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "$ALUCARD_TEST_GH_TRACE"
case "$1 $2" in
  "pr list")
    printf '479\n' ;;
  "pr view")
    printf '%s\n' "$ALUCARD_TEST_HEAD_SHA" ;;
  "pr checks")
    echo "GraphQL: Resource not accessible by personal access token" >&2
    exit 1 ;;
  "run list")
    n=0
    if [ -f "$ALUCARD_TEST_RUN_COUNT" ]; then
      n=$(<"$ALUCARD_TEST_RUN_COUNT")
    fi
    n=$((n + 1))
    printf '%s\n' "$n" > "$ALUCARD_TEST_RUN_COUNT"
    case "${ALUCARD_TEST_RUN_MODE:-stale}" in
      empty)
        printf '[]\n' ;;
      stale)
        printf '[{"databaseId":11,"status":"completed","conclusion":"success","headSha":"%s"}]\n' \
          "$ALUCARD_TEST_OLD_SHA" ;;
      stale_then_head)
        if [ "$n" -eq 1 ]; then
          printf '[{"databaseId":11,"status":"completed","conclusion":"success","headSha":"%s"}]\n' \
            "$ALUCARD_TEST_OLD_SHA"
        else
          printf '[{"databaseId":22,"status":"completed","conclusion":"success","headSha":"%s"}]\n' \
            "$ALUCARD_TEST_HEAD_SHA"
        fi ;;
      in_progress_then_success)
        if [ "$n" -eq 1 ]; then
          printf '[{"databaseId":22,"status":"in_progress","conclusion":"","headSha":"%s"}]\n' \
            "$ALUCARD_TEST_HEAD_SHA"
        else
          printf '[{"databaseId":22,"status":"completed","conclusion":"success","headSha":"%s"}]\n' \
            "$ALUCARD_TEST_HEAD_SHA"
        fi ;;
      head_failure)
        printf '[{"databaseId":33,"status":"completed","conclusion":"failure","headSha":"%s"}]\n' \
          "$ALUCARD_TEST_HEAD_SHA" ;;
      head_success)
        printf '[{"databaseId":22,"status":"completed","conclusion":"success","headSha":"%s"}]\n' \
          "$ALUCARD_TEST_HEAD_SHA" ;;
      *)
        echo "unknown ALUCARD_TEST_RUN_MODE=${ALUCARD_TEST_RUN_MODE:-}" >&2
        exit 1 ;;
    esac ;;
  "run view")
    printf 'FAILED something\n' ;;
  *)
    echo "unexpected gh: $*" >&2
    exit 1 ;;
esac
MOCK

cat > "$MOCK_BIN/sleep" <<'MOCK'
#!/bin/bash
printf '%s\n' "$*" >> "$ALUCARD_TEST_SLEEP_TRACE"
exit 0
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then
  shift 3
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
chmod +x "$MOCK_BIN/gh" "$MOCK_BIN/sleep" "$MOCK_BIN/timeout" "$MOCK_BIN/docker"

# shellcheck disable=SC1090
source "$ALUCARD"

export PATH="$MOCK_BIN:$PATH"
export ALUCARD_TEST_GH_TRACE="$GH_TRACE"
export ALUCARD_TEST_SLEEP_TRACE="$SLEEP_TRACE"
export ALUCARD_TEST_OLD_SHA="$OLD_SHA"
export ALUCARD_TEST_HEAD_SHA="$HEAD_SHA"
export ALUCARD_TEST_RUN_COUNT="$TEST_DIR/run-count"

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

reset_traces() {
  : > "$GH_TRACE"
  : > "$SLEEP_TRACE"
  : > "$EVENTS"
  : > "$ALUCARD_TEST_RUN_COUNT"
}

run_poll() {
  local mode="$1"
  reset_traces
  export ALUCARD_TEST_RUN_MODE="$mode"
  set +e
  POLL_RC=$(poll_ci_via_run_list 479 feature "$HEAD_SHA" 2>"$TEST_DIR/poll-err")
  POLL_ERR=$(<"$TEST_DIR/poll-err")
  set -e
  GH=$(<"$GH_TRACE")
  EV=$(<"$EVENTS")
  SLEEP_N=$(grep -c . "$SLEEP_TRACE" 2>/dev/null || true)
  RUN_N=$(grep -c 'run list' <<<"$GH" || true)
}

# ── Stale SHA: branch ran, HEAD did not ──────────────────────────────────────
echo "── stale SHA: skip without the 45-minute wait ──"
run_poll stale

assert_eq "stale SHA skip-as-green (cannot force Actions to start)" "0" "$POLL_RC"
assert_contains "logs no run for current HEAD" \
  "no run for current HEAD" "$EV"
assert_contains "names the HEAD SHA" \
  "$HEAD_SHA" "$EV"
assert_contains "mentions older SHAs on the branch did run" \
  "older SHAs on this branch did run" "$EV"
assert_not_contains "does not blame Actions permissions" \
  "actions' read permission" "$EV"
assert_not_contains "does not print the empty-branch wait line" \
  "No workflow runs found yet" "$POLL_ERR"
assert_eq "probes twice (one lag, then skip)" "2" "$RUN_N"
assert_eq "sleeps once between the two probes" "1" "$SLEEP_N"

# ── Empty branch: still skip-as-green after a short wait ─────────────────────
echo ""
echo "── empty branch: short wait then skip-as-green ──"
run_poll empty

assert_eq "empty branch skip-as-green" "0" "$POLL_RC"
assert_contains "empty branch still names the Actions-permission skip" \
  "token may lack 'actions' read permission" "$EV"
assert_not_contains "empty branch does not use the stale-HEAD log" \
  "no run for current HEAD" "$EV"
assert_contains "empty branch still prints the not-started wait line" \
  "No workflow runs found yet" "$POLL_ERR"
assert_eq "empty branch probes four times" "4" "$RUN_N"
assert_le "empty branch does not spin toward 2700s" "6" "$SLEEP_N"

# ── Lag window: HEAD run appearing on the second probe is used ───────────────
echo ""
echo "── lag window: HEAD run on second probe is green ──"
run_poll stale_then_head

assert_eq "a HEAD run that appears after the lag is green" "0" "$POLL_RC"
assert_not_contains "does not skip once a HEAD run exists" \
  "no run for current HEAD" "$EV"
assert_eq "stops after the HEAD run appears" "2" "$RUN_N"

# ── In-progress HEAD still waits ─────────────────────────────────────────────
echo ""
echo "── in-progress HEAD waits then greens ──"
run_poll in_progress_then_success

assert_eq "in-progress HEAD completes as green" "0" "$POLL_RC"
assert_contains "in-progress wait message still fires" \
  "Runs still in progress" "$POLL_ERR"
assert_eq "waits once for the in-progress run" "1" "$SLEEP_N"

# ── Failing HEAD still returns 1 ─────────────────────────────────────────────
echo ""
echo "── failing HEAD run is red ──"
run_poll head_failure

assert_eq "a failed HEAD run is red" "1" "$POLL_RC"
assert_eq "no wait when the failed run is already complete" "0" "$SLEEP_N"

# ── ci_gate: stale SHA does not launch a fix agent ───────────────────────────
echo ""
echo "── ci_gate: stale SHA skips, does not launch a fix agent ──"
reset_traces
export ALUCARD_TEST_RUN_MODE=stale
CI_CHECKS_AVAILABLE=false
MEASURE_CI_RESULT=""
set +e
ci_gate 1 feature "$REMOTE" >"$TEST_DIR/out-stale" 2>&1
set -e
EV=$(<"$EVENTS")
OUT=$(<"$TEST_DIR/out-stale")
assert_contains "ci_gate logs the stale-HEAD skip" \
  "no run for current HEAD" "$EV"
assert_not_contains "ci_gate does not blame Actions permissions" \
  "actions' read permission" "$EV"
assert_not_contains "ci_gate does not launch a fix agent for a missing HEAD run" \
  "launching fix agent" "$EV"
assert_not_contains "ci_gate does not sit on the empty-branch wait line" \
  "No workflow runs found yet" "$OUT"

# ── ci_gate: failing HEAD still launches the fix agent ───────────────────────
echo ""
echo "── ci_gate: failing HEAD still launches the fix agent ──"
reset_traces
export ALUCARD_TEST_RUN_MODE=head_failure
CI_CHECKS_AVAILABLE=false
MEASURE_CI_RESULT=""
set +e
ci_gate 1 feature "$REMOTE" >"$TEST_DIR/out-fail" 2>&1
set -e
EV=$(<"$EVENTS")
assert_contains "a failed HEAD run launches the fix agent" \
  "launching fix agent" "$EV"
assert_contains "exhausted attempts still leave the PR open" \
  "still failing after 3 fix attempts" "$EV"
assert_eq "MEASURE_CI_RESULT records the failure" "failed" "$MEASURE_CI_RESULT"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
