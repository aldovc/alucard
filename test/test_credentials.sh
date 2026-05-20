#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"

PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# Run function in subshell; assert its exit code matches EXPECTED_EXIT.
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

# ── Env file fixtures ─────────────────────────────────────────────────────────

TEST_TMPDIR=$(mktemp -d)
trap 'rm -rf "$TEST_TMPDIR"' EXIT

CLAUDE_ENV="$TEST_TMPDIR/claude.env"
CODEX_ENV="$TEST_TMPDIR/codex.env"
BOTH_ENV="$TEST_TMPDIR/both.env"
NONE_ENV="$TEST_TMPDIR/none.env"

printf 'GITHUB_TOKEN=gh_test\nANTHROPIC_API_KEY=sk-ant-test\n'                               > "$CLAUDE_ENV"
printf 'GITHUB_TOKEN=gh_test\nOPENAI_API_KEY=sk-oai-test\n'                                  > "$CODEX_ENV"
printf 'GITHUB_TOKEN=gh_test\nANTHROPIC_API_KEY=sk-ant-test\nOPENAI_API_KEY=sk-oai-test\n'  > "$BOTH_ENV"
printf 'GITHUB_TOKEN=gh_test\n'                                                               > "$NONE_ENV"

# ── Provider helpers called inside subshells via assert_exit ──────────────────

all_claude() {
  unset ALUCARD_PROVIDER ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
        ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER
  unset ANTHROPIC_API_KEY OPENAI_API_KEY
}

all_codex() {
  export ALUCARD_PROVIDER=codex
  unset ALUCARD_WORKER_PROVIDER ALUCARD_CI_FIX_PROVIDER \
        ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER
  unset ANTHROPIC_API_KEY OPENAI_API_KEY
}

# WORKER=codex, all others default to claude
mixed() {
  unset ALUCARD_PROVIDER ALUCARD_CI_FIX_PROVIDER \
        ALUCARD_REVIEWER_PROVIDER ALUCARD_FEEDBACK_PROVIDER
  export ALUCARD_WORKER_PROVIDER=codex
  unset ANTHROPIC_API_KEY OPENAI_API_KEY
}

# ── Test group 1: claude-only config (default) ───────────────────────────────

t_claude_pass()         { all_claude; load_env_file "$CLAUDE_ENV" true; }
t_claude_no_ant_fails() { all_claude; load_env_file "$NONE_ENV"   true; }
t_claude_no_oai_ok()    { all_claude; load_env_file "$CLAUDE_ENV" true; }  # OPENAI not needed

assert_exit "claude-only: ANTHROPIC_API_KEY present passes"            0 t_claude_pass
assert_exit "claude-only: missing ANTHROPIC_API_KEY fails"             1 t_claude_no_ant_fails
assert_exit "claude-only: absent OPENAI_API_KEY is not required"       0 t_claude_no_oai_ok

# ── Test group 2: codex-only config ──────────────────────────────────────────

t_codex_pass()          { all_codex; load_env_file "$CODEX_ENV"  true; }
t_codex_no_oai_fails()  { all_codex; load_env_file "$NONE_ENV"   true; }
t_codex_no_ant_ok()     { all_codex; load_env_file "$CODEX_ENV"  true; }  # ANTHROPIC not needed

assert_exit "codex-only: OPENAI_API_KEY present passes"                0 t_codex_pass
assert_exit "codex-only: missing OPENAI_API_KEY fails"                 1 t_codex_no_oai_fails
assert_exit "codex-only: absent ANTHROPIC_API_KEY is not required"     0 t_codex_no_ant_ok

# ── Test group 3: mixed config (both providers active) ───────────────────────

t_mixed_both_pass()     { mixed; load_env_file "$BOTH_ENV"  true; }
t_mixed_no_oai_fails()  { mixed; load_env_file "$CLAUDE_ENV" true; }  # OPENAI missing
t_mixed_no_ant_fails()  { mixed; load_env_file "$CODEX_ENV"  true; }  # ANTHROPIC missing

assert_exit "mixed: both keys present passes"                          0 t_mixed_both_pass
assert_exit "mixed: missing OPENAI_API_KEY fails"                      1 t_mixed_no_oai_fails
assert_exit "mixed: missing ANTHROPIC_API_KEY fails"                   1 t_mixed_no_ant_fails

# ── Test group 4: missing key for the active provider ────────────────────────

t_claude_missing_key()  { all_claude; load_env_file "$NONE_ENV"  true; }
t_codex_missing_key()   { all_codex;  load_env_file "$NONE_ENV"  true; }

assert_exit "active claude provider: missing ANTHROPIC_API_KEY exits 1" 1 t_claude_missing_key
assert_exit "active codex provider: missing OPENAI_API_KEY exits 1"    1 t_codex_missing_key

# ── Helpers: assert presence/absence of a pattern in command output ──────────

assert_output_contains() {
  local label="$1" pattern="$2"
  shift 2
  local out
  out=$(( "$@" ) 2>&1) || true
  if echo "$out" | grep -qF "$pattern"; then
    pass "$label"
  else
    fail "$label (expected '$pattern' not found in output)"
  fi
}

assert_output_absent() {
  local label="$1" pattern="$2"
  shift 2
  local out
  out=$(( "$@" ) 2>&1) || true
  if ! echo "$out" | grep -qF "$pattern"; then
    pass "$label"
  else
    fail "$label (unexpected '$pattern' found in output)"
  fi
}

# ── Test group 5: command_doctor credential output ────────────────────────────
# command_doctor is called with --env-file to point at our fixtures.
# Only credential-related output lines are asserted; unrelated doctor checks
# (missing docker, tool files, etc.) may emit "missing:" lines but don't exit
# early, so the credential section is always reached when the env file exists.

td_claude()       { all_claude; command_doctor --env-file "$CLAUDE_ENV"; }
td_codex()        { all_codex;  command_doctor --env-file "$CODEX_ENV";  }
td_mixed()        { mixed;      command_doctor --env-file "$BOTH_ENV";   }
td_codex_none()   { all_codex;  command_doctor --env-file "$NONE_ENV";   }

assert_output_contains "doctor claude-only: ok ANTHROPIC_API_KEY present"          "ok: ANTHROPIC_API_KEY"         td_claude
assert_output_absent   "doctor claude-only: OPENAI_API_KEY not checked"            "OPENAI_API_KEY"                td_claude
assert_output_contains "doctor codex-only: ok OPENAI_API_KEY present"              "ok: OPENAI_API_KEY"            td_codex
assert_output_absent   "doctor codex-only: ANTHROPIC_API_KEY not checked"          "ANTHROPIC_API_KEY"             td_codex
assert_output_contains "doctor mixed: ok ANTHROPIC_API_KEY present"                "ok: ANTHROPIC_API_KEY"         td_mixed
assert_output_contains "doctor mixed: ok OPENAI_API_KEY present"                   "ok: OPENAI_API_KEY"            td_mixed
assert_output_contains "doctor codex-only missing key: reports missing OPENAI"     "missing: OPENAI_API_KEY value" td_codex_none
assert_output_absent   "doctor codex-only missing key: ANTHROPIC not checked"      "ANTHROPIC_API_KEY"             td_codex_none

# ── Summary ───────────────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
