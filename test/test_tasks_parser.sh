#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"
FIX_BASIC="$SCRIPT_DIR/fixtures/tasks-basic.md"
FIX_MALFORMED="$SCRIPT_DIR/fixtures/tasks-malformed.md"

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

assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

# Source alucard to load helper functions without running main.
# The BASH_SOURCE guard in alucard prevents main from executing when sourced.
# shellcheck disable=SC1090
source "$ALUCARD"

# ── Test group 1: parse_tasks_file structure ─────────────────────────────────

PARSED=$(parse_tasks_file "$FIX_BASIC")

assert_eq "parse: no errors on basic fixture" "0" \
  "$(echo "$PARSED" | jq '.errors | length')"
assert_eq "parse: extracts all 7 tasks" "7" \
  "$(echo "$PARSED" | jq '.tasks | length')"
assert_contains "parse: parent context comes from above the first heading" \
  "Shared context for the parser tests" \
  "$(echo "$PARSED" | jq -r '.parent_context')"
assert_eq "parse: task ids in file order" "1,2,3,4,5,6,7" \
  "$(echo "$PARSED" | jq -r '[.tasks[].id] | join(",")')"
assert_eq "parse: states captured" ' ,x, , ,>,h, ' \
  "$(echo "$PARSED" | jq -r '[.tasks[].state] | join(",")')"
assert_eq "parse: PR suffix stripped from title" "In flight" \
  "$(echo "$PARSED" | jq -r '.tasks[4].title')"
assert_eq "parse: PR number extracted from title suffix" "123" \
  "$(echo "$PARSED" | jq -r '.tasks[4].pr')"
assert_eq "parse: pr is null without a PR suffix" "null" \
  "$(echo "$PARSED" | jq -r '.tasks[0].pr')"
assert_contains "parse: body captured until next heading" \
  "What to build: the first eligible thing." \
  "$(echo "$PARSED" | jq -r '.tasks[0].body')"
assert_eq "parse: heading line numbers recorded" "6" \
  "$(echo "$PARSED" | jq -r '.tasks[0].line')"

# ── Test group 2: get_unblocked_local_tasks eligibility ─────────────────────
# The basic fixture has no GitHub-issue blockers, so this stays offline.

QUEUE=$(get_unblocked_local_tasks "$FIX_BASIC" "$SCRIPT_DIR/..")

assert_eq "queue: only unblocked todo tasks, in file order" "1,4" \
  "$(echo "$QUEUE" | jq -r '[.[].id] | join(",")')"
assert_eq "queue: excludes done, in-flight, human, and blocked tasks" "2" \
  "$(echo "$QUEUE" | jq 'length')"
assert_eq "queue: emits id, title, body per task" "body,id,title" \
  "$(echo "$QUEUE" | jq -r '.[0] | keys | join(",")')"

# ── Test group 3: validate_tasks_file (doctor) ───────────────────────────────

assert_exit "validate: basic fixture passes" 0 validate_tasks_file "$FIX_BASIC"
assert_exit "validate: malformed fixture fails" 1 validate_tasks_file "$FIX_MALFORMED"

PROBLEMS=$(validate_tasks_file "$FIX_MALFORMED" || true)

assert_contains "validate: reports duplicate id" \
  "duplicate task id 1" "$PROBLEMS"
assert_contains "validate: reports dangling blocker ref" \
  "blocked by unknown task id 9" "$PROBLEMS"
assert_contains "validate: reports empty header" \
  "empty header" "$PROBLEMS"
assert_contains "validate: reports bad state token as malformed" \
  "malformed task heading: ## [?] 3: Bad state token" "$PROBLEMS"
assert_contains "validate: reports missing colon as malformed" \
  "malformed task heading: ## [ ] 4 missing the colon separator" "$PROBLEMS"

EMPTY_FILE=$(mktemp /tmp/alucard_test_tasks.XXXXXX)
printf '' > "$EMPTY_FILE"
EMPTY_PROBLEMS=$(validate_tasks_file "$EMPTY_FILE" || true)
assert_contains "validate: empty file reports no tasks" \
  "no task headings found" "$EMPTY_PROBLEMS"
rm -f "$EMPTY_FILE"

# ── Test group 4: resolve_task_source precedence ─────────────────────────────

FAKE_REPO=$(mktemp -d /tmp/alucard_test_repo.XXXXXX)
mkdir -p "$FAKE_REPO/.alucard"
cp "$FIX_BASIC" "$FAKE_REPO/.alucard/tasks.md"

TASKS_FILE="" TASKS_FILE_FROM_FLAG=false FORCE_GITHUB=false
resolve_task_source ""
assert_eq "source: defaults to github with no file and no repo" "github" "$TASK_SOURCE"

TASKS_FILE="" TASKS_FILE_FROM_FLAG=false FORCE_GITHUB=false
resolve_task_source "$FAKE_REPO"
assert_eq "source: auto-detects .alucard/tasks.md" "file" "$TASK_SOURCE"
assert_eq "source: auto-detect resolves the file path" \
  "$FAKE_REPO/.alucard/tasks.md" "$TASKS_FILE_ABS"

TASKS_FILE="$FIX_BASIC" TASKS_FILE_FROM_FLAG=true FORCE_GITHUB=false
resolve_task_source ""
assert_eq "source: explicit --tasks wins without a repo" "file" "$TASK_SOURCE"
assert_eq "source: explicit --tasks resolves to an absolute path" \
  "$FIX_BASIC" "$TASKS_FILE_ABS"

TASKS_FILE="/nonexistent/tasks.md" TASKS_FILE_FROM_FLAG=true FORCE_GITHUB=false
assert_exit "source: missing explicit tasks file dies" 1 resolve_task_source ""

TASKS_FILE="$FIX_BASIC" TASKS_FILE_FROM_FLAG=true FORCE_GITHUB=true
assert_exit "source: --tasks with --github dies" 1 resolve_task_source ""

# ALUCARD_TASKS_FILE (env default, not flag) + --github: github wins quietly.
TASKS_FILE="$FIX_BASIC" TASKS_FILE_FROM_FLAG=false FORCE_GITHUB=true
resolve_task_source "$FAKE_REPO"
assert_eq "source: --github overrides env default and auto-detect" "github" "$TASK_SOURCE"

rm -rf "$FAKE_REPO"

# ── Test group 5: get_task_queue dispatch ────────────────────────────────────

TASK_SOURCE="file" TASKS_FILE_ABS="$FIX_BASIC"
assert_eq "dispatch: file source returns the local queue" "1,4" \
  "$(get_task_queue "$SCRIPT_DIR/.." | jq -r '[.[].id] | join(",")')"

TASK_SOURCE="bogus"
assert_exit "dispatch: unknown source dies" 1 get_task_queue "$SCRIPT_DIR/.."

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
