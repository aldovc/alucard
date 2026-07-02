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

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    fail "$label (output unexpectedly contains '$needle')"
  else
    pass "$label"
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

# ── Test group 6: build_worker_prompt ────────────────────────────────────────

TASK_SOURCE="github"
BASE_BRANCH="main"
build_worker_prompt '[{"number":1,"title":"t","body":"b"}]' 'line1
line2' "INSTRUCTIONS"
EXPECTED_GH='<instructions>INSTRUCTIONS</instructions>
<base_branch>main</base_branch>
<commits>line1
line2</commits>
<issues>[{"number":1,"title":"t","body":"b"}]</issues>'
assert_eq "prompt: github mode format is byte-identical" "$EXPECTED_GH" "$FULL_PROMPT"
assert_eq "prompt: github mode leaves no dispatched task id" "" "$DISPATCHED_TASK_ID"
assert_eq "prompt: github mode leaves TASK_XML empty" "" "$TASK_XML"
assert_eq "prompt: github mode leaves PARENT_CONTEXT_XML empty" "" "$PARENT_CONTEXT_XML"

build_worker_prompt '[]' 'a <b> & c' "I"
assert_contains "prompt: xml-escapes injected content" "a &lt;b&gt; &amp; c" "$FULL_PROMPT"

TASK_SOURCE="file"
TASKS_FILE_ABS="$FIX_BASIC"
BASE_BRANCH="main"
FQUEUE=$(get_unblocked_local_tasks "$FIX_BASIC" "$SCRIPT_DIR/..")
build_worker_prompt "$FQUEUE" "commit log" "INSTRUCTIONS"
assert_eq "prompt: file mode dispatches the first eligible task" "1" "$DISPATCHED_TASK_ID"
assert_contains "prompt: file mode marks the task source" "<task_source>file</task_source>" "$FULL_PROMPT"
assert_contains "prompt: file mode injects parent context verbatim" \
  "Shared context for the parser tests" "$FULL_PROMPT"
assert_contains "prompt: file mode injects the chosen task" "First queued task" "$FULL_PROMPT"
assert_not_contains "prompt: file mode injects no issues array" "<issues>" "$FULL_PROMPT"
assert_not_contains "prompt: file mode injects only the first task" \
  "Unblocked because its blocker is done" "$FULL_PROMPT"
assert_contains "prompt: file mode sets TASK_XML for the reviewer" "First queued task" "$TASK_XML"
assert_contains "prompt: file mode sets PARENT_CONTEXT_XML for the reviewer" \
  "Shared context for the parser tests" "$PARENT_CONTEXT_XML"

# ── Test group 6b: append_task_context ───────────────────────────────────────

REVIEW_BASE='<instructions>x</instructions>
<pr_num>7</pr_num>'

assert_eq "review prompt: github mode (empty task_xml) is unchanged" \
  "$REVIEW_BASE" "$(append_task_context "$REVIEW_BASE" "" "")"

APPENDED=$(append_task_context "$REVIEW_BASE" '{"id":"1"}' 'shared plan context')
assert_contains "review prompt: file mode appends <task>" '<task>{"id":"1"}</task>' "$APPENDED"
assert_contains "review prompt: file mode appends <parent_context>" \
  "<parent_context>shared plan context</parent_context>" "$APPENDED"
assert_contains "review prompt: file mode keeps the base prompt" "$REVIEW_BASE" "$APPENDED"

# ── Test group 7: mark_task_in_flight ────────────────────────────────────────

WORK_FILE=$(mktemp /tmp/alucard_test_mark.XXXXXX)
cp "$FIX_BASIC" "$WORK_FILE"

mark_task_in_flight "$WORK_FILE" 1 42
assert_eq "mark: heading flipped with PR annotation" \
  "## [>] 1: First queued task (PR #42)" "$(sed -n '6p' "$WORK_FILE")"
assert_eq "mark: exactly one line changed" "1" \
  "$(diff "$FIX_BASIC" "$WORK_FILE" | grep -c '^<')"
assert_eq "mark: flipped task leaves the queue" "4" \
  "$(get_unblocked_local_tasks "$WORK_FILE" "$SCRIPT_DIR/.." | jq -r '[.[].id] | join(",")')"

assert_exit "mark: unknown task id returns 1" 1 mark_task_in_flight "$WORK_FILE" 99 43
assert_eq "mark: failed mark leaves the file untouched" "1" \
  "$(diff "$FIX_BASIC" "$WORK_FILE" | grep -c '^<')"

rm -f "$WORK_FILE"

# ── Summary ──────────────────────────────────────────────────────────────────

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
