#!/bin/bash
# `alucard merge` rebases a harness-approved PR onto its base, runs the
# operator's check command, posts the result, and squash-merges. Driven against
# a real bare remote; gh is mocked and traced.
set -euo pipefail

exec </dev/null

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
ALUCARD="$SCRIPT_DIR/../alucard"

TEST_DIR=$(mktemp -d /tmp/alucard_test_merge.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
STATE="$TEST_DIR/state"
GH_TRACE="$STATE/gh-trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
BRANCH="alucard/20261002-090000/iter-1-issue-9"
mkdir -p "$MOCK_BIN" "$STATE"
: > "$TEST_DIR/alucard.env"
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

# shellcheck disable=SC1090
source "$ALUCARD"

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
      *headRefOid*) git --git-dir="$ALUCARD_TEST_REMOTE" rev-parse "refs/heads/$ALUCARD_TEST_BRANCH" ;;
      *) jq -n --arg b "$ALUCARD_TEST_BRANCH" --argjson c "$ALUCARD_TEST_COMMENTS" \
           '{state: "OPEN", headRefName: $b, baseRefName: "main",
             isCrossRepository: false, comments: $c}' ;;
    esac ;;
  "pr comment")
    while [ "$#" -gt 0 ]; do
      [ "$1" = "--body" ] && { printf '%s\n' "$2" >> "$ALUCARD_TEST_STATE/comments"; break; }
      shift
    done ;;
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
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_STATE="$STATE" ALUCARD_TEST_REMOTE="$REMOTE" \
  ALUCARD_TEST_BRANCH="$BRANCH" ALUCARD_TEST_COMMENTS="$COMMENTS" \
    "$ALUCARD" merge 9 "$TARGET" --env-file "$TEST_DIR/alucard.env" \
      --logs-root "$TEST_DIR/logs" "$@" > "$TEST_DIR/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  TRACE=$(<"$GH_TRACE")
  POSTED=$(cat "$STATE/comments" 2>/dev/null || true)
  REMOTE_HEAD=$(git --git-dir="$REMOTE" rev-parse "refs/heads/$BRANCH")
}

# The check records the tree it ran on, so a test can tell it saw the rebase.
PASSING_CHECK='cat a.txt > "$ALUCARD_TEST_STATE/checked"; echo "== 12 passed =="'

echo "── stale but clean approved PR ──"
setup_repos stale
ALUCARD_TEST_STATE="$STATE" run_merge --check-command "$PASSING_CHECK"
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
assert_contains "and deletes the branch" "git/refs/heads/$BRANCH" "$TRACE"

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
echo "── dry run ──"
setup_repos stale
run_merge --check-command 'true' --dry-run
assert_eq "a passing dry run succeeds" "0" "$RC"
assert_eq "nothing is pushed" "$PR_SHA" "$REMOTE_HEAD"
assert_eq "nothing is commented" "" "$POSTED"
assert_not_contains "nothing is merged" "pr merge" "$TRACE"
assert_contains "the evidence is printed instead" "would comment on PR #9" "$OUT"

finish
