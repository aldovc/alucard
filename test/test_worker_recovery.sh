#!/bin/bash
# Covers what happens to a GitHub ticket when its worker stops without a PR.
# The contract: a recovery PR is the one thing holding the ticket (Refs, not
# Closes; the in-progress label comes off; the ticket is told where its work
# went; the PR is labeled needs-human), and the run's last lines say what was
# parked instead of reporting a finished queue. One run ended "queue empty"
# with its only ticket locked twice by an unfinished recovery branch.
set -euo pipefail

# The mocks stand in for tools that read stdin; see test_timeout_recovery.sh.
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

# Line order in the event journal: $2 must appear after $3.
assert_line_after() {
  local label="$1" later="$2" earlier="$3" text="$4"
  local a b
  a=$(printf '%s\n' "$text" | grep -nF -- "$earlier" | head -n1 | cut -d: -f1 || true)
  b=$(printf '%s\n' "$text" | grep -nF -- "$later" | head -n1 | cut -d: -f1 || true)
  if [ -n "$a" ] && [ -n "$b" ] && [ "$b" -gt "$a" ]; then
    pass "$label"
  else
    fail "$label ('$later' does not follow '$earlier')"
  fi
}

TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
mkdir -p "$MOCK_BIN"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

# The worker. Finds its worktree from the -v mount and plays one scenario:
#   exhausted — one commit, one uncommitted file, then the max-turns result
#   deferred  — no claim, no changes, a clean exit (it parked the ticket)
#   parked_then_pr — parked #480, then opened a PR for #481
#   failed         — claimed, changed nothing, died
# Touching the flag file is how the gh mock knows the worker has run, so its
# label queries can answer differently before and after.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
wt=""
for _a in "$@"; do
  case "$_a" in *:/work:rw) wt="${_a%%:*}" ;; esac
done
touch "$ALUCARD_TEST_STATE/worker-ran"
case "$ALUCARD_TEST_SCENARIO" in
  exhausted)
    printf 'intake done\n' > "$wt/backend.py"
    git -C "$wt" add -A
    git -C "$wt" -c user.name=w -c user.email=w@example.invalid commit -qm 'feat: backend half'
    printf 'panel, untested\n' > "$wt/panel.tsx"
    printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":181,"result":""}'
    exit 1 ;;
  deferred)
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Parked #480 as ready-for-human; nothing else to pick."}'
    exit 0 ;;
  parked_then_pr)
    printf 'fixed\n' > "$wt/small-fix.txt"
    git -C "$wt" add -A
    git -C "$wt" -c user.name=w -c user.email=w@example.invalid commit -qm 'fix: small fix'
    git -C "$wt" push -qu origin HEAD
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Parked #480, then opened PR #91 for #481."}'
    exit 0 ;;
  failed)
    printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"boom"}'
    exit 1 ;;
esac
MOCK

# GitHub. Traces every call; captures the PR body and ticket comments to files
# so assertions read the text the harness actually sent rather than a %q-quoted
# echo of it. Ticket #480 is the whole queue. `pr create` reproduces what real
# gh does — a warning on the same stream as the URL — because every recovery
# event logged so far had recorded that warning as the PR.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_STATE/trace"
ran=false; [ -f "$ALUCARD_TEST_STATE/worker-ran" ] && ran=true
has_jq=false; for _a in "$@"; do [ "$_a" = "--jq" ] && has_jq=true; done
case "$1 $2" in
  "repo view")    echo "example/api" ;;
  "api graphql")  echo "[]" ;;
  "issue list")
    case "$*" in
      *in-progress*)
        # The worker claims in every scenario but the deferral.
        if $ran && [ "$ALUCARD_TEST_SCENARIO" != deferred ]; then echo "[480]"; else echo "[]"; fi ;;
      *ready-for-agent*)
        if $ran && { [ "$ALUCARD_TEST_SCENARIO" = deferred ] || [ "$ALUCARD_TEST_SCENARIO" = parked_then_pr ]; }; then
          echo "[]"
        elif $has_jq; then
          if [ "$ALUCARD_TEST_SCENARIO" = parked_then_pr ]; then echo "[480,481]"; else echo "[480]"; fi
        else
          if [ "$ALUCARD_TEST_SCENARIO" = parked_then_pr ]; then
            echo '[{"number":480,"title":"Queue receipt ingest and web multi-file drop","body":"A big one.","labels":[{"name":"ready-for-agent"}]},{"number":481,"title":"Small fix","body":"A small one.","labels":[{"name":"ready-for-agent"}]}]'
          else
            echo '[{"number":480,"title":"Queue receipt ingest and web multi-file drop","body":"A big one.","labels":[{"name":"ready-for-agent"}]}]'
          fi
        fi ;;
      *) echo "[]" ;;
    esac ;;
  "pr list")
    if $ran && [ "$ALUCARD_TEST_SCENARIO" = parked_then_pr ]; then echo "91"; else echo ""; fi ;;
  "pr view")
    if [ "$ALUCARD_TEST_SCENARIO" = parked_then_pr ]; then
      echo '{"number":91,"title":"fix: small fix","body":"Closes #481"}'
    fi ;;
  "pr create")
    while [ $# -gt 0 ]; do
      case "$1" in
        --body)  printf '%s' "$2" > "$ALUCARD_TEST_STATE/pr-body"; shift ;;
        --title) printf '%s' "$2" > "$ALUCARD_TEST_STATE/pr-title"; shift ;;
      esac
      shift
    done
    echo "Warning: 1 uncommitted change" >&2
    echo "https://github.com/example/api/pull/91" ;;
  "issue comment")
    n="$3"; shift 3
    while [ $# -gt 0 ]; do
      case "$1" in --body) printf '%s\n---\n' "$2" >> "$ALUCARD_TEST_STATE/issue-comment-$n"; shift ;; esac
      shift
    done ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/timeout" "$MOCK_BIN/docker" "$MOCK_BIN/gh"

# A fresh target and remote per scenario; no tasks file, so the GitHub queue
# is the source.
run_scenario() {
  local name="$1" iterations="$2"
  local root="$TEST_DIR/$name" target remote
  target="$root/target"; remote="$root/remote.git"
  STATE="$root/state"
  mkdir -p "$target" "$STATE"
  git init -q --bare "$remote"
  git -C "$target" init -q -b main
  git -C "$target" config user.email test@example.invalid
  git -C "$target" config user.name test
  printf '# Target\n' > "$target/README.md"
  git -C "$target" add .
  git -C "$target" commit -qm initial
  git -C "$target" remote add origin "$remote"
  git -C "$target" push -qu origin main

  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_SCENARIO="$name" ALUCARD_TEST_STATE="$STATE" \
  ALUCARD_TRANSPORT_RETRY_ATTEMPTS=0 \
  "$ALUCARD" run "$target" --iterations "$iterations" --no-build \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$root/logs" > "$root/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$root/out")
  EVENTS=$(cut -f2- "$root"/logs/alucard-*/events.log)
  TRACE=$(<"$STATE/trace")
  REMOTE_DIR="$remote"
}

# ── The pure texts ───────────────────────────────────────────────────────────
echo "── recovery texts ──"

# shellcheck disable=SC1090
source "$ALUCARD"

body=$(recovery_pr_body "Refs #480" exhausted 1 1 github)
assert_eq "the GitHub body opens with the Refs line" "Refs #480" "$(printf '%s\n' "$body" | head -n1)"
assert_not_contains "and never with a closing keyword" "Closes #" "$body"
assert_contains "it says what continue actually does" "does not resume implementation" "$body"
assert_contains "it says the branch is unverified" "has been through CI or review" "$body"
assert_contains "it names the stop in plain words" "ran out of turns or hit its time limit" "$body"

body=$(recovery_pr_body "Task: 7" wedged 137 3 file)
assert_eq "the local body opens with the Task line" "Task: 7" "$(printf '%s\n' "$body" | head -n1)"
assert_not_contains "a local body never mentions Refs" "Refs" "$body"
assert_contains "a local body describes the heading flip" '`[ ]`' "$body"

body=$(recovery_pr_body "" failed 1 0 github)
assert_eq "an unattributed body starts at the marker, not a blank line" \
  "${BOT_COMMENT_PREFIX} recovery PR**" "$(printf '%s\n' "$body" | head -n1 | cut -d' ' -f1-4)"

comment=$(recovery_ticket_comment 91 exhausted 1 1)
assert_contains "the ticket comment names the PR" "PR #91" "$comment"
assert_contains "and says the label is gone" '`in-progress` label is removed' "$comment"

LOG_DIR="$TEST_DIR/logs-unit"; mkdir -p "$LOG_DIR"
RUN_ATTENTION=()
log_run_end "nothing parked." >/dev/null
assert_eq "an empty attention list adds no lines" "1" "$(wc -l < "$LOG_DIR/events.log")"
RUN_ATTENTION=("one" "two")
log_run_end "two parked." >/dev/null
assert_eq "a run end with two parked items adds four lines" "5" "$(wc -l < "$LOG_DIR/events.log")"
assert_eq "the count line follows the run end" \
  "Needs attention: 2 item(s)" "$(sed -n 3p "$LOG_DIR/events.log" | cut -f2- | cut -d' ' -f1-4)"
assert_eq "then each item, in order" "  - one|  - two" \
  "$(tail -n 2 "$LOG_DIR/events.log" | cut -f2- | paste -sd'|')"

# ── An exhausted worker with work to save ────────────────────────────────────
echo ""
echo "── exhausted worker ──"

run_scenario exhausted 1
assert_eq "the run completes" "0" "$RC"
PR_BODY=$(<"$STATE/pr-body")
assert_eq "the recovery PR body opens with Refs, not Closes" "Refs #480" "$(printf '%s\n' "$PR_BODY" | head -n1)"
assert_not_contains "no closing keyword anywhere in the body" "Closes #" "$PR_BODY"
assert_contains "the title carries the failure class" "exhausted" "$(<"$STATE/pr-title")"
assert_contains "the PR is labeled needs-human" "issues/91/labels" "$TRACE"
assert_contains "with that exact label" "needs-human" "$TRACE"
assert_contains "the claim label comes off the ticket" \
  "gh issue edit 480 --remove-label in-progress" "$TRACE"
if [ -f "$STATE/issue-comment-480" ]; then
  pass "the ticket is told where its work went"
  assert_contains "the comment names the recovery PR" "PR #91" "$(<"$STATE/issue-comment-480")"
  assert_contains "and that nothing was verified" "CI or review" "$(<"$STATE/issue-comment-480")"
else
  fail "the ticket is told where its work went (no comment on #480)"
fi
assert_contains "the PR number is read out of gh's noisy output" \
  "Recovery: opened PR #91 (https://github.com/example/api/pull/91)" "$EVENTS"
assert_not_contains "gh's warning is not logged as the PR" "opened PR Warning" "$EVENTS"
assert_contains "the run end is followed by what was parked" "Needs attention: 1 item(s)" "$EVENTS"
assert_contains "naming the PR and the ticket it holds" "recovery PR #91 holds issue #480" "$EVENTS"
assert_line_after "the attention list comes after the run end line" \
  "Needs attention:" "Run end:" "$EVENTS"
assert_not_contains "a ticket the PR holds is not also reported as deferred" \
  "off the queue" "$EVENTS"

branch=$(git --git-dir="$REMOTE_DIR" for-each-ref --format='%(refname:short)' refs/heads/alucard/ | head -n1)
if [ -n "$branch" ]; then
  pass "the branch was pushed"
  assert_eq "with the worker's commit and the recovery commit" \
    "2" "$(git --git-dir="$REMOTE_DIR" rev-list --count "main..$branch")"
  assert_contains "the uncommitted file is in the pushed tree" \
    "panel.tsx" "$(git --git-dir="$REMOTE_DIR" ls-tree --name-only "$branch")"
else
  fail "the branch was pushed"
fi

# ── A worker that parked its only ticket ─────────────────────────────────────
echo ""
echo "── deferred ticket ──"

run_scenario deferred 2
assert_eq "the run completes" "0" "$RC"
assert_contains "the second iteration finds the queue empty" \
  "Run end: queue empty — alucard rests after 1 iterations." "$EVENTS"
assert_contains "the deferral is named when it happens" \
  "Worker took #480 off the queue without a PR" "$EVENTS"
assert_contains "and again at the end" "#480 left the queue without a PR" "$EVENTS"
assert_line_after "after the queue-empty line" "Needs attention:" "Run end: queue empty" "$EVENTS"
assert_not_contains "nothing was claimed, so nothing is unclaimed" "issue edit" "$TRACE"
assert_not_contains "and no PR is opened for it" "pr create" "$TRACE"

# ── A parked ticket followed by a normal PR ──────────────────────────────────
echo ""
echo "── parked ticket, then PR ──"

run_scenario parked_then_pr 1
assert_eq "the run completes after opening the second ticket's PR" "0" "$RC"
assert_contains "the parked ticket is listed despite the PR" \
  "#480 left the queue without a PR" "$EVENTS"
assert_not_contains "the ticket occupied by the new PR is not listed as parked" \
  "#481 left the queue without a PR" "$EVENTS"

# ── A worker that claimed and produced nothing ───────────────────────────────
echo ""
echo "── failed worker, nothing to save ──"

run_scenario failed 1
assert_eq "the run completes" "0" "$RC"
assert_not_contains "no recovery PR for an unchanged worktree" "pr create" "$TRACE"
assert_contains "the claim label still comes off" \
  "gh issue edit 480 --remove-label in-progress" "$TRACE"
assert_contains "and the console says why" "Worker claimed #480 but produced no PR" "$OUT"
assert_not_contains "a ticket back in the queue is not an attention item" "Needs attention" "$EVENTS"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
