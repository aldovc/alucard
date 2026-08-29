#!/bin/bash
# A transport failure after the worker creates a PR must not launch a duplicate
# worker on a fresh branch.
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
PR_MARKER="$TEST_DIR/pr-created"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN" "$TARGET/.alucard"
touch "$TRACE"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Retry test\n\n## [ ] 1: A queued task\n\nBlocked by: none\n' > "$TARGET/.alucard/tasks.md"
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
    printf 'worker launch\n' >> "$ALUCARD_TEST_TRACE"
    touch "$ALUCARD_TEST_PR_MARKER"
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
if [ "$1" = "pr" ] && [ "$2" = "list" ] && [ -f "$ALUCARD_TEST_PR_MARKER" ]; then
  printf '59\n'
fi
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

set +e
PATH="$MOCK_BIN:$PATH" \
ALUCARD_TEST_TRACE="$TRACE" \
ALUCARD_TEST_PR_MARKER="$PR_MARKER" \
ALUCARD_TRANSPORT_RETRY_ATTEMPTS=1 \
"$ALUCARD" run "$TARGET" --iterations 1 --no-build --max-review-cycles 0 \
  --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
rc=$?
set -e

assert_eq "transport-after-PR run completes" "0" "$rc"
trace=$(<"$TRACE")
assert_eq "transport-after-PR launches one worker" "1" "$(grep -c '^worker launch$' "$TRACE" || true)"
assert_not_contains "transport-after-PR does not take retry branch" \
  'retrying with a fresh worktree' "$(<"$TEST_DIR/logs"/alucard-*/*events*)"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
