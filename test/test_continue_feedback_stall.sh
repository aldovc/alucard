#!/bin/bash
# `alucard continue` runs feedback once, then the gates. A feedback agent that
# dies without advancing the branch must stop before those gates — the same
# stall decision review_gate already makes inside its loop — and hand the PR to
# a human. Driven through continue with mocked docker/gh, the way
# test_review_no_verdict.sh covers the no-verdict path.
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

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    fail "$label (output unexpectedly contains '$needle')"
  else
    pass "$label"
  fi
}

TEST_DIR=$(mktemp -d /tmp/alucard_test_continue_stall.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
GH_TRACE="$TEST_DIR/gh-trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN"
touch "$TRACE" "$GH_TRACE"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

BRANCH="feat/continue-stall"
git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main 2>/dev/null || { mkdir -p "$TARGET" && git -C "$TARGET" init -q -b main; }
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# stall seed\n' > "$TARGET/README.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm initial
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main
git -C "$TARGET" checkout -q -b "$BRANCH"
printf 'change\n' >> "$TARGET/README.md"
git -C "$TARGET" commit -qam change
git -C "$TARGET" push -q origin "$BRANCH"
git -C "$TARGET" checkout -q main

# One prior CHANGES_REQUESTED bot comment so continue has findings to hand the
# feedback agent. Without it the path under test is skipped entirely.
CR_BODY='**🤖 Alucard review cycle 1/3: CHANGES_REQUESTED**

## Findings
- fix the thing
'
COMMENTS_JSON=$(jq -n --arg body "$CR_BODY" \
  '{comments:[{body:$body,createdAt:"2026-01-01T00:00:00Z",author:{login:"bot"}}]}')

# Role order on continue-with-findings: feedback first, then (if not stalled)
# reviewer via the review gate. Each docker-run line is one agent launch.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  image) exit 0 ;;
  rm|kill) exit 0 ;;
  run) ;;
  *) exit 0 ;;
esac

printf 'agent launch\n' >> "$ALUCARD_TEST_TRACE"
_n=$(grep -c '^agent launch$' "$ALUCARD_TEST_TRACE")
_line=$(sed -n "${_n}p" "$ALUCARD_TEST_SCRIPT")
[ -n "$_line" ] || _line=$(tail -n1 "$ALUCARD_TEST_SCRIPT")
_rc="${_line%% *}"
_json="${_line#* }"

# Optional: a "push" line tells this agent to advance the branch the way a real
# feedback agent does — from a separate clone, so the host tracking ref stays
# stale and only remote_branch_sha's fetch sees the move.
if [ "${_line##* }" = "push" ]; then
  _json="${_line#* }"
  _json="${_json% push}"
  _agent=$(mktemp -d "$ALUCARD_TEST_DIR/agent.XXXXXX")
  git clone --quiet "$ALUCARD_TEST_REMOTE" "$_agent"
  git -C "$_agent" config user.email test@example.invalid
  git -C "$_agent" config user.name test
  git -C "$_agent" checkout --quiet "$ALUCARD_TEST_BRANCH"
  printf 'agent-push-%s\n' "$_n" >> "$_agent/README.md"
  git -C "$_agent" commit --quiet -am "agent $_n"
  git -C "$_agent" push --quiet origin "$ALUCARD_TEST_BRANCH"
  rm -rf "$_agent"
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
    echo "no checks reported on the '$ALUCARD_TEST_BRANCH' branch" >&2
    exit 1 ;;
  "pr list")   printf '%s\n' "$ALUCARD_TEST_PR" ;;
  "pr view")
    case "$*" in
      *state,headRefName*) printf '{"state":"OPEN","headRefName":"%s"}\n' "$ALUCARD_TEST_BRANCH" ;;
      *comments*)          printf '%s\n' "$ALUCARD_TEST_COMMENTS" ;;
      *reviews*)           printf '\n' ;;
      *headRefOid*)
        # Current tip of the branch on the remote — must move when an agent pushes.
        git -C "$ALUCARD_TEST_TARGET" fetch --quiet origin "$ALUCARD_TEST_BRANCH" 2>/dev/null || true
        git -C "$ALUCARD_TEST_TARGET" rev-parse "origin/$ALUCARD_TEST_BRANCH" 2>/dev/null \
          || printf 'deadbeef\n' ;;
      *baseRefOid*)        printf 'baseoid\n' ;;
      *)                   printf '{}\n' ;;
    esac ;;
  "pr comment") : ;;
  "api") : ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

MAX_TURNS_RESULT='{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}'

run_continue() {
  : > "$TRACE"
  : > "$GH_TRACE"
  rm -rf "$TEST_DIR/logs"
  # Reset the branch tip so a prior scenario's push does not leak.
  git -C "$TARGET" fetch --quiet origin "$BRANCH"
  git -C "$TARGET" push --quiet origin "origin/main:refs/heads/$BRANCH" --force
  git -C "$TARGET" fetch --quiet origin "$BRANCH"
  # Re-seed a one-commit tip on the branch for a clean before/after SHA.
  _seed=$(mktemp -d "$TEST_DIR/seed.XXXXXX")
  git clone --quiet "$REMOTE" "$_seed"
  git -C "$_seed" config user.email test@example.invalid
  git -C "$_seed" config user.name test
  git -C "$_seed" checkout --quiet "$BRANCH" 2>/dev/null \
    || git -C "$_seed" checkout --quiet -b "$BRANCH"
  printf 'seed\n' > "$_seed/README.md"
  git -C "$_seed" add README.md
  git -C "$_seed" commit --quiet -m seed || true
  git -C "$_seed" push --quiet origin "$BRANCH" --force
  rm -rf "$_seed"

  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_TRACE="$TRACE" \
  ALUCARD_TEST_GH_TRACE="$GH_TRACE" \
  ALUCARD_TEST_SCRIPT="$TEST_DIR/script" \
  ALUCARD_TEST_PR=88 \
  ALUCARD_TEST_BRANCH="$BRANCH" \
  ALUCARD_TEST_COMMENTS="$COMMENTS_JSON" \
  ALUCARD_TEST_REMOTE="$REMOTE" \
  ALUCARD_TEST_TARGET="$TARGET" \
  ALUCARD_TEST_DIR="$TEST_DIR" \
  ALUCARD_TRANSPORT_RETRY_ATTEMPTS=1 \
    "$ALUCARD" continue 88 "$TARGET" --no-build --max-review-cycles 1 \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
  CONTINUE_RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  EVENTS=$(cat "$TEST_DIR/logs"/alucard-*/events.log 2>/dev/null || true)
  GH=$(<"$GH_TRACE")
}

# ── Scenario 1: feedback dies without pushing ────────────────────────────────
echo "── continue: feedback stalled ──"
printf '1 %s\n' "$MAX_TURNS_RESULT" > "$TEST_DIR/script"
run_continue

assert_eq "continue exits zero after handing off" "0" "$CONTINUE_RC"
assert_eq "only the feedback agent ran — gates did not" \
  "1" "$(grep -c '^agent launch$' "$TRACE" || true)"
assert_contains "the stall is logged" \
  "stopping before gates" "$EVENTS"
assert_contains "the PR gets the needs-human label" \
  "issues/88/labels" "$GH"
assert_contains "the comment names the continue stall" \
  "feedback agent failed — needs human" "$GH"
assert_contains "the comment points at the turn cap" \
  "ALUCARD_FEEDBACK_MAX_TURNS" "$GH"
assert_not_contains "review gate never starts" \
  "Review gate" "$OUT"

# ── Scenario 2: feedback dies after pushing — gates still run ────────────────
echo ""
echo "── continue: feedback failed after push ──"
# feedback exits 1 after pushing; reviewer then runs and posts no verdict.
{
  printf '1 %s push\n' "$MAX_TURNS_RESULT"
  printf '1 %s\n' "$MAX_TURNS_RESULT"
} > "$TEST_DIR/script"
run_continue

assert_eq "feedback plus reviewer both ran" \
  "2" "$(grep -c '^agent launch$' "$TRACE" || true)"
assert_contains "the partial failure is said on the PR" \
  "feedback agent failed after pushing" "$GH"
assert_contains "gates ran — review gate started" \
  "Review gate" "$OUT"
assert_not_contains "a push is not treated as a stall" \
  "stopping before gates" "$EVENTS"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
