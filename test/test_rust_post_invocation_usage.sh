#!/bin/bash
# A worker log must still be reported when PR discovery fails after invocation.
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"

TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN" "$TARGET/.alucard"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Usage test\n\n## [ ] 1: A queued task\n\nBlocked by: none\n' > "$TARGET/.alucard/tasks.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm 'initial task'
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
case "$1" in
  image) exit 0 ;;
  run)
    touch "$ALUCARD_TEST_WORKER_RAN"
    printf '%s\n' '{"type":"result","modelUsage":{"claude":{"inputTokens":12,"outputTokens":34}},"num_turns":1,"total_cost_usd":0.01}'
    exit 0
    ;;
  rm|kill) exit 0 ;;
esac
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
if [ ! -f "$ALUCARD_TEST_WORKER_RAN" ]; then
  printf '[]\n'
  exit 0
fi
printf 'GitHub unavailable\n' >&2
exit 1
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

set +e
PATH="$MOCK_BIN:$PATH" \
ALUCARD_CACHE_DIR="$TEST_DIR/cache" \
ALUCARD_TEST_WORKER_RAN="$TEST_DIR/worker-ran" \
cargo run --quiet -- --db-path "$TEST_DIR/alucard.db" run "$TARGET" --iterations 1 --no-build \
  --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
rc=$?
set -e

assert_eq "PR discovery failure exits unsuccessfully" "1" "$rc"
out=$(<"$TEST_DIR/out")
assert_contains "PR discovery failure prints worker usage" "=== Alucard Token Usage:" "$out"
assert_contains "PR discovery failure reports worker row" "worker 1" "$out"

finish
