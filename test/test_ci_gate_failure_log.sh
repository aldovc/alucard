#!/bin/bash
# ci_gate must resolve the failed run ID via a real jq pipeline (not
# `gh ... --arg`, which gh rejects and silently swallows) and hand the
# fix agent the actual failure text, not an empty <failure_log> block.
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

TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
CAPTURE="$TEST_DIR/docker-run.args"
REMOTE="$TEST_DIR/remote.git"
REPO="$TEST_DIR/repo"
mkdir -p "$MOCK_BIN"

PR_NUM=77
HEAD_SHA="deadbeefcafef00d"
RUN_ID=123456789
FAILURE_TEXT="FAILED test_thing.py::test_widget — AssertionError: expected 1 got 2"

git init -q --bare "$REMOTE"
mkdir -p "$REPO"
git -C "$REPO" init -q -b main
git -C "$REPO" config user.email test@example.invalid
git -C "$REPO" config user.name test
printf 'hello\n' > "$REPO/README.md"
git -C "$REPO" add .
git -C "$REPO" commit -qm 'initial'
git -C "$REPO" remote add origin "$REMOTE"
git -C "$REPO" push -qu origin main
git -C "$REPO" checkout -qb feature-branch
printf 'change\n' >> "$REPO/README.md"
git -C "$REPO" commit -qam 'feature work'
git -C "$REPO" push -qu origin feature-branch
# make_agent_clone force-updates this branch from origin; git refuses that
# on the branch currently checked out, so leave REPO_ABS on main.
git -C "$REPO" checkout -q main

# `gh` mock covers every call ci_gate makes: the open-PR lookup, the head
# SHA refresh, the CI-checks poll (made to fail so the fix-agent path is
# reached), the failed-run lookup (the buggy line under test), and the
# failed-run log fetch.
cat > "$MOCK_BIN/gh" <<MOCK
#!/bin/bash
set -euo pipefail
case "\$1 \$2" in
  "pr list")
    printf '%s\n' "$PR_NUM"
    ;;
  "pr view")
    printf '%s\n' "$HEAD_SHA"
    ;;
  "pr checks")
    echo "some checks were not successful" >&2
    exit 1
    ;;
  "run list")
    # Reproduce real gh's behavior for the buggy invocation: gh has no --arg
    # flag, so it swallows "--arg" as --jq's value and treats "sha" as an
    # unknown positional subcommand.
    for a in "\$@"; do
      if [ "\$a" = "--arg" ]; then
        echo 'unknown command "sha" for "gh run list"' >&2
        exit 1
      fi
    done
    printf '[{"databaseId":%s,"conclusion":"failure","headSha":"%s"}]\n' "$RUN_ID" "$HEAD_SHA"
    ;;
  "run view")
    printf '%s\n' "$FAILURE_TEXT"
    ;;
  *)
    echo "unexpected gh invocation: \$*" >&2
    exit 1
    ;;
esac
MOCK
chmod +x "$MOCK_BIN/gh"

# `timeout` passes the CI-checks poll straight through to the mocked `gh`,
# and short-circuits the invoke_agent docker call (mirrors test_agent_settings.sh).
cat > "$MOCK_BIN/timeout" <<MOCK
#!/bin/bash
set -euo pipefail
if [ "\$1" = "--kill-after" ]; then
  shift 3
  printf '%s\n' "\$@" > "$CAPTURE"
  printf '%s\n' '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"ok"}}}'
  exit 0
fi
shift
exec "\$@"
MOCK
chmod +x "$MOCK_BIN/timeout"

PATH="$MOCK_BIN:$PATH"

# Source alucard to load helper functions without running main.
# shellcheck disable=SC1090
source "$ALUCARD"

REPO_ABS="$REPO"
WT_ROOT="$TEST_DIR/worktrees"
LOG_DIR="$TEST_DIR/logs"
mkdir -p "$WT_ROOT" "$LOG_DIR"
ENV_FILE="$SCRIPT_DIR/fixtures/credentials.env"
IMAGE="test-image"
TIMEOUT_MIN=1
stream_text='.event.delta.text'
CREATED_WORKTREES=()
CREATED_CONTAINERS=()

set +e
ci_gate 1 "feature-branch" "$REMOTE" > "$TEST_DIR/out" 2>&1
rc=$?
set -e

out=$(<"$TEST_DIR/out")
fix_log="$LOG_DIR/iter-1-fix-1.jsonl"

[ "$rc" -eq 0 ] && pass "ci_gate returns 0 after exhausting fix attempts" \
  || fail "ci_gate returns 0 after exhausting fix attempts (got $rc)"

assert_contains "ci_gate resolves the failed run ID and fetches its log" \
  "launching fix agent" "$out"

if [ -f "$CAPTURE" ]; then
  docker_args=$(<"$CAPTURE")
else
  docker_args=""
  fail "docker run capture file was written"
fi

assert_contains "fix agent prompt carries the real failure text" \
  "$FAILURE_TEXT" "$docker_args"
assert_not_contains "ci_gate does not log the empty-failure-log warning when the lookup succeeds" \
  "could not resolve a failed run ID" "$out"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
