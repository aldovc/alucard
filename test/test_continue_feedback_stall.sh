#!/bin/bash
# `alucard continue` runs feedback once, then the gates. A feedback agent that
# dies without advancing the branch must stop before those gates — the same
# stall decision review_gate already makes inside its loop — and hand the PR to
# a human. Driven through continue with mocked docker/gh.
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
: "${ALUCARD:?set ALUCARD to the alucard binary}"
source "$SCRIPT_DIR/lib.sh"
export ALUCARD_CI_POLL_INTERVAL=0

TEST_DIR=$(mktemp -d /tmp/alucard_test_continue_stall.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
GH_TRACE="$TEST_DIR/gh-trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
LEDGER="${CONTINUE_TEST_LEDGER:-$TEST_DIR/ledger.db}"
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

if [ "${_line##* }" = "checkpoint" ]; then
  _json="${_json% checkpoint}"
  for _a in "$@"; do
    case "$_a" in
      *:/work:rw)
        _wt="${_a%:/work:rw}"
        git -C "$_wt" config user.email test@example.invalid
        git -C "$_wt" config user.name test
        printf 'checkpoint\n' >> "$_wt/README.md"
        git -C "$_wt" commit --quiet -am checkpoint
        printf '%s\n' "$_wt" > "$ALUCARD_TEST_DIR/retained-path" ;;
    esac
  done
fi

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
      *state,headRefName*)
        # Current tip of the branch on the remote — must move when an agent pushes.
        git -C "$ALUCARD_TEST_TARGET" fetch --quiet origin "$ALUCARD_TEST_BRANCH" 2>/dev/null || true
        _head=$(git -C "$ALUCARD_TEST_TARGET" rev-parse "origin/$ALUCARD_TEST_BRANCH" 2>/dev/null \
          || printf 'deadbeef')
        _base=$(git -C "$ALUCARD_TEST_TARGET" rev-parse origin/main 2>/dev/null || printf 'baseoid')
        jq -c --arg branch "$ALUCARD_TEST_BRANCH" --arg head "$_head" --arg base "$_base" \
          '. + {state:"OPEN",headRefName:$branch,headRefOid:$head,baseRefOid:$base,title:"test pull request"}' \
          <<< "$ALUCARD_TEST_COMMENTS" ;;
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
  "api user") printf 'bot\n' ;;
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
  local -a command=("$ALUCARD" --db-path "$LEDGER" continue 88 "$TARGET" --no-build --max-review-cycles 1 \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs")
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
    "${command[@]}" > "$TEST_DIR/out" 2>&1
  CONTINUE_RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  EVENTS=$(cat "$TEST_DIR/logs"/*/events.log 2>/dev/null || true)
  GH=$(<"$GH_TRACE")
}

# ── Scenario 1: feedback dies without pushing ────────────────────────────────
echo "── continue: feedback stalled ──"
printf '1 %s checkpoint\n' "$MAX_TURNS_RESULT" > "$TEST_DIR/script"
run_continue

assert_eq "continue exits zero after handing off" "0" "$CONTINUE_RC"
assert_eq "only the feedback agent ran — gates did not" \
  "1" "$(grep -c '^agent launch$' "$TRACE" || true)"
assert_contains "the PR gets the needs-human label" \
  "issues/88/labels" "$GH"
assert_contains "the comment names the continue stall" \
  "feedback agent stopped without publishing — needs human" "$GH"
assert_not_contains "review gate never starts" \
  "Review gate" "$OUT"
retained=$(<"$TEST_DIR/retained-path")
assert_eq "the unpublished commit survives process-exit cleanup" \
  "checkpoint" "$(git -C "$retained" log -1 --format=%s)"
assert_contains "the recovery path is recorded" "$retained" "$EVENTS"
assert_not_contains "the checkpoint was not pushed" "checkpoint" \
  "$(git --git-dir="$REMOTE" log -1 --format=%s "$BRANCH")"

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

# ── Scenario 3: a later approval supersedes the change request ───────────────
echo ""
echo "── continue: findings already approved ──"
# The reviewer is the only agent; with stale findings, feedback would run
# first, push nothing, and stop before the gates.
COMMENTS_JSON=$(jq -n --arg cr "$CR_BODY" \
  --arg ok '**🤖 Alucard review cycle 2/3: APPROVED**' \
  '{comments:[{body:$cr,createdAt:"2026-01-01T00:00:00Z",author:{login:"bot"}},
              {body:$ok,createdAt:"2026-01-02T00:00:00Z",author:{login:"bot"}}]}')
printf '1 %s\n' "$MAX_TURNS_RESULT" > "$TEST_DIR/script"
run_continue

assert_eq "only the reviewer ran" "1" "$(grep -c '^agent launch$' "$TRACE" || true)"
assert_contains "the review gate started without feedback" "Review gate" "$OUT"

# ── Scenario 4: an authenticated recovery PR with no ledger row is adopted ──
echo ""
echo "── continue: recovery PR adoption ──"
rm -f "$LEDGER"
COMMENTS_JSON=$(jq -n --arg marker '**🤖 Alucard recovery PR parked** — opened as a draft and labeled `needs-human` by the harness. When the review gate approves this PR it marks it ready, removes the label, turns the `Refs` line into `Closes`, and replaces the stub title with the ticket'"'"'s.' \
  '{comments:[{body:$marker,createdAt:"2026-01-01T00:00:00Z",author:{login:"bot"}}]}')
printf '1 %s\n' "$MAX_TURNS_RESULT" > "$TEST_DIR/script"
run_continue

assert_eq "only the reviewer runs for a recovery PR without findings" \
  "1" "$(grep -c '^agent launch$' "$TRACE" || true)"
assert_contains "the adopted recovery PR reaches the review gate" "Review gate" "$OUT"

finish
