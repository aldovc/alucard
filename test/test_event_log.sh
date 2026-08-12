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
    printf '%s\n' '{}'
    exit "${ALUCARD_TEST_DOCKER_RUN_EXIT:-0}"
    ;;
  rm|kill)
    exit 0
    ;;
esac
MOCK

cat > "$MOCK_BIN/jq" <<'MOCK'
#!/bin/bash
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

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
