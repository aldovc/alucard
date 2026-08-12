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

assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    pass "$label"
  else
    fail "$label (missing '$needle')"
  fi
}

assert_matches() {
  local label="$1" pattern="$2" actual="$3"
  if printf '%s' "$actual" | grep -qE -- "$pattern"; then
    pass "$label"
  else
    fail "$label ('$actual' does not match /$pattern/)"
  fi
}

# Source alucard to load helper functions without running main.
# The BASH_SOURCE guard in alucard prevents main from executing when sourced.
# shellcheck disable=SC1090
source "$ALUCARD"

TEST_DIR=$(mktemp -d)
cleanup() { rm -rf "$TEST_DIR"; }
trap cleanup EXIT

# ── Test group 1: log_event basic shape ──────────────────────────────────────

LOG_DIR="$TEST_DIR/logs1"
mkdir -p "$LOG_DIR"

out=$(log_event "hello world")
assert_matches "log_event prints HH:MM:SS <msg> to stdout" \
  '^[0-9]{2}:[0-9]{2}:[0-9]{2} hello world$' "$out"

assert_eq "events.log has exactly one line after one call" \
  "1" "$(wc -l < "$LOG_DIR/events.log" | tr -d ' ')"

file_line=$(cat "$LOG_DIR/events.log")
assert_matches "events.log line is ISO-8601-UTC<TAB>msg" \
  $'^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z\thello world$' "$file_line"

log_event "second event" >/dev/null
assert_eq "events.log is one event per line (appends, not overwrites)" \
  "2" "$(wc -l < "$LOG_DIR/events.log" | tr -d ' ')"

# ── Test group 2: missing LOG_DIR is a no-op on the file side ────────────────

unset LOG_DIR || true
out=$(log_event "no log dir")
assert_eq "log_event with LOG_DIR unset still exits 0" "0" "$?"
assert_contains "log_event with LOG_DIR unset still prints to stdout" "no log dir" "$out"

LOG_DIR="$TEST_DIR/does-not-exist"
out=$(log_event "nonexistent log dir")
assert_contains "log_event with a nonexistent LOG_DIR still prints to stdout" \
  "nonexistent log dir" "$out"
if [ -e "$LOG_DIR" ]; then
  fail "log_event must not create LOG_DIR itself (command_queue never sets one up)"
else
  pass "log_event does not create a missing LOG_DIR"
fi

# ── Test group 3: invoke_agent logs rc + duration, flags 124/137 as timeout ──

MOCK_BIN="$TEST_DIR/bin"
TRACE_FILE="$TEST_DIR/trace"
mkdir -p "$MOCK_BIN"
touch "$TRACE_FILE"

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then
  shift 2
fi
shift
exec "$@"
MOCK

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "$1" in
  run)
    printf '%s\n' "${ALUCARD_TEST_DOCKER_RUN_OUTPUT:-{}}"
    exit "${ALUCARD_TEST_DOCKER_RUN_EXIT:-0}"
    ;;
  rm|kill)
    exit 0
    ;;
esac
MOCK

cat > "$MOCK_BIN/jq" <<'MOCK'
#!/bin/bash
if [ "${1:-}" = "length" ]; then
  echo 1
  exit 0
fi
cat
MOCK

chmod +x "$MOCK_BIN/timeout" "$MOCK_BIN/docker" "$MOCK_BIN/jq"

export PATH="$MOCK_BIN:$PATH"
TIMEOUT_MIN=1
ENV_FILE="$TEST_DIR/alucard.env"
IMAGE="alucard-test-image"
stream_text='.'
codex_stream_text='.'
touch "$ENV_FILE"

LOG_DIR="$TEST_DIR/logs3"
mkdir -p "$LOG_DIR"

run_and_get_event() {
  local docker_exit="$1" role="$2" log_file="$3"
  export ALUCARD_TEST_DOCKER_RUN_EXIT="$docker_exit"
  export ALUCARD_WORKER_PROVIDER="claude"
  invoke_agent "$role" "$log_file" 10 1 'test prompt' >/dev/null 2>&1 || true
  unset ALUCARD_TEST_DOCKER_RUN_EXIT ALUCARD_WORKER_PROVIDER
  grep "Agent ${role} end:" "$LOG_DIR/events.log" | tail -1
}

: > "$LOG_DIR/events.log"
event=$(run_and_get_event 0 "iter-ok" "$TEST_DIR/ok.jsonl")
assert_matches "clean exit logs rc=0 and a numeric duration" \
  'Agent iter-ok end: rc=0 duration=[0-9]+s$' "$event"

event=$(run_and_get_event 124 "iter-timeout" "$TEST_DIR/timeout.jsonl")
assert_contains "rc=124 is annotated as a timeout" "rc=124" "$event"
assert_contains "rc=124 is annotated as a timeout" "(timeout)" "$event"

event=$(run_and_get_event 137 "iter-killed" "$TEST_DIR/killed.jsonl")
assert_contains "rc=137 is annotated as a timeout" "rc=137" "$event"
assert_contains "rc=137 is annotated as a timeout" "(timeout)" "$event"

event=$(run_and_get_event 3 "iter-err" "$TEST_DIR/err.jsonl")
assert_contains "an ordinary failure rc is reported" "rc=3" "$event"
if printf '%s' "$event" | grep -qF "(timeout)"; then
  fail "an ordinary failure rc is not mislabeled as a timeout"
else
  pass "an ordinary failure rc is not mislabeled as a timeout"
fi

# ── Test group 4: billing abort closes the active iteration ─────────────────

BILLING_REPO="$TEST_DIR/billing-repo"
BILLING_REMOTE="$TEST_DIR/billing-remote.git"
mkdir -p "$BILLING_REPO"
git -C "$BILLING_REPO" init --initial-branch=main >/dev/null
git -C "$BILLING_REPO" config user.email test@example.com
git -C "$BILLING_REPO" config user.name "Alucard Test"
touch "$BILLING_REPO/README.md"
git -C "$BILLING_REPO" add README.md
git -C "$BILLING_REPO" commit -m "Initial commit" >/dev/null
git init --bare "$BILLING_REMOTE" >/dev/null
git -C "$BILLING_REPO" remote add origin "$BILLING_REMOTE"
git -C "$BILLING_REPO" push -u origin main >/dev/null

need_cmd() { :; }
parse_repo_options() {
  REPO_ABS="$BILLING_REPO"
  BASE_BRANCH="main"
  ITERATIONS=1
  TIMEOUT_MIN=1
  IMAGE="alucard-test-image"
}
setup_run_environment() {
  LOG_DIR="$TEST_DIR/billing-logs"
  WT_ROOT="$TEST_DIR/billing-worktrees"
  ENV_FILE="$TEST_DIR/alucard.env"
  CREATED_WORKTREES=()
  mkdir -p "$LOG_DIR" "$WT_ROOT"
}
resolve_task_source() { TASK_SOURCE="github"; }
get_task_queue() { printf '%s\n' '[{}]'; }
build_worker_prompt() { FULL_PROMPT="test prompt"; }

export ALUCARD_TEST_DOCKER_RUN_EXIT=7
export ALUCARD_TEST_DOCKER_RUN_OUTPUT='{"error":"billing_error"}'
export ALUCARD_WORKER_PROVIDER="claude"
set +e
(command_run) >/dev/null 2>&1
billing_rc=$?
set -e
unset ALUCARD_TEST_DOCKER_RUN_EXIT ALUCARD_TEST_DOCKER_RUN_OUTPUT ALUCARD_WORKER_PROVIDER

assert_eq "billing error aborts the run with rc=2" "2" "$billing_rc"
mapfile -t billing_events < <(cut -f2- "$LOG_DIR/events.log" | grep -E \
  '^(Iteration 1/1 start|Agent iter-1 end:|Iteration 1/1 end:|Run end: provider returned billing_error)')
assert_eq "billing abort emits four iteration/run transitions" "4" "${#billing_events[@]}"
assert_eq "billing abort starts the iteration first" \
  "Iteration 1/1 start" "${billing_events[0]:-}"
assert_matches "billing abort records the worker end before cleanup" \
  '^Agent iter-1 end: rc=7 duration=[0-9]+s$' "${billing_events[1]:-}"
assert_matches "billing abort closes the iteration with the worker rc" \
  '^Iteration 1/1 end: rc=7 duration=[0-9]+s$' "${billing_events[2]:-}"
assert_eq "billing abort records the run end last" \
  "Run end: provider returned billing_error (credit balance too low) — aborting run." \
  "${billing_events[3]:-}"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
