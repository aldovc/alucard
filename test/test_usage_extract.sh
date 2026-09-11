#!/bin/bash
# Covers extract_agent_usage, the shared jq extraction behind
# print_usage_report and post_iteration_usage, for both agent-log formats:
# claude's single trailing "type":"result" line and codex's summed
# "type":"turn.completed" stream (no cost field).
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

# Source alucard to load helper functions without running main.
# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_usage_extract.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── Test group 1: claude-format log ──────────────────────────────────────────
echo "── claude format ──"

CLAUDE_LOG="$TMP_ROOT/claude.jsonl"
cat > "$CLAUDE_LOG" <<'EOF'
{"type":"system","subtype":"init"}
{"type":"result","subtype":"success","num_turns":42,"total_cost_usd":1.234,"modelUsage":{"claude-sonnet-4-5":{"inputTokens":1000,"outputTokens":2000,"cacheReadInputTokens":50000,"cacheCreationInputTokens":9000}}}
EOF

assert_eq "claude log: exact TSV" \
  "$(printf '1000\t2000\t50000\t9000\t42\t1.234')" \
  "$(extract_agent_usage "$CLAUDE_LOG")"

# ── Test group 2: codex-format log ───────────────────────────────────────────
echo "── codex format ──"

CODEX_LOG="$TMP_ROOT/codex.jsonl"
cat > "$CODEX_LOG" <<'EOF'
{"type":"turn.completed","usage":{"input_tokens":1291543,"cached_input_tokens":1218798,"output_tokens":15363,"reasoning_output_tokens":5497}}
{"type":"item.completed","item":{"id":"item_3","type":"command_execution","command":"git status"}}
{"type":"item.completed","item":{"id":"item_4","type":"agent_message"}}
EOF

# input = input_tokens - cached_input_tokens = 1291543 - 1218798 = 72745
# turns = count of item.completed events with .item.type == command_execution
assert_eq "codex log: exact TSV, cost empty" \
  "$(printf '72745\t15363\t1218798\t0\t1\t')" \
  "$(extract_agent_usage "$CODEX_LOG")"

# ── Test group 2b: codex-cli 0.147+ log carrying cache_write_input_tokens ───
echo "── codex format, 0.147+ cache-write field ──"

CODEX_CW_LOG="$TMP_ROOT/codex_cw.jsonl"
cat > "$CODEX_CW_LOG" <<'EOF'
{"type":"turn.completed","usage":{"input_tokens":10817,"cached_input_tokens":0,"cache_write_input_tokens":10814,"output_tokens":5,"reasoning_output_tokens":0}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution"}}
EOF

assert_eq "codex 0.147 log: cache-write column populated" \
  "$(printf '10817\t5\t0\t10814\t1\t')" \
  "$(extract_agent_usage "$CODEX_CW_LOG")"

# ── Test group 3: codex log summed across multiple turn.completed events ────
echo "── codex format, multiple turns ──"

CODEX_MULTI_LOG="$TMP_ROOT/codex_multi.jsonl"
cat > "$CODEX_MULTI_LOG" <<'EOF'
{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution"}}
{"type":"turn.completed","usage":{"input_tokens":2000,"cached_input_tokens":600,"output_tokens":200}}
{"type":"item.completed","item":{"id":"item_2","type":"command_execution"}}
{"type":"item.completed","item":{"id":"item_3","type":"error"}}
EOF

# input = (1000 + 2000) - (400 + 600) = 2000; turns = 2 command_execution items
assert_eq "codex multi-turn log: summed TSV" \
  "$(printf '2000\t300\t1000\t0\t2\t')" \
  "$(extract_agent_usage "$CODEX_MULTI_LOG")"

# ── Test group 4: unparsable log yields empty output ────────────────────────
echo "── unparsable / missing logs ──"

EMPTY_LOG="$TMP_ROOT/empty.jsonl"
: > "$EMPTY_LOG"
assert_eq "empty log: no output" "" "$(extract_agent_usage "$EMPTY_LOG")"

NOISE_LOG="$TMP_ROOT/noise.jsonl"
cat > "$NOISE_LOG" <<'EOF'
{"type":"system","subtype":"init"}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message"}}
EOF
assert_eq "log with neither format: no output" "" "$(extract_agent_usage "$NOISE_LOG")"

assert_eq "missing log file: no output" "" "$(extract_agent_usage "$TMP_ROOT/does-not-exist.jsonl")"

# ── Test group 5: the posted comment is recognisable as ours ────────────────
echo "── posted comment prefix ──"

# The human-vs-bot filters key off the body's opening prefix. A usage comment
# that does not carry it is handed to the feedback agent as human input, and on
# `continue` it reads as a human replying after the reviewer asked for changes.
CAPTURE="$TMP_ROOT/comment-body"
MOCK_BIN="$TMP_ROOT/bin"; mkdir -p "$MOCK_BIN"
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
case "$1 $2" in
  "pr list") printf '7\n' ;;
  "pr view") printf '\n' ;;
  "pr comment"|"issue comment")
    while [ "$#" -gt 0 ]; do
      [ "$1" = "--body" ] && { printf '%s' "$2" > "$ALUCARD_TEST_CAPTURE"; break; }
      shift
    done ;;
esac
exit 0
MOCK
chmod +x "$MOCK_BIN/gh"

RUN_DIR="$TMP_ROOT/run"; mkdir -p "$RUN_DIR"
cp "$CLAUDE_LOG" "$RUN_DIR/iter-1.jsonl"
PATH="$MOCK_BIN:$PATH" ALUCARD_TEST_CAPTURE="$CAPTURE" \
  post_iteration_usage 1 some-branch "$TMP_ROOT" "$RUN_DIR" >/dev/null

posted=$(cat "$CAPTURE" 2>/dev/null || true)
case "$posted" in
  "$BOT_COMMENT_PREFIX"*) pass "the usage comment opens with the wrapper prefix" ;;
  *) fail "the usage comment opens with the wrapper prefix (got '${posted:0:40}')" ;;
esac

excluded=$(jq -nr --arg body "$posted" --arg prefix "$BOT_COMMENT_PREFIX" \
  '[$body] | map(select(startswith($prefix) | not)) | length')
assert_eq "so the human-comment filter drops it" "0" "$excluded"

# Nothing may keep its own copy of the marker: that is how the writers and the
# filters drifted apart last time. Matching the emoji alone is deliberate — a
# writer that drifts to some other wording is precisely the case a search for
# the current prefix would miss.
assert_eq "no comment body spells the marker out for itself" \
  "1" "$(grep -c '🤖' "$ALUCARD")"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
