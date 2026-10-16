#!/bin/bash
# `alucard merge` rebases a harness-approved PR onto its base, runs the
# operator's check command, posts the result, and squash-merges. Driven against
# a real bare remote; gh is mocked and traced.
set -euo pipefail

exec </dev/null

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
if [ -z "${ALUCARD:-}" ]; then
  ALUCARD="$SCRIPT_DIR/../alucard"
  # shellcheck disable=SC1090
  source "$ALUCARD"
  USES_RUST_BINARY=false
else
  BOT_REVIEWED_HEAD="Reviewed head:"
  USES_RUST_BINARY=true
fi

TEST_DIR=$(mktemp -d /tmp/alucard_test_merge.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
STATE="$TEST_DIR/state"
GH_TRACE="$STATE/gh-trace"
LEDGER="$TEST_DIR/ledger.db"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
BRANCH="alucard/20261002-090000/iter-1-issue-9"
mkdir -p "$MOCK_BIN" "$STATE"
: > "$TEST_DIR/alucard.env"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

# gh: PR metadata and comments come from the scenario; the head is read from
# the bare remote, as GitHub would report it after a push.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_STATE/gh-trace"
case "$1 $2" in
  "api user") printf '%s\n' alucard-bot ;;
  "pr view")
    case "$*" in
      *labels,comments*)
        if [ -f "$ALUCARD_TEST_STATE/recovery-labelled" ]; then
          printf '%s\n' '{"labels":[{"name":"alucard"}],"comments":[]}'
        else
          printf '%s\n' '{"labels":[],"comments":[]}'
        fi ;;
      *headRefOid*)
        calls=$(cat "$ALUCARD_TEST_STATE/pr-head-calls" 2>/dev/null || echo 0)
        calls=$((calls + 1))
        printf '%s\n' "$calls" > "$ALUCARD_TEST_STATE/pr-head-calls"
        if [ -n "${ALUCARD_TEST_STALE_PR_HEAD:-}" ] \
          && [ "$calls" -le "${ALUCARD_TEST_STALE_PR_HEAD_CALLS:-0}" ]; then
          printf '%s\n' "$ALUCARD_TEST_STALE_PR_HEAD"
        else
          : > "$ALUCARD_TEST_STATE/pr-head-advanced"
          git --git-dir="$ALUCARD_TEST_REMOTE" rev-parse "refs/heads/$ALUCARD_TEST_BRANCH"
        fi ;;
      *) jq -n --arg b "$ALUCARD_TEST_BRANCH" --argjson c "$ALUCARD_TEST_COMMENTS" \
           '{state: "OPEN", headRefName: $b, baseRefName: "main",
             isCrossRepository: false, comments: $c}' ;;
    esac ;;
  "api --silent") : > "$ALUCARD_TEST_STATE/recovery-labelled" ;;
  "pr comment")
    while [ "$#" -gt 0 ]; do
      [ "$1" = "--body" ] && { printf '%s\n' "$2" >> "$ALUCARD_TEST_STATE/comments"; break; }
      shift
    done ;;
  "pr merge")
    if [ -n "${ALUCARD_TEST_STALE_PR_HEAD:-}" ] \
      && [ ! -f "$ALUCARD_TEST_STATE/pr-head-advanced" ]; then
      echo 'merge attempted before PR head advanced' >&2
      exit 1
    fi ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/gh"

approval_comments() {
  jq -n --arg body "**🤖 Alucard review cycle 1/10: APPROVED**

${BOT_REVIEWED_HEAD} \`$1\`

LGTM" --arg login "${2:-alucard-bot}" '[{author: {login: $login}, body: $body}]'
}

# A base and a PR branch; `stale` advances the base after the branch forked,
# `conflict` advances it with an edit to the same line.
setup_repos() {
  local shape="$1"
  rm -rf "$REMOTE" "$TARGET" "${STATE:?}"/*
  : > "$GH_TRACE"
  git init -q --bare -b main "$REMOTE"
  git init -q -b main "$TARGET"
  printf 'one\n' > "$TARGET/a.txt"
  git -C "$TARGET" add . && git -C "$TARGET" commit -qm base
  git -C "$TARGET" remote add origin "$REMOTE"
  git -C "$TARGET" push -q origin main
  git -C "$TARGET" checkout -q -b "$BRANCH"
  printf 'pr change\n' > "$TARGET/b.txt"
  [ "$shape" = conflict ] && printf 'pr\n' > "$TARGET/a.txt"
  git -C "$TARGET" add . && git -C "$TARGET" commit -qm pr
  git -C "$TARGET" push -q origin "$BRANCH"
  PR_SHA=$(git -C "$TARGET" rev-parse HEAD)
  git -C "$TARGET" checkout -q main
  case "$shape" in
    stale|conflict)
      printf 'base moved\n' > "$TARGET/a.txt"
      git -C "$TARGET" commit -qam "base moved"
      git -C "$TARGET" push -q origin main ;;
  esac
  BASE_SHA=$(git -C "$TARGET" rev-parse main)
  COMMENTS=$(approval_comments "$PR_SHA")
}

run_merge() {
  local -a command=("$ALUCARD")
  if "$USES_RUST_BINARY"; then
    command+=(--db-path "$LEDGER")
  fi
  command+=(merge 9 "$TARGET" --env-file "$TEST_DIR/alucard.env" \
    --logs-root "$TEST_DIR/logs" "$@")
  if [ -n "${MERGE_TIMEOUT:-}" ]; then
    command=(timeout --kill-after 1s "$MERGE_TIMEOUT" "${command[@]}")
  fi
  if [ "${NO_LOG_FALLBACK:-}" = true ]; then
    command=(env -u HOME -u XDG_STATE_HOME -u ALUCARD_LOG_ROOT "${command[@]}")
  fi
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_STATE="$STATE" ALUCARD_TEST_REMOTE="$REMOTE" \
  ALUCARD_TEST_BRANCH="$BRANCH" ALUCARD_TEST_COMMENTS="$COMMENTS" \
    "${command[@]}" > "$TEST_DIR/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  TRACE=$(<"$GH_TRACE")
  POSTED=$(cat "$STATE/comments" 2>/dev/null || true)
  REMOTE_HEAD=$(git --git-dir="$REMOTE" rev-parse "refs/heads/$BRANCH")
}

assert_merge_lifecycle() {
  local expected_status="$1"
  local run_id run
  "$USES_RUST_BINARY" || return 0
  run_id=$("$ALUCARD" --db-path "$LEDGER" runs | awk -F '\t' '$3 == "merge" && !run_id { run_id=$1 } END { print run_id }')
  run=$("$ALUCARD" --db-path "$LEDGER" runs show "$run_id")
  assert_contains "the merge run is terminal" "status: $expected_status" "$run"
  assert_contains "the PR lease is released" "claim: pr 9 released" "$run"
}

seed_pending_recovery_annotation() {
  "$USES_RUST_BINARY" || return 0
  "$ALUCARD" --db-path "$LEDGER" db migrate >/dev/null
  python3 - "$LEDGER" "$(git -C "$TARGET" remote get-url origin)" <<'PY'
import sqlite3
import sys

database, repository_id = sys.argv[1:]
repository_id = "/".join(repository_id.rstrip("/").removesuffix(".git").rsplit("/", 2)[-2:]).lower()
connection = sqlite3.connect(database)
connection.execute(
    "INSERT OR IGNORE INTO repositories (id, owner, name, created_at) VALUES (?, '', 'target', datetime('now'))",
    (repository_id,),
)
connection.execute(
    "INSERT INTO operations (id, repository_id, target, operation_type, phase, attempt_count, created_at, updated_at) VALUES ('pending-recovery-annotation', ?, 'recovery-pr:7', 'alucard-label', 'pending', 0, datetime('now'), datetime('now'))",
    (repository_id,),
)
connection.commit()
PY
}

pending_recovery_annotation_phase() {
  python3 - "$LEDGER" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
print(connection.execute("SELECT phase FROM operations WHERE id = 'pending-recovery-annotation'").fetchone()[0])
PY
}

# The check records the tree it ran on, so a test can tell it saw the rebase.
PASSING_CHECK='cat a.txt > "$ALUCARD_TEST_STATE/checked"; echo "== 12 passed =="'

echo "── stale but clean approved PR ──"
setup_repos stale
export ALUCARD_TEST_STALE_PR_HEAD="$PR_SHA" ALUCARD_TEST_STALE_PR_HEAD_CALLS=1
ALUCARD_TEST_STATE="$STATE" run_merge --check-command "$PASSING_CHECK"
unset ALUCARD_TEST_STALE_PR_HEAD ALUCARD_TEST_STALE_PR_HEAD_CALLS
assert_eq "merge succeeds" "0" "$RC"
assert_contains "a logs root under /tmp warns, as for run" "will not survive a reboot" "$OUT"
assert_eq "the checks ran on the rebased tree" "base moved" "$(cat "$STATE/checked" 2>/dev/null)"
if [ "$REMOTE_HEAD" != "$PR_SHA" ] \
   && git --git-dir="$REMOTE" merge-base --is-ancestor "$BASE_SHA" "$REMOTE_HEAD"; then
  pass "the rebased branch is pushed on top of the base"
else
  fail "the rebased branch is pushed on top of the base"
fi
assert_contains "the evidence comment says the checks passed" "Alucard merge: checks passed" "$POSTED"
assert_contains "and carries the check's summary" "== 12 passed ==" "$POSTED"
assert_contains "squash-merges exactly the commit it checked" \
  "gh pr merge 9 --squash --match-head-commit $REMOTE_HEAD" "$TRACE"
assert_eq "the stale PR head is polled until it advances" "2" \
  "$(cat "$STATE/pr-head-calls")"
assert_contains "and deletes the branch" "git/refs/heads/$BRANCH" "$TRACE"
assert_merge_lifecycle completed

if "$USES_RUST_BINARY"; then
  echo ""
  echo "── explicit logs root without fallback environment ──"
  setup_repos current
  NO_LOG_FALLBACK=true run_merge --check-command 'true'
  unset NO_LOG_FALLBACK
  assert_eq "merge succeeds without a log-root fallback" "0" "$RC"
  assert_contains "the merge log is persisted below the flag root" "$TEST_DIR/logs/" \
    "$("$ALUCARD" --db-path "$LEDGER" runs | awk -F '\t' '$3 == "merge" { print $4; exit }')"
fi

echo ""
echo "── approved head already contains the base ──"
setup_repos current
run_merge --check-command 'true'
assert_eq "merge succeeds" "0" "$RC"
assert_eq "nothing is force-pushed" "$PR_SHA" "$REMOTE_HEAD"
assert_contains "the approved commit is merged" "--match-head-commit $PR_SHA" "$TRACE"

echo ""
echo "── rebase conflict ──"
setup_repos conflict
run_merge --check-command 'true'
assert_eq "merge fails" "1" "$RC"
assert_eq "nothing is pushed" "$PR_SHA" "$REMOTE_HEAD"
assert_contains "the comment names the conflicting path" '- `a.txt`' "$POSTED"
assert_contains "the PR is labelled needs-human" "needs-human" "$(grep -F -- '-X POST' <<<"$TRACE")"
assert_not_contains "nothing is merged" "pr merge" "$TRACE"
assert_merge_lifecycle failed

echo ""
echo "── rebase fails without a conflict ──"
setup_repos stale
mkdir -p "$STATE/hooks"
printf '#!/bin/sh\nexit 1\n' > "$STATE/hooks/pre-rebase"
chmod +x "$STATE/hooks/pre-rebase"
export GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0="$STATE/hooks"
run_merge --check-command 'true'
unset GIT_CONFIG_COUNT GIT_CONFIG_KEY_0 GIT_CONFIG_VALUE_0
assert_eq "merge fails" "1" "$RC"
assert_contains "the non-conflict failure is reported" "failed without a conflict" "$OUT"
assert_eq "nothing is pushed" "$PR_SHA" "$REMOTE_HEAD"
assert_eq "no conflict handoff is posted" "" "$POSTED"
assert_not_contains "the PR is not labelled needs-human" "needs-human" "$TRACE"
assert_not_contains "nothing is merged" "pr merge" "$TRACE"
assert_merge_lifecycle failed

echo ""
echo "── checks fail after the rebase ──"
setup_repos stale
run_merge --check-command 'echo "FAILED tests/db/test_x.py"; exit 3'
assert_eq "merge fails" "1" "$RC"
assert_eq "nothing is pushed" "$PR_SHA" "$REMOTE_HEAD"
assert_contains "the failure summary is posted" "checks failed — not merged" "$POSTED"
assert_contains "with the failing output" "FAILED tests/db/test_x.py" "$POSTED"
assert_not_contains "nothing is merged" "pr merge" "$TRACE"

echo ""
echo "── background check child ──"
setup_repos current
MERGE_TIMEOUT=5s run_merge --check-command 'sleep 10 & echo "check completed"'
assert_eq "merge returns after its foreground check completes" "0" "$RC"
assert_contains "the foreground check output is recorded" "check completed" "$POSTED"

echo ""
echo "── refusals ──"
setup_repos stale
COMMENTS=$(approval_comments 1111111111111111111111111111111111111111)
run_merge --check-command 'true'
assert_eq "an approval of an older head is refused" "1" "$RC"
assert_contains "naming the mismatch" "is not the commit the harness approved" "$OUT"
assert_eq "nothing is pushed" "$PR_SHA" "$REMOTE_HEAD"

COMMENTS=$(approval_comments "$PR_SHA" passer-by)
run_merge --check-command 'true'
assert_eq "an approval comment someone else posted is refused" "1" "$RC"

COMMENTS=$(jq -n --argjson a "$(approval_comments "$PR_SHA")" \
  '$a + [{author: {login: "alucard-bot"}, body: "**🤖 Alucard review cycle 2/10: CHANGES_REQUESTED**\n\nfix it"}]')
run_merge --check-command 'true'
assert_eq "an approval superseded by a later CHANGES_REQUESTED is refused" "1" "$RC"

COMMENTS='[]'
run_merge --check-command 'true'
assert_eq "an unreviewed PR is refused" "1" "$RC"
assert_contains "pointing at continue" "alucard continue 9" "$OUT"
assert_eq "no refusal comments" "" "$POSTED"
assert_not_contains "and no merge" "pr merge" "$TRACE"

echo ""
echo "── no check command ──"
setup_repos current
run_merge
assert_eq "merge refuses without a check command" "1" "$RC"
assert_contains "and says how to give one" "ALUCARD_CHECK_COMMAND" "$OUT"

echo ""
echo "── parser-only merge options ──"
setup_repos current
run_merge --help
assert_eq "merge help does not run a merge" "1" "$RC"
assert_contains "merge help keeps its usage result" "usage: alucard merge" "$OUT"
assert_eq "merge help makes no GitHub calls" "" "$TRACE"
run_merge --issue 7
assert_eq "merge rejects the run-only issue option" "1" "$RC"
assert_contains "merge names the run-only issue option" "--issue is only valid with alucard run" "$OUT"

echo ""
echo "── dry run ──"
setup_repos stale
seed_pending_recovery_annotation
if "$USES_RUST_BINARY"; then
  run_merge
  assert_eq "invalid merge configuration does not replay recovery" "" "$TRACE"
  assert_eq "invalid merge configuration leaves recovery pending" "pending" "$(pending_recovery_annotation_phase)"
fi

run_merge --check-command "$PASSING_CHECK" --dry-run
assert_eq "a passing dry run succeeds" "0" "$RC"
assert_eq "nothing is pushed" "$PR_SHA" "$REMOTE_HEAD"
assert_eq "nothing is commented" "" "$POSTED"
assert_not_contains "nothing is merged" "pr merge" "$TRACE"
assert_not_contains "nothing is recovered" "pr edit" "$TRACE"
assert_contains "the evidence is printed instead" "would comment on PR #9" "$OUT"
assert_eq "the dry-run check sees the rebased tree" "base moved" "$(cat "$STATE/checked")"
if "$USES_RUST_BINARY"; then
  assert_eq "dry run leaves recovery pending" "pending" "$(pending_recovery_annotation_phase)"
  run_merge --check-command 'true'
  assert_eq "a real merge replays recovery" "done" "$(pending_recovery_annotation_phase)"
  assert_contains "the pending recovery annotation is replayed" \
    "api --silent -X POST" "$TRACE"
fi

finish
