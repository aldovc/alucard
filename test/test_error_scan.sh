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
assert_contains "live codex stream emits error warning" \
  '⚠ codex error: Model metadata for `gpt-5.6-terra` not found' "$out"

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

# ── claude: subtype error independently reaches stderr ──────────────────────
echo "── scan_agent_errors: claude subtype error ──"

claude_log="$TMP_ROOT/claude-error.jsonl"
cat > "$claude_log" <<'EOF'
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"working"}}}
{"type":"result","subtype":"error_max_turns","is_error":false,"result":"Reached max turns without completing the task."}
EOF

out=$(scan_agent_errors "review-3-1" "$claude_log" "claude" "1" 2>&1)
assert_contains "claude error result reaches stderr" \
  "Reached max turns without completing the task." "$out"
assert_contains "claude error output is tagged with role" "review-3-1" "$out"
assert_contains "claude error output is tagged with exit code" "rc=1" "$out"
assert_contains "claude error output carries the subtype" "error_max_turns" "$out"

# ── claude: is_error independently reaches stderr ──────────────────────────
echo "── scan_agent_errors: claude is_error ──"

claude_is_error_log="$TMP_ROOT/claude-is-error.jsonl"
cat > "$claude_is_error_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"result":"Provider marked the result as an error."}
EOF

out=$(scan_agent_errors "review-3-2" "$claude_is_error_log" "claude" "1" 2>&1)
assert_contains "claude is_error result reaches stderr" \
  "Provider marked the result as an error." "$out"
assert_contains "claude is_error output is tagged with role" "review-3-2" "$out"
assert_contains "claude is_error output carries the subtype" "success" "$out"

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

# ── classify_agent_failure ──────────────────────────────────────────────────
echo "── classify_agent_failure ──"

wedge_marker_log="$TMP_ROOT/wedge.jsonl"
cat > "$wedge_marker_log" <<'EOF'
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Shell cwd was reset to /work (deleted)"}}}
EOF

out=$(classify_agent_failure "$wedge_marker_log" "claude" "137")
assert_eq "wedge marker classifies as wedged" "wedged" "$out"

out=$(classify_agent_failure "$wedge_marker_log" "claude" "0")
assert_eq "wedge marker outranks a clean exit code" "wedged" "$out"

out=$(classify_agent_failure "$claude_ok_log" "claude" "124")
assert_eq "timeout exit code (124) classifies as exhausted" "exhausted" "$out"

out=$(classify_agent_failure "$claude_ok_log" "claude" "137")
assert_eq "hard-kill exit code (137) classifies as exhausted" "exhausted" "$out"

assert_eq "wedge marker beats a kill-signal exit code (ordering)" "wedged" \
  "$(classify_agent_failure "$wedge_marker_log" "claude" "137")"

claude_max_turns_log="$TMP_ROOT/claude-max-turns.jsonl"
cat > "$claude_max_turns_log" <<'EOF'
{"type":"result","subtype":"error_max_turns","is_error":false,"result":"Reached max turns without completing the task."}
EOF

out=$(classify_agent_failure "$claude_max_turns_log" "claude" "1")
assert_eq "claude max-turns result classifies as exhausted" "exhausted" "$out"

claude_transport_log="$TMP_ROOT/claude-transport.jsonl"
cat > "$claude_transport_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"result":"API Error: Connection closed mid-response"}
EOF

out=$(classify_agent_failure "$claude_transport_log" "claude" "1")
assert_eq "claude connection-closed result classifies as transport" "transport" "$out"

claude_overload_log="$TMP_ROOT/claude-overload.jsonl"
cat > "$claude_overload_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"result":"Overloaded: the API is temporarily overloaded, please retry"}
EOF

out=$(classify_agent_failure "$claude_overload_log" "claude" "1")
assert_eq "claude overload/rate-limit result classifies as transport" "transport" "$out"

out=$(classify_agent_failure "$claude_ok_log" "claude" "0")
assert_eq "a clean claude log with rc=0 classifies as clean" "clean" "$out"

claude_unrecognized_log="$TMP_ROOT/claude-unrecognized.jsonl"
cat > "$claude_unrecognized_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"result":"The task could not be completed."}
EOF

out=$(classify_agent_failure "$claude_unrecognized_log" "claude" "1")
assert_eq "a contradictory Claude result record classifies as transport regardless of result text" "transport" "$out"

# ── #62: a worker's own summary must not be misread as a transport drop ────
echo "── classify_agent_failure: worker prose vs. real transport signals ──"

claude_self_describing_log="$TMP_ROOT/claude-self-describing.jsonl"
cat > "$claude_self_describing_log" <<'EOF'
{"type":"result","subtype":"success","is_error":false,"result":"Implemented classify_agent_failure. Transport failures (connection closed, overloaded, rate limit) now retry with a fresh worktree instead of burning the ticket. All 290 tests pass."}
EOF

out=$(classify_agent_failure "$claude_self_describing_log" "claude" "1")
assert_eq "a normal completion whose prose mentions transport words is not misread as transport" \
  "failed" "$out"

claude_http_status_log="$TMP_ROOT/claude-http-status.jsonl"
cat > "$claude_http_status_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"api_error_status":429,"result":"Too many requests."}
EOF

out=$(classify_agent_failure "$claude_http_status_log" "claude" "1")
assert_eq "api_error_status 429 classifies as transport regardless of result text" "transport" "$out"

claude_5xx_status_log="$TMP_ROOT/claude-5xx-status.jsonl"
cat > "$claude_5xx_status_log" <<'EOF'
{"type":"result","subtype":"error","is_error":true,"api_error_status":503,"result":"Service unavailable."}
EOF

out=$(classify_agent_failure "$claude_5xx_status_log" "claude" "1")
assert_eq "api_error_status 503 classifies as transport regardless of result text" "transport" "$out"

claude_incident_log="$TMP_ROOT/claude-incident.jsonl"
cat > "$claude_incident_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"api_error_status":null,"result":"API Error: Connection closed mid-response. The response above may be incomplete."}
EOF

out=$(classify_agent_failure "$claude_incident_log" "claude" "1")
assert_eq "the #58 incident record still classifies as transport" "transport" "$out"

out=$(classify_agent_failure "$codex_log" "codex" "1")
assert_eq "codex error item (fallback metadata) classifies as failed" "failed" "$out"

codex_transport_log="$TMP_ROOT/codex-transport.jsonl"
cat > "$codex_transport_log" <<'EOF'
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"stream error: connection closed before message completed"}}
EOF

out=$(classify_agent_failure "$codex_transport_log" "codex" "1")
assert_eq "codex connection-closed error item classifies as transport" "transport" "$out"

codex_truncated_log="$TMP_ROOT/codex-truncated.jsonl"
cat > "$codex_truncated_log" <<'EOF'
{"type":"item.completed","item":{"id":"item_0","type":"error","message":"connection closed"
EOF
classifier_stderr="$TMP_ROOT/classifier-stderr"
out=$(classify_agent_failure "$codex_truncated_log" "codex" "1" 2>"$classifier_stderr")
assert_eq "malformed codex JSONL classifies as failed" "failed" "$out"
assert_empty "malformed codex JSONL produces no classifier stderr" "$(<"$classifier_stderr")"

claude_truncated_log="$TMP_ROOT/claude-truncated.jsonl"
cat > "$claude_truncated_log" <<'EOF'
{"type":"result","subtype":"success","is_error":true,"result":"connection closed"
EOF
out=$(classify_agent_failure "$claude_truncated_log" "claude" "1" 2>"$classifier_stderr")
assert_eq "malformed claude JSONL classifies as failed" "failed" "$out"
assert_empty "malformed claude JSONL produces no classifier stderr" "$(<"$classifier_stderr")"

out=$(classify_agent_failure "$clean_log" "codex" "0")
assert_eq "a clean codex log with rc=0 classifies as clean" "clean" "$out"

out=$(classify_agent_failure "$TMP_ROOT/does-not-exist.jsonl" "claude" "0")
assert_eq "a missing log with rc=0 classifies as clean" "clean" "$out"

out=$(classify_agent_failure "$TMP_ROOT/does-not-exist.jsonl" "claude" "1")
assert_eq "a missing log with a non-zero rc classifies as failed" "failed" "$out"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
