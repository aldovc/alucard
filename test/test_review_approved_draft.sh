#!/bin/bash
# A recovery PR is parked: opened as a draft, labeled needs-human, marked with
# a harness comment. When the review gate later approves it, the harness lifts
# what it set — ready, label off, `Refs #N` to `Closes #N`, stub title replaced
# by the ticket's. One approved recovery PR sat an hour looking gated on a
# human until someone ran `gh pr ready` by hand (#97).
#
# The un-park must be keyed on the harness's own mark and on an approval of
# the current head: a draft a developer keeps on purpose, a needs-human some
# other path set, or a stale formal review must not un-park anything. Every
# other verdict leaves draft state and the label alone. Driven through
# `alucard continue`, which reaches the review gate without the worker loop.
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
STATE="$TEST_DIR/state"
GH_TRACE="$STATE/gh-trace"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN" "$TARGET" "$STATE"
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
# decision file the scenario asks for, or none; the feedback agent changes
# nothing.
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
      if [ -n "${ALUCARD_TEST_VERDICT:-}" ]; then
        printf '%s\n' "$ALUCARD_TEST_VERDICT" > "${_a%%:*}/.alucard-review"
        printf 'one finding\n' > "${_a%%:*}/.alucard-review-body"
      fi ;;
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

# GitHub. The PR's state comes from the scenario. Every call is traced, and a
# REST edit's payload is kept, so the assertions read what the harness sent.
# The formal-review lookups are answered by the jq expression they carry, as
# the real gh would.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_STATE/gh-trace"
case "$1 $2" in
  "pr checks") echo "no checks reported" >&2; exit 1 ;;
  "pr list")   printf '77\n' ;;
  "pr ready")  exit "${ALUCARD_TEST_READY_RC:-0}" ;;
  "issue view") printf 'Small fix\n' ;;
  "pr view")
    case "$*" in
      *isDraft*)
        jq -n --argjson d "$ALUCARD_TEST_DRAFT" --argjson l "$ALUCARD_TEST_LABELS" \
          --arg t "$ALUCARD_TEST_TITLE" --arg b "$ALUCARD_TEST_BODY" \
          '{isDraft: $d, labels: ($l | map({name: .})), title: $t, body: $b}' ;;
      *state,headRefName*) printf '{"state":"OPEN","headRefName":"%s"}\n' "$ALUCARD_TEST_BRANCH" ;;
      *comments*)          printf '{"comments":%s}\n' "$ALUCARD_TEST_COMMENTS" ;;
      *reviews*)
        case "$*" in
          *".state // empty"*) printf '%s\n' "${ALUCARD_TEST_FORMAL_STATE:-}" ;;
          *".commit.oid"*)     printf '%s\n' "${ALUCARD_TEST_FORMAL_OID:-}" ;;
          *)                   [ -n "${ALUCARD_TEST_FORMAL_STATE:-}" ] && printf 'formal review body\n' || printf '\n' ;;
        esac ;;
      *headRefOid*)        printf 'deadbeef\n' ;;
      *)                   printf '{}\n' ;;
    esac ;;
  "api --silent")
    # Read the payload like the real gh does; an unread pipe fails jq upstream.
    case "$*" in
      *PATCH*) cat > "$ALUCARD_TEST_STATE/patch.json" ;;
      *)       cat > /dev/null ;;
    esac ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

PARKED_MARK='[{"body":"**🤖 Alucard recovery PR parked** — opened as a draft and labeled needs-human by the harness."}]'
STUB_TITLE='wip: alucard recovery — iter 1 (worker stopped: exhausted, rc=1)'
STUB_BODY=$'Refs #534\n\n**🤖 Alucard recovery PR** — the worker ran out of turns (rc=1) before opening a PR.\n\nThis is not a finished change.'

# The default scenario: a parked recovery PR exactly as the harness left it,
# approved by this cycle's reviewer through its decision file.
reset_pr() {
  VERDICT=APPROVED
  DRAFT=true
  LABELS='["alucard","needs-human"]'
  TITLE="$STUB_TITLE"
  BODY="$STUB_BODY"
  COMMENTS="$PARKED_MARK"
  READY_RC=0
  FORMAL_STATE=""
  FORMAL_OID=""
}

run_continue() {
  rm -rf "$TEST_DIR/logs" "$STATE/patch.json"
  : > "$GH_TRACE"
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_STATE="$STATE" \
  ALUCARD_TEST_BRANCH="$BRANCH" \
  ALUCARD_TEST_VERDICT="$VERDICT" \
  ALUCARD_TEST_DRAFT="$DRAFT" \
  ALUCARD_TEST_LABELS="$LABELS" \
  ALUCARD_TEST_TITLE="$TITLE" \
  ALUCARD_TEST_BODY="$BODY" \
  ALUCARD_TEST_COMMENTS="$COMMENTS" \
  ALUCARD_TEST_READY_RC="$READY_RC" \
  ALUCARD_TEST_FORMAL_STATE="$FORMAL_STATE" \
  ALUCARD_TEST_FORMAL_OID="$FORMAL_OID" \
  ALUCARD_TRANSPORT_RETRY_ATTEMPTS=0 \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 1 \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  EVENTS=$(cut -f2- "$TEST_DIR"/logs/alucard-*/events.log 2>/dev/null || true)
  TRACE=$(<"$GH_TRACE")
  APPROVED_COMMENT=$(grep -F 'pr comment 77' <<<"$TRACE" | grep -F 'APPROVED' || true)
  PATCH=$(cat "$STATE/patch.json" 2>/dev/null || true)
}

READY_CALL="gh pr ready 77"
UNLABEL_CALL="issues/77/labels/needs-human"

assert_untouched() {
  local why="$1"
  assert_not_contains "$why: not marked ready" "$READY_CALL" "$TRACE"
  assert_not_contains "$why: label kept" "$UNLABEL_CALL" "$TRACE"
  assert_eq "$why: title and body untouched" "" "$PATCH"
}

echo "── APPROVED on a parked recovery PR ──"
reset_pr
run_continue
assert_eq "the continue completes" "0" "$RC"
assert_contains "the gate approves" "Review gate: PR #77 approved" "$EVENTS"
assert_contains "the draft is marked ready" "$READY_CALL" "$TRACE"
assert_contains "and the event log says so" "PR #77 marked ready for review" "$EVENTS"
assert_contains "needs-human comes off through the REST endpoint" "-X DELETE" "$TRACE"
assert_contains "naming that label on this PR" "$UNLABEL_CALL" "$TRACE"
assert_eq "the body's Refs line becomes Closes" "Closes #534" "$(jq -r '.body' <<<"$PATCH" | head -n1)"
assert_contains "the rest of the body is kept" "This is not a finished change." "$(jq -r '.body' <<<"$PATCH")"
assert_eq "the stub title is replaced by the ticket's" "Small fix" "$(jq -r '.title' <<<"$PATCH")"
assert_contains "the APPROVED comment says the PR was marked ready" "Marked ready for review" "$APPROVED_COMMENT"
assert_contains "that the label came off" "is done by this approval" "$APPROVED_COMMENT"
assert_contains "and that the ticket now closes on merge" "the ticket closes when this merges" "$APPROVED_COMMENT"

echo ""
echo "── APPROVED on a draft with needs-human that the harness never parked ──"
reset_pr
COMMENTS='[]'
TITLE="feat: kept in draft on purpose"
BODY=$'Closes #12\n\nA developer'"'"'s own draft, labelled by a maintainer.'
run_continue
assert_contains "the gate approves" "Review gate: PR #77 approved" "$EVENTS"
assert_untouched "not the harness's recovery PR"
assert_not_contains "and the comment carries no un-park note" "Marked ready" "$APPROVED_COMMENT"

echo ""
echo "── APPROVED but marking ready fails ──"
reset_pr
READY_RC=1
run_continue
assert_contains "the failure is logged" "could not mark PR #77 ready" "$EVENTS"
assert_not_contains "the label stays while the PR is still a draft" "$UNLABEL_CALL" "$TRACE"
assert_contains "and the comment says the label stays and what to do" "stays on it" "$APPROVED_COMMENT"

echo ""
echo "── a recovery PR the feedback agent already rewrote, labelled Needs-Human ──"
reset_pr
COMMENTS='[]'
DRAFT=false
LABELS='["alucard","Needs-Human"]'
TITLE="test(db): utilities readings and exchange-rate upserts"
BODY=$'Closes #534\n\n## Summary\n\nDatabase tests.\n\n**🤖 Alucard recovery PR** — kept from the stub.'
run_continue
assert_not_contains "a PR that is not a draft is not marked ready" "$READY_CALL" "$TRACE"
assert_contains "the label comes off whatever its case" "$UNLABEL_CALL" "$TRACE"
assert_eq "nothing to edit, so no edit" "" "$PATCH"

echo ""
echo "── APPROVED comes from a formal review of an older commit ──"
reset_pr
VERDICT=""
FORMAL_STATE=APPROVED
FORMAL_OID=0000000000000000000000000000000000000000
run_continue
assert_contains "the gate still approves" "Review gate: PR #77 approved" "$EVENTS"
assert_contains "but says why it leaves the PR parked" "formal review of an older commit" "$EVENTS"
assert_untouched "stale approval"

echo ""
echo "── APPROVED comes from a formal review of the current head ──"
reset_pr
VERDICT=""
FORMAL_STATE=APPROVED
FORMAL_OID=deadbeef
run_continue
assert_contains "a current formal approval un-parks" "$READY_CALL" "$TRACE"
assert_contains "and the label comes off" "$UNLABEL_CALL" "$TRACE"

echo ""
echo "── CHANGES_REQUESTED on a parked recovery PR ──"
reset_pr
VERDICT=CHANGES_REQUESTED
run_continue
assert_untouched "changes requested"

echo ""
echo "── BLOCKED on a parked recovery PR ──"
reset_pr
VERDICT=BLOCKED
run_continue
assert_contains "the gate blocks" "blocked on human action" "$EVENTS"
assert_untouched "blocked"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
