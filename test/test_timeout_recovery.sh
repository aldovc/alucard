#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"

PASS=0
FAIL=0
readonly CODEX_DOCKER_EXIT=42
readonly CLAUDE_DOCKER_EXIT=43
readonly PREFLIGHT_DOCKER_EXIT=44

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

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    fail "$label (unexpected '$needle')"
  else
    pass "$label"
  fi
}

assert_line_after() {
  local label="$1" first="$2" second="$3" trace="$4"
  local first_line second_line
  first_line=$(printf '%s\n' "$trace" | grep -nF -- "$first" | head -n1 | cut -d: -f1 || true)
  second_line=$(printf '%s\n' "$trace" | grep -nF -- "$second" | head -n1 | cut -d: -f1 || true)
  if [ -n "$first_line" ] && [ -n "$second_line" ] && [ "$second_line" -gt "$first_line" ]; then
    pass "$label"
  else
    fail "$label (cleanup did not follow run)"
  fi
}

TEST_DIR=$(mktemp -d)
MOCK_BIN="$TEST_DIR/bin"
TRACE_FILE="$TEST_DIR/trace"
mkdir -p "$MOCK_BIN"
touch "$TRACE_FILE"

cleanup() {
  rm -rf "$TEST_DIR"
}
trap cleanup EXIT

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
{
  printf 'timeout'
  printf ' %q' "$@"
  printf '\n'
} >> "$ALUCARD_TEST_TRACE"
if [ "$1" = "--kill-after" ]; then
  shift 2
fi
shift
exec "$@"
MOCK

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
{
  printf 'docker'
  printf ' %q' "$@"
  printf '\n'
} >> "$ALUCARD_TEST_TRACE"

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

cat > "$MOCK_BIN/git" <<'MOCK'
#!/bin/bash
set -euo pipefail
{
  printf 'git'
  printf ' %q' "$@"
  printf '\n'
} >> "$ALUCARD_TEST_TRACE"
if [ "$1" = "clone" ]; then
  mkdir -p "${!#}"
fi
MOCK

cat > "$MOCK_BIN/jq" <<'MOCK'
#!/bin/bash
cat
MOCK

chmod +x "$MOCK_BIN/timeout" "$MOCK_BIN/docker" "$MOCK_BIN/git" "$MOCK_BIN/jq"

# Source alucard to load helper functions without running main.
# The BASH_SOURCE guard in alucard prevents main from executing when sourced.
# shellcheck disable=SC1090
source "$ALUCARD"

export PATH="$MOCK_BIN:$PATH"
export ALUCARD_TEST_TRACE="$TRACE_FILE"
TIMEOUT_MIN=7
ENV_FILE="$TEST_DIR/alucard.env"
IMAGE="alucard-test-image"
stream_text='.'
codex_stream_text='.'
touch "$ENV_FILE"

run_agent_and_capture_exit() {
  local provider="$1" role="$2" docker_exit="$3" log_file="$4"
  local actual_exit=0
  export ALUCARD_WORKER_PROVIDER="$provider"
  export ALUCARD_TEST_DOCKER_RUN_EXIT="$docker_exit"
  invoke_agent "$role" "$log_file" 10 1 'test prompt' >/dev/null || actual_exit=$?
  unset ALUCARD_WORKER_PROVIDER ALUCARD_TEST_DOCKER_RUN_EXIT
  printf '%s' "$actual_exit"
}

# ── Test group 1: invoke_agent timeout and reaping ───────────────────────────

CODEX_LOG="$TEST_DIR/codex.jsonl"
code=$(run_agent_and_capture_exit codex iter-codex "$CODEX_DOCKER_EXIT" "$CODEX_LOG")
trace=$(cat "$TRACE_FILE")
assert_eq "Codex returns the docker status from PIPESTATUS[0]" "$CODEX_DOCKER_EXIT" "$code"
assert_contains "Codex timeout uses kill-after grace period" \
  'timeout --kill-after 30s 7m docker run --rm -i' "$trace"
assert_contains "Codex force-removes its named container" \
  'docker rm -f alucard-iter-codex' "$trace"
assert_line_after "Codex cleanup follows non-zero docker run" \
  'docker run --rm -i' 'docker rm -f alucard-iter-codex' "$trace"

: > "$TRACE_FILE"
CLAUDE_LOG="$TEST_DIR/claude.jsonl"
code=$(run_agent_and_capture_exit claude iter-claude "$CLAUDE_DOCKER_EXIT" "$CLAUDE_LOG")
trace=$(cat "$TRACE_FILE")
assert_eq "Claude returns the docker status from PIPESTATUS[0]" "$CLAUDE_DOCKER_EXIT" "$code"
assert_contains "Claude timeout uses kill-after grace period" \
  'timeout --kill-after 30s 7m docker run --rm' "$trace"
assert_contains "Claude force-removes its named container" \
  'docker rm -f alucard-iter-claude' "$trace"
assert_line_after "Claude cleanup follows non-zero docker run" \
  'docker run --rm' 'docker rm -f alucard-iter-claude' "$trace"

# ── Test group 2: toolchain preflight timeout and reaping ────────────────────

: > "$TRACE_FILE"
REPO_ABS="$TEST_DIR/repo"
LOG_DIR="$TEST_DIR/logs"
BASE_BRANCH=main
CREATED_WORKTREES=()
mkdir -p "$REPO_ABS" "$LOG_DIR"
touch "$REPO_ABS/package-lock.json"
export ALUCARD_TEST_DOCKER_RUN_EXIT="$PREFLIGHT_DOCKER_EXIT"
toolchain_preflight
unset ALUCARD_TEST_DOCKER_RUN_EXIT
trace=$(cat "$TRACE_FILE")
assert_contains "Preflight timeout uses kill-after grace period" \
  'timeout --kill-after 30s 7m docker run --rm --name alucard-preflight-' "$trace"
assert_contains "Preflight force-removes its named container" \
  "docker rm -f alucard-preflight-$$" "$trace"
assert_line_after "Preflight cleanup follows non-zero docker run" \
  'docker run --rm --name alucard-preflight-' "docker rm -f alucard-preflight-$$" "$trace"
assert_contains "Preflight records the docker failure status" \
  "rc=${PREFLIGHT_DOCKER_EXIT}" "$TOOLCHAIN_STATUS"

# ── Test group 3: clone setup and human-comment composition ─────────────────

: > "$TRACE_FILE"
REPO_ABS="$TEST_DIR/repo-source"
mkdir -p "$REPO_ABS"
branch_dest="$TEST_DIR/branch-clone"
branch_origin="git@github.com:example/target.git"
make_agent_clone "feature/test" "$branch_dest" "$branch_origin"
trace=$(cat "$TRACE_FILE")
assert_contains "Branch clone fetches the worker branch" \
  "git -C $REPO_ABS fetch origin feature/test" "$trace"
assert_contains "Branch clone creates its local tracking branch" \
  "git -C $REPO_ABS branch --force feature/test origin/feature/test" "$trace"
assert_contains "Branch clone uses the requested branch" \
  "git clone --local --no-tags --single-branch --branch feature/test $REPO_ABS $branch_dest" "$trace"
assert_contains "Branch clone restores the real origin URL" \
  "git -C $branch_dest remote set-url origin $branch_origin" "$trace"
assert_line_after "Branch clone creates the tracking branch after fetching" \
  "git -C $REPO_ABS fetch origin feature/test" \
  "git -C $REPO_ABS branch --force feature/test origin/feature/test" "$trace"
assert_line_after "Branch clone happens after its tracking branch is created" \
  "git -C $REPO_ABS branch --force feature/test origin/feature/test" \
  "git clone --local --no-tags --single-branch --branch feature/test $REPO_ABS $branch_dest" "$trace"

: > "$TRACE_FILE"
base_dest="$TEST_DIR/base-clone"
make_agent_clone "main" "$base_dest" "$branch_origin" base
trace=$(cat "$TRACE_FILE")
assert_contains "Base clone fetches directly into the remote-tracking ref" \
  "git -C $REPO_ABS fetch origin +refs/heads/main:refs/remotes/origin/main" "$trace"
assert_not_contains "Base clone skips a local branch force-update" \
  "git -C $REPO_ABS branch --force" "$trace"
assert_line_after "Base clone happens after its refspec fetch" \
  "git -C $REPO_ABS fetch origin +refs/heads/main:refs/remotes/origin/main" \
  "git clone --local --no-tags --single-branch --branch main $REPO_ABS $base_dest" "$trace"

findings_file="$TEST_DIR/findings"
empty_actual="$TEST_DIR/human-empty-actual"
printf 'Original findings\n\nWith trailing newlines\n\n' > "$findings_file"
findings=$'Original findings\n\nWith trailing newlines\n\n'
append_human_comments "$findings" "" > "$empty_actual"
if cmp -s "$findings_file" "$empty_actual"; then
  pass "Empty human comments preserve findings byte-for-byte"
else
  fail "Empty human comments changed the findings"
fi

human="Please add focused coverage."
nonempty_actual="$TEST_DIR/human-nonempty-actual"
nonempty_expected="$TEST_DIR/human-nonempty-expected"
append_human_comments "$findings" "$human" > "$nonempty_actual"
printf '%s\n\n---\n\n## Additional human PR comments\n\nTreat each as an additional finding when actionable and on-topic. Off-topic or out-of-scope comments should be acknowledged via `gh pr comment --body-file <file>` rather than silently ignored.\n\n%s' \
  "$findings" "$human" > "$nonempty_expected"
if cmp -s "$nonempty_expected" "$nonempty_actual"; then
  pass "Non-empty human comments emit the established block"
else
  fail "Non-empty human comments did not emit the established block"
fi
echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
