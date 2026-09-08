#!/bin/bash
# The worker stage is the one measurement call site that only fires in the run
# loop: it records the pushed branch after the worker opens a PR. Everything
# else about the artifact is covered by unit fixtures or the continue path, so
# this drives `alucard run` with a mocked worker that really commits and
# pushes, and checks the records that come out.
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

TEST_DIR=$(mktemp -d /tmp/alucard_test_measure_run.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
PR_MARKER="$TEST_DIR/pr-created"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN" "$TARGET/.alucard"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
mkdir -p "$TARGET/src" "$TARGET/tests"
printf '# Measurement test\n\n## [ ] 1: A queued task\n\nBlocked by: none\n' \
  > "$TARGET/.alucard/tasks.md"
printf 'existing\n' > "$TARGET/src/app.py"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm 'initial task'
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main
BASE_SHA=$(git -C "$TARGET" rev-parse HEAD)

# A worker that does what a real one does: write code and tests into the
# worktree it was handed, commit, and push the branch it was placed on.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "$1" in
  image|rm|kill) exit 0 ;;
  run) ;;
  *) exit 0 ;;
esac

wt=""
for a in "$@"; do
  case "$a" in *:/work:rw) wt="${a%%:*}" ;; esac
done

if [ -n "$wt" ] && [ -d "$wt" ]; then
  mkdir -p "$wt/src" "$wt/tests"
  printf 'a\nb\nc\n' >> "$wt/src/app.py"
  printf 'x\ny\n' > "$wt/tests/test_app.py"
  git -C "$wt" -c user.email=w@example.invalid -c user.name=worker add -A
  git -C "$wt" -c user.email=w@example.invalid -c user.name=worker commit -qm 'worker output'
  git -C "$wt" push -q origin HEAD
  touch "$ALUCARD_TEST_PR_MARKER"
fi

printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done","num_turns":7,"total_cost_usd":1.25,"modelUsage":{"m":{"inputTokens":100,"outputTokens":200,"cacheReadInputTokens":300,"cacheCreationInputTokens":40}}}'
exit 0
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
case "$1 $2" in
  "pr list") [ -f "$ALUCARD_TEST_PR_MARKER" ] && printf '59\n' ;;
  "pr checks") echo "no checks reported" >&2; exit 1 ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

set +e
PATH="$MOCK_BIN:$PATH" \
ALUCARD_TEST_PR_MARKER="$PR_MARKER" \
  "$ALUCARD" run "$TARGET" --iterations 1 --no-build --max-review-cycles 0 \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" \
    > "$TEST_DIR/out" 2>&1
RUN_RC=$?
set -e

MEAS_FILE=$(ls "$TEST_DIR/logs"/alucard-*/measurements.jsonl 2>/dev/null | head -1)

echo "── the run produces an artifact at all ──"
assert_eq "the run completes"          "0" "$RUN_RC"
assert_eq "measurements.jsonl written" "true" \
  "$([ -n "$MEAS_FILE" ] && [ -s "$MEAS_FILE" ] && echo true || echo false)"

MEAS=$(cat "$MEAS_FILE" 2>/dev/null || true)
RUN_REC=$(printf '%s\n' "$MEAS" | jq -c 'select(.record == "run")' | head -1)
STAGE_REC=$(printf '%s\n' "$MEAS" | jq -c 'select(.record == "stage" and .stage == "worker")' | head -1)
ITER_REC=$(printf '%s\n' "$MEAS" | jq -c 'select(.record == "iteration")' | head -1)

echo "── the run record ──"
assert_eq "a run record is written" "run" \
  "$(printf '%s' "$RUN_REC" | jq -r '.record // "MISSING"')"
assert_eq "the format is stamped"   "1" \
  "$(printf '%s' "$RUN_REC" | jq -r '.format')"
assert_eq "the repo is identified"  "true" \
  "$(printf '%s' "$RUN_REC" | jq -r '(.repo_id | length > 0)')"

echo "── the worker stage record ──"
assert_eq "a worker stage is recorded" "worker" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.stage // "MISSING"')"
assert_eq "it names the PR"            "59" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.pr')"
assert_eq "it pins the iteration base" "$BASE_SHA" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.base_sha')"
assert_eq "the base is exact"          "pinned" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.baseline_source')"
# The head has to come from the pushed branch: the worker's worktree is
# disposed before the gates run, so a stale local ref would measure nothing.
assert_eq "it resolves the pushed head" "true" \
  "$(printf '%s' "$STAGE_REC" | jq -r '(.head_sha != .base_sha)')"
assert_eq "the diff was available"      "null" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.from_base.unavailable')"
assert_eq "implementation lines counted" "3" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.from_base.impl.added')"
assert_eq "test lines counted"           "2" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.from_base.tests.added')"
assert_eq "changed paths retained"       "2" \
  "$(printf '%s' "$STAGE_REC" | jq -r '.from_base.paths | length')"

echo "── the iteration record ──"
assert_eq "an iteration record is written" "iteration" \
  "$(printf '%s' "$ITER_REC" | jq -r '.record // "MISSING"')"
assert_eq "it names the PR"                "59" \
  "$(printf '%s' "$ITER_REC" | jq -r '.pr')"
assert_eq "worker usage is rolled up"      "1" \
  "$(printf '%s' "$ITER_REC" | jq -r '.roles.worker.invocations')"
assert_eq "worker cost is recorded"        "1.25" \
  "$(printf '%s' "$ITER_REC" | jq -r '.roles.worker.cost')"
assert_eq "elapsed time is recorded"       "true" \
  "$(printf '%s' "$ITER_REC" | jq -r '(.duration_s >= 0)')"

echo "── the dispatched prompt is archived ──"
assert_eq "the worker prompt is kept" "true" \
  "$([ -s "$(dirname "$MEAS_FILE")/prompts/iter-1.txt" ] && echo true || echo false)"

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
