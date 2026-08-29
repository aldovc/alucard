#!/bin/bash
# Covers the recovery contract for a final transport failure before the worker
# has changed its worktree: preserve a discoverable branch, but do not open PR.
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"

PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

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

TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN" "$TARGET/.alucard"
touch "$TRACE" "$TEST_DIR/alucard.env"

git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Recovery test\n\n## [ ] 1: A queued task\n\nBlocked by: none\n' > "$TARGET/.alucard/tasks.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm 'initial task'
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "$1" in
  image) exit 0 ;;
  run)
    printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"API Error: Connection closed"}'
    exit 1
    ;;
  rm|kill) exit 0 ;;
esac
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
printf 'gh %q' "$@" >> "$ALUCARD_TEST_TRACE"
printf '\n' >> "$ALUCARD_TEST_TRACE"
# Both PR-list calls deliberately find no existing PR. A PR creation would be
# recorded above and fail the assertion below.
exit 0
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

set +e
PATH="$MOCK_BIN:$PATH" \
ALUCARD_TEST_TRACE="$TRACE" \
ALUCARD_TRANSPORT_RETRY_ATTEMPTS=0 \
"$ALUCARD" run "$TARGET" --iterations 1 --no-build \
  --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
rc=$?
set -e

if [ "$rc" -eq 0 ]; then
  pass "transport recovery run completes"
else
  fail "transport recovery run completes (rc=$rc)"
fi

branches=$(git --git-dir="$REMOTE" for-each-ref --format='%(refname:short)' refs/heads/alucard/)
if [ -n "$branches" ]; then
  pass "clean transport failure pushes a recovery branch"
else
  fail "clean transport failure pushes a recovery branch"
fi

trace=$(<"$TRACE")
assert_not_contains "clean transport failure does not create a recovery PR" "pr create" "$trace"
assert_contains "clean transport failure emits its preserved-branch event" \
  "transport failure with no worker commits" "$(<"$TEST_DIR/logs"/alucard-*/*events*)"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
