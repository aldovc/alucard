#!/bin/bash
# Covers scan_agent_errors: the post-run pass that prints provider-reported
# errors to stderr, tagged with role and exit code, so a degraded run (e.g.
# codex falling back on unknown model metadata) is never silently missed.
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"

PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

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

assert_empty() {
  local label="$1" actual="$2"
  if [ -z "$actual" ]; then
    pass "$label"
  else
    fail "$label (expected empty, got '$actual')"
  fi
}

# Source alucard to load helper functions without running main.
# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_error_scan.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── codex: a real fallback-metadata error item ───────────────────────────────
echo "── scan_agent_errors: codex ──"

codex_log="$TMP_ROOT/codex-error.jsonl"
cat > "$codex_log" <<'EOF'
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"Model metadata for `gpt-5.6-terra` not found. Defaulting to fallback metadata; this can degrade performance and cause issues."}}
{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"hello"}}
EOF

out=$(jq --unbuffered -rj "$CODEX_STREAM_TEXT" < "$codex_log")
  '⚠ codex error: Model metadata for `gpt-5.6-terra` not found' "$out"
  "⚠ codex error: Model metadata for `gpt-5.6-terra` not found" "$out"

out=$(scan_agent_errors "fix-3" "$codex_log" "codex" "0" 2>&1)
assert_contains "codex error item reaches stderr" \
  "Model metadata for \`gpt-5.6-terra\` not found" "$out"
assert_contains "codex error output is tagged with role" "fix-3" "$out"
assert_contains "codex error output is tagged with exit code" "rc=0" "$out"

# ── codex: a clean log has nothing to report ────────────────────────────────
echo "── scan_agent_errors: codex clean log ──"

clean_log="$TMP_ROOT/codex-clean.jsonl"
cat > "$clean_log" <<'EOF'
{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"all good"}}
{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"ls -la"}}
EOF

out=$(scan_agent_errors "iter-3" "$clean_log" "codex" "0" 2>&1)
assert_empty "clean codex log produces no stderr output" "$out"

# ── claude: a non-success final result line ─────────────────────────────────
echo "── scan_agent_errors: claude ──"

claude_log="$TMP_ROOT/claude-error.jsonl"
cat > "$claude_log" <<'EOF'
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"working"}}}
{"type":"result","subtype":"error_max_turns","is_error":true,"result":"Reached max turns without completing the task."}
EOF

out=$(scan_agent_errors "review-3-1" "$claude_log" "claude" "1" 2>&1)
assert_contains "claude error result reaches stderr" \
  "Reached max turns without completing the task." "$out"
assert_contains "claude error output is tagged with role" "review-3-1" "$out"
assert_contains "claude error output is tagged with exit code" "rc=1" "$out"
assert_contains "claude error output carries the subtype" "error_max_turns" "$out"

# ── claude: a successful final result line reports nothing ─────────────────
echo "── scan_agent_errors: claude success ──"

claude_ok_log="$TMP_ROOT/claude-ok.jsonl"
cat > "$claude_ok_log" <<'EOF'
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"done"}}}
{"type":"result","subtype":"success","is_error":false,"result":"All good."}
EOF

out=$(scan_agent_errors "iter-4" "$claude_ok_log" "claude" "0" 2>&1)
assert_empty "successful claude result produces no stderr output" "$out"

# ── missing log file is not fatal ───────────────────────────────────────────
echo "── scan_agent_errors: missing log ──"

_rc=0
scan_agent_errors "iter-5" "$TMP_ROOT/does-not-exist.jsonl" "codex" "0" > /dev/null 2>&1 || _rc=$?
if [ "$_rc" -eq 0 ]; then
  pass "missing log file is not fatal"
else
  fail "missing log file is not fatal (rc=$_rc)"
fi

# ── billing_error abort grep is untouched by the error scan ────────────────
echo "── billing_error detection still works ──"

billing_log="$TMP_ROOT/billing.jsonl"
cat > "$billing_log" <<'EOF'
{"type":"result","subtype":"success","is_error":false,"error":"billing_error","result":"insufficient credit"}
EOF

if grep -q '"error":"billing_error"' "$billing_log"; then
  pass "billing_error grep still matches its fixture"
else
  fail "billing_error grep still matches its fixture"
fi
assert_not_contains "codex_stream_text filter is a separate concern from billing_error" \
  "billing_error" "$(scan_agent_errors "iter-6" "$billing_log" "claude" "0" 2>&1)"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
