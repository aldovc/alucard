#!/bin/bash
# A recovery PR is opened as a draft and labeled needs-human by the harness.
# When the review gate later approves it, the harness must undo both: mark the
# PR ready and take the label off. One approved recovery PR sat for an hour
# looking gated on a human until someone ran `gh pr ready` by hand (#97).
# Every other verdict leaves draft state and the label exactly as they were.
# Driven through `alucard continue`, which reaches the review gate without
# running the worker loop.
set -euo pipefail

exec </dev/null

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

TEST_DIR=$(mktemp -d /tmp/alucard_test_approved_draft.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
GH_TRACE="$TEST_DIR/gh-trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN" "$TARGET"
touch "$GH_TRACE"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

BRANCH="alucard/20260915-131124/iter-1-issue-534"
git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main
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

# Reviewer and feedback containers. The reviewer (rw output mount) writes the
# verdict the scenario asks for; the feedback agent changes nothing.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
for _a in "$@"; do
  case "$_a" in
    *:/work-output:rw)
      printf '%s\n' "$ALUCARD_TEST_VERDICT" > "${_a%%:*}/.alucard-review"
      printf 'one finding\n' > "${_a%%:*}/.alucard-review-body" ;;
  esac
done
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

# GitHub. The PR's draft state and labels come from the scenario; every call
# is traced so the assertions read what the harness actually asked for.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_GH_TRACE"
case "$1 $2" in
  "pr checks") echo "no checks reported" >&2; exit 1 ;;
  "pr list")   printf '77\n' ;;
  "pr view")
    case "$*" in
      *isDraft*)           printf '%s\n%s\n' "$ALUCARD_TEST_DRAFT" "$ALUCARD_TEST_LABELS" ;;
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

# $1 = verdict, $2 = isDraft, $3 = comma-joined labels
run_continue() {
  : > "$GH_TRACE"
  rm -rf "$TEST_DIR/logs"
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_GH_TRACE="$GH_TRACE" \
  ALUCARD_TEST_BRANCH="$BRANCH" \
  ALUCARD_TEST_VERDICT="$1" \
  ALUCARD_TEST_DRAFT="$2" \
  ALUCARD_TEST_LABELS="$3" \
  ALUCARD_TRANSPORT_RETRY_ATTEMPTS=0 \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 1 \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  EVENTS=$(cut -f2- "$TEST_DIR"/logs/alucard-*/events.log)
  TRACE=$(<"$GH_TRACE")
  APPROVED_COMMENT=$(grep -F 'pr comment 77' <<<"$TRACE" | grep -F 'APPROVED' || true)
}

READY_CALL="gh pr ready 77"
UNLABEL_CALL="issues/77/labels/needs-human"

echo "── APPROVED on a draft recovery PR labeled needs-human ──"
run_continue APPROVED true "alucard,needs-human"
assert_eq "the continue completes" "0" "$RC"
assert_contains "the gate approves" "Review gate: PR #77 approved" "$EVENTS"
assert_contains "the draft is marked ready" "$READY_CALL" "$TRACE"
assert_contains "and the event log says so" "PR #77 marked ready for review" "$EVENTS"
assert_contains "needs-human comes off through the REST endpoint" "-X DELETE" "$TRACE"
assert_contains "naming that label on this PR" "$UNLABEL_CALL" "$TRACE"
assert_contains "the APPROVED comment says the PR was marked ready" "Marked ready for review" "$APPROVED_COMMENT"
assert_contains "and that the label was removed" "the human step this label asked for is done by this approval" "$APPROVED_COMMENT"

echo ""
echo "── APPROVED on an ordinary PR ──"
run_continue APPROVED false "alucard"
assert_eq "the continue completes" "0" "$RC"
assert_contains "the gate approves" "Review gate: PR #77 approved" "$EVENTS"
assert_not_contains "a PR that is not a draft is not marked ready" "$READY_CALL" "$TRACE"
assert_not_contains "a PR without the label gets no removal" "$UNLABEL_CALL" "$TRACE"
assert_not_contains "and the comment carries no such note" "Marked ready" "$APPROVED_COMMENT"

echo ""
echo "── APPROVED on a draft that carries only the alucard label ──"
run_continue APPROVED true "alucard"
assert_contains "the draft is marked ready" "$READY_CALL" "$TRACE"
assert_not_contains "nothing is removed" "$UNLABEL_CALL" "$TRACE"

echo ""
echo "── CHANGES_REQUESTED on a draft recovery PR ──"
run_continue CHANGES_REQUESTED true "alucard,needs-human"
assert_not_contains "the draft stays a draft" "$READY_CALL" "$TRACE"
assert_not_contains "and keeps needs-human" "$UNLABEL_CALL" "$TRACE"

echo ""
echo "── BLOCKED on a draft recovery PR ──"
run_continue BLOCKED true "alucard,needs-human"
assert_contains "the gate blocks" "blocked on human action" "$EVENTS"
assert_not_contains "the draft stays a draft" "$READY_CALL" "$TRACE"
assert_not_contains "and keeps needs-human" "$UNLABEL_CALL" "$TRACE"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
