#!/bin/bash
# A review cycle that ends without a verdict means the PR was never reviewed.
# It must be labelled for a human rather than returned from quietly, and a
# transport drop must buy a fresh reviewer attempt instead of burning the
# cycle. Driven through `alucard continue`, which reaches the review gate
# without ever running the worker loop.
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

assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

TEST_DIR=$(mktemp -d /tmp/alucard_test_no_verdict.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
GH_TRACE="$TEST_DIR/gh-trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN"
touch "$TRACE" "$GH_TRACE"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

BRANCH="feat/under-review"
git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main 2>/dev/null || { mkdir -p "$TARGET" && git -C "$TARGET" init -q -b main; }
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Under review\n' > "$TARGET/README.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm initial
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main
git -C "$TARGET" checkout -q -b "$BRANCH"
printf 'change\n' >> "$TARGET/README.md"
git -C "$TARGET" commit -qam change
git -C "$TARGET" push -q origin "$BRANCH"
git -C "$TARGET" checkout -q main

# The reviewer container's outcome is driven per-run by a file the test
# rewrites between scenarios: each line is one `<exit-code> <result-json>`
# consumed in order, so a scenario can hand back a transport drop followed by
# a clean run.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  image) exit 0 ;;
  rm|kill) exit 0 ;;
  run) ;;
  *) exit 0 ;;
esac

printf 'review launch\n' >> "$ALUCARD_TEST_TRACE"
_n=$(grep -c '^review launch$' "$ALUCARD_TEST_TRACE")
_line=$(sed -n "${_n}p" "$ALUCARD_TEST_SCRIPT")
[ -n "$_line" ] || _line=$(tail -n1 "$ALUCARD_TEST_SCRIPT")
_rc="${_line%% *}"
_json="${_line#* }"

# The verdict file lands in the rw output mount, which arrives as
# `-v <hostdir>:/work-output:rw` in this argv.
if [ -n "${ALUCARD_TEST_VERDICT:-}" ] && [ "$_rc" = "0" ]; then
  for _a in "$@"; do
    case "$_a" in
      *:/work-output:rw) printf '%s\n' "$ALUCARD_TEST_VERDICT" > "${_a%%:*}/.alucard-review"
                         printf 'looks fine\n' > "${_a%%:*}/.alucard-review-body" ;;
    esac
  done
fi

printf '%s\n' "$_json"
exit "$_rc"
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
printf '%s\n' "$*" >> "$ALUCARD_TEST_GH_TRACE"
case "$1 $2" in
  "pr checks")
    # No CI configured — the gate treats this as a pass.
    echo "no checks reported on the '$ALUCARD_TEST_BRANCH' branch" >&2
    exit 1 ;;
  "pr list")   printf '%s\n' "$ALUCARD_TEST_PR" ;;
  "pr view")
    case "$*" in
      *state,headRefName*) printf '{"state":"OPEN","headRefName":"%s"}\n' "$ALUCARD_TEST_BRANCH" ;;
      *comments*)          printf '{"comments":[]}\n' ;;
      *reviews*)           printf '\n' ;;
      *headRefOid*)        printf 'deadbeef\n' ;;
      *)                   printf '{}\n' ;;
    esac ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

MAX_TURNS_RESULT='{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}'
TRANSPORT_RESULT='{"type":"result","subtype":"success","is_error":true,"result":"API Error: Connection closed"}'
CLEAN_RESULT='{"type":"result","subtype":"success","is_error":false,"result":"reviewed"}'

run_continue() {
  : > "$TRACE"
  : > "$GH_TRACE"
  rm -rf "$TEST_DIR/logs"
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_TRACE="$TRACE" \
  ALUCARD_TEST_GH_TRACE="$GH_TRACE" \
  ALUCARD_TEST_SCRIPT="$TEST_DIR/script" \
  ALUCARD_TEST_PR=77 \
  ALUCARD_TEST_BRANCH="$BRANCH" \
  ALUCARD_TEST_VERDICT="${1:-}" \
  ALUCARD_TRANSPORT_RETRY_ATTEMPTS="${2:-1}" \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 1 \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
  CONTINUE_RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  EVENTS=$(cat "$TEST_DIR/logs"/alucard-*/events.log 2>/dev/null || true)
}

# ── Scenario 1: reviewer exhausts its turns ──────────────────────────────────
echo "── reviewer exhausted (max turns) ──"
printf '1 %s\n' "$MAX_TURNS_RESULT" > "$TEST_DIR/script"
run_continue "" 1

assert_eq "an exhausted reviewer is not retried" \
  "1" "$(grep -c '^review launch$' "$TRACE" || true)"
assert_contains "the exhausted class is logged" "class=exhausted" "$EVENTS"
assert_contains "the no-verdict PR is flagged for a human" \
  "flagging for human" "$EVENTS"
assert_contains "the PR gets the needs-human label" \
  "pr edit 77 --add-label needs-human" "$(<"$GH_TRACE")"
assert_contains "the PR comment says it was never reviewed" \
  "has not been reviewed" "$(<"$GH_TRACE")"
assert_contains "the comment names the caps to raise" \
  "ALUCARD_REVIEWER_MAX_TURNS" "$(<"$GH_TRACE")"

# ── Scenario 2: transport drop, then a clean review ──────────────────────────
echo ""
echo "── transport drop then a verdict ──"
{ printf '1 %s\n' "$TRANSPORT_RESULT"; printf '0 %s\n' "$CLEAN_RESULT"; } > "$TEST_DIR/script"
run_continue "APPROVED" 1

assert_eq "a transport drop buys a second reviewer attempt" \
  "2" "$(grep -c '^review launch$' "$TRACE" || true)"
assert_contains "the retry is logged" "retrying reviewer" "$EVENTS"
assert_contains "the recovered verdict is used" "approved" "$EVENTS"

# ── Scenario 3: transport drops on every attempt ─────────────────────────────
echo ""
echo "── transport drops on every attempt ──"
printf '1 %s\n' "$TRANSPORT_RESULT" > "$TEST_DIR/script"
run_continue "" 1

assert_eq "retries are capped by the configured budget" \
  "2" "$(grep -c '^review launch$' "$TRACE" || true)"
assert_contains "an unrecoverable transport drop still flags a human" \
  "pr edit 77 --add-label needs-human" "$(<"$GH_TRACE")"
assert_contains "the comment tells the operator to re-run continue" \
  "alucard continue" "$(<"$GH_TRACE")"

# ── Scenario 4: a verdict recorded before the drop is kept ───────────────────
# The reviewer did the work and wrote its decision file; losing the connection
# on the way out must not buy a second full review of the same PR.
echo ""
echo "── verdict recorded, then a transport drop ──"
printf '0 %s\n' "$TRANSPORT_RESULT" > "$TEST_DIR/script"
run_continue "CHANGES_REQUESTED" 1
assert_eq "a recorded verdict short-circuits the retry" \
  "1" "$(grep -c '^review launch$' "$TRACE" || true)"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
