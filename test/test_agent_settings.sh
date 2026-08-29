#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"
SETTINGS_ENV="$SCRIPT_DIR/fixtures/agent-settings.env"

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

assert_flag_value() {
  local label="$1" flag="$2" expected="$3" file="$4" actual
  actual=$(awk -v flag="$flag" '$0 == flag { getline; print; exit }' "$file")
  assert_eq "$label" "$expected" "$actual"
}

TEST_TMPDIR=$(mktemp -d)
trap 'rm -rf "$TEST_TMPDIR"' EXIT

unset ALUCARD_PROVIDER ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
      ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER

unset ALUCARD_WORKER_MAX_TURNS ALUCARD_WORKER_MAX_BUDGET \
      ALUCARD_CI_FIX_MAX_TURNS ALUCARD_CI_FIX_MAX_BUDGET \
      ALUCARD_REVIEWER_MAX_TURNS ALUCARD_REVIEWER_MAX_BUDGET \
      ALUCARD_FEEDBACK_MAX_TURNS ALUCARD_FEEDBACK_MAX_BUDGET \
      ALUCARD_CLAUDE_MODEL ALUCARD_CLAUDE_FALLBACK_MODEL \
      ALUCARD_TRANSPORT_RETRY_ATTEMPTS

# Source alucard to load helper functions without running main.
# The BASH_SOURCE guard in alucard prevents main from executing when sourced.
# shellcheck disable=SC1090
source "$ALUCARD"

# Replace timeout so invoke_agent's fully constructed Claude command can be
# checked without starting Docker. It emits one stream event for the pipeline.
FAKE_BIN="$TEST_TMPDIR/bin"
mkdir -p "$FAKE_BIN"
cat > "$FAKE_BIN/timeout" <<'EOF_TIMEOUT'
#!/bin/bash
set -euo pipefail
shift
printf '%s\n' "$@" > "$ALUCARD_TEST_CAPTURE"
printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"ok"}}}'
EOF_TIMEOUT
chmod +x "$FAKE_BIN/timeout"
PATH="$FAKE_BIN:$PATH"

TIMEOUT_MIN=1
ENV_FILE="$SETTINGS_ENV"
IMAGE="test-image"
stream_text='.event.delta.text'

invoke_and_capture() {
  local role="$1" max_turns="$2" max_budget="$3"
  ALUCARD_TEST_CAPTURE="$TEST_TMPDIR/${role}.args"
  export ALUCARD_TEST_CAPTURE
  invoke_agent "$role" "$TEST_TMPDIR/${role}.log" "$max_turns" "$max_budget" prompt > /dev/null
}

assert_claude_args() {
  local role="$1" max_turns="$2" max_budget="$3" model="$4" fallback_model="$5"
  local capture="$TEST_TMPDIR/${role}.args"
  assert_flag_value "$role uses configured max turns" --max-turns "$max_turns" "$capture"
  assert_flag_value "$role uses configured max budget" --max-budget-usd "$max_budget" "$capture"
  assert_flag_value "$role uses configured model" --model "$model" "$capture"
  assert_flag_value "$role uses configured fallback model" --fallback-model "$fallback_model" "$capture"
}

# ── Test group 1: defaults reach Claude arguments ────────────────────────────

invoke_and_capture iter-default "$DEFAULT_WORKER_MAX_TURNS" "$DEFAULT_WORKER_MAX_BUDGET"
assert_claude_args iter-default 180 10 sonnet haiku

# ── Test group 2: selected env-file overrides reach all Claude roles ─────────

load_env_file "$SETTINGS_ENV" true

invoke_and_capture iter-override "$DEFAULT_WORKER_MAX_TURNS" "$DEFAULT_WORKER_MAX_BUDGET"
assert_claude_args iter-override 181 11 opus sonnet

invoke_and_capture fix-override "$DEFAULT_CI_FIX_MAX_TURNS" "$DEFAULT_CI_FIX_MAX_BUDGET"
assert_claude_args fix-override 31 3 opus sonnet

invoke_and_capture review-override "$DEFAULT_REVIEWER_MAX_TURNS" "$DEFAULT_REVIEWER_MAX_BUDGET"
assert_claude_args review-override 21 4 opus sonnet

invoke_and_capture feedback-override "$DEFAULT_FEEDBACK_MAX_TURNS" "$DEFAULT_FEEDBACK_MAX_BUDGET"
assert_claude_args feedback-override 51 5 opus sonnet

# ── Test group 3: transport retry attempts are validated before arithmetic ───

assert_eq "transport retries add one initial attempt" "4" "$(transport_retry_total_attempts 3)"
assert_eq "zero transport retries keeps the initial attempt" "1" "$(transport_retry_total_attempts 0)"

assert_invalid_transport_retries() {
  local label="$1" value="$2" output
  if output=$(transport_retry_total_attempts "$value" 2>&1); then
    fail "$label (expected validation failure, got '$output')"
  elif [[ "$output" == *"ALUCARD_TRANSPORT_RETRY_ATTEMPTS must be a non-negative integer"* ]]; then
    pass "$label"
  else
    fail "$label (unexpected error '$output')"
  fi
}

assert_invalid_transport_retries "negative transport retries are rejected" "-1"
assert_invalid_transport_retries "malformed transport retries are rejected" "two"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
