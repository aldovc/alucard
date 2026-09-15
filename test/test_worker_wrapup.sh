#!/bin/bash
# A worker that runs out of turns with work in its worktree gets a short
# wrap-up agent before mechanical recovery: same worktree, told the ticket,
# the stop reason, and the worker's own last narration; it commits leftovers
# with honest messages and writes the handoff the recovery PR body carries.
# The harness still does the push, the PR, the labels, and the ticket comment,
# and still commits anything the agent left behind. One recovery PR held two
# hours of backend work under a single "wip: alucard recovery" commit and a
# body that could say nothing about it (#84).
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

# Line order: $2 must appear after $3.
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

TEST_DIR=$(mktemp -d /tmp/alucard_test_wrapup.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
mkdir -p "$MOCK_BIN"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

# ── The pure pieces ──────────────────────────────────────────────────────────
echo "── last words ──"

# shellcheck disable=SC1090
source "$ALUCARD"

CLAUDE_LOG="$TEST_DIR/worker.jsonl"
cat > "$CLAUDE_LOG" <<'EOF'
{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Backend intake done; tests pass."},{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}
{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"partial"}}}
{"type":"assistant","message":{"content":[{"type":"text","text":"Now starting the frontend panel."}]}}
{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":181,"result":""}
EOF
words=$(agent_last_words "$CLAUDE_LOG" claude 4000)
assert_contains "a claude log yields the worker's narration" "Backend intake done; tests pass." "$words"
assert_contains "in order, ending with its last message" "Now starting the frontend panel." "$words"
assert_line_after "the later message comes after the earlier" \
  "Now starting the frontend panel." "Backend intake done" "$words"
assert_not_contains "tool calls and partial deltas are not narration" "partial" "$words"

CODEX_LOG="$TEST_DIR/worker-codex.jsonl"
cat > "$CODEX_LOG" <<'EOF'
{"type":"item.completed","item":{"type":"command_execution","command":"ls"}}
{"type":"item.completed","item":{"type":"agent_message","text":"Wrote the migration; the model is next."}}
EOF
assert_eq "a codex log yields its agent messages" \
  "Wrote the migration; the model is next." "$(agent_last_words "$CODEX_LOG" codex 4000)"

long=$(agent_last_words "$CLAUDE_LOG" claude 20)
assert_eq "a long narration keeps its tail, marked as cut" "… the frontend panel." "$long"
assert_eq "a missing log yields nothing" "" "$(agent_last_words "$TEST_DIR/nope.jsonl" claude 4000)"
printf 'not json\n' > "$TEST_DIR/junk.jsonl"
assert_eq "an unreadable log yields nothing rather than an error" "" "$(agent_last_words "$TEST_DIR/junk.jsonl" claude 4000)"

echo ""
echo "── who gets a wrap-up ──"
wrapup_applies exhausted && pass "an exhausted worker" || fail "an exhausted worker"
wrapup_applies failed && pass "a failed worker" || fail "a failed worker"
wrapup_applies wedged && fail "not a wedged one (its worktree is gone)" || pass "not a wedged one (its worktree is gone)"
wrapup_applies transport && fail "not a transport drop (already retried)" || pass "not a transport drop (already retried)"
wrapup_applies no_worktree && fail "not one that never started" || pass "not one that never started"

echo ""
echo "── the PR body ──"
HANDOFF='## Done
- intake endpoint and queue processing

## Remaining
- frontend panel discards results

## Unverified
- frontend typecheck never ran

## Next step
- run the frontend typecheck'
body=$(recovery_pr_body "Refs #480" exhausted 1 1 github "$HANDOFF")
assert_eq "the Refs line still opens the body" "Refs #480" "$(printf '%s\n' "$body" | head -n1)"
assert_contains "the handoff is set off under its own heading" "## Handoff from the wrap-up agent" "$body"
assert_contains "and carries the agent's text" "frontend panel discards results" "$body"
assert_line_after "it comes after the harness's own summary" \
  "## Handoff from the wrap-up agent" "recovery PR**" "$body"
assert_line_after "and before the next-step list" \
  "This is not a finished change" "## Handoff from the wrap-up agent" "$body"
assert_contains "the mechanical facts stay" "does not resume implementation" "$body"
assert_contains "a GitHub body names the pinned restart" "alucard run --issue <N>" "$body"
body=$(recovery_pr_body "Refs #480" exhausted 1 1 github "")
assert_not_contains "no handoff, no heading" "Handoff from the wrap-up agent" "$body"
body=$(recovery_pr_body "Task: 7" exhausted 1 1 file "")
assert_not_contains "a local body does not offer --issue" "--issue" "$body"

# ── End to end ───────────────────────────────────────────────────────────────
cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

# The first `docker run` is the worker: one commit, one uncommitted file, two
# narration lines, then the max-turns result. The second is the wrap-up, which
# plays ALUCARD_TEST_WRAPUP: `handoff` commits the leftover file itself and
# writes the note; `silent` exits having done neither.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
# One line per launch: the prompt argument has newlines of its own.
printf '%s\n' "$(printf '%s' "$*" | tr '\n' ' ')" >> "$ALUCARD_TEST_STATE/argv"
wt=""; out=""
for _a in "$@"; do
  case "$_a" in
    *:/work:rw) wt="${_a%%:*}" ;;
    *:/work-output:rw) out="${_a%%:*}" ;;
  esac
done
n=$(grep -c '^run ' "$ALUCARD_TEST_STATE/launches" || true)
n=$((n + 1))
printf 'run %s %s\n' "$n" "$wt" >> "$ALUCARD_TEST_STATE/launches"
if [ "$n" -eq 1 ]; then
  touch "$ALUCARD_TEST_STATE/worker-ran"
  printf 'intake done\n' > "$wt/backend.py"
  git -C "$wt" add -A
  git -C "$wt" -c user.name=w -c user.email=w@example.invalid commit -qm 'feat: intake endpoint'
  printf 'panel, untested\n' > "$wt/panel.tsx"
  printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Backend intake done; tests pass."}]}}'
  printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Now starting the frontend panel."}]}}'
  printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":181,"result":""}'
  exit 1
fi
case "$ALUCARD_TEST_WRAPUP" in
  handoff)
    git -C "$wt" add -A
    printf 'wip: panel wiring — untested, worker stopped at its turn cap\n' > "$wt/.git/COMMIT_MSG"
    git -C "$wt" -c user.name=wrapup -c user.email=wrapup@example.invalid commit -qF "$wt/.git/COMMIT_MSG"
    printf '## Done\n- intake endpoint\n\n## Remaining\n- panel discards results\n\n## Unverified\n- frontend typecheck\n\n## Next step\n- run the typecheck\n' > "$out/.alucard-handoff"
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"handoff written"}'
    exit 0 ;;
  silent)
    printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"result":""}'
    exit 1 ;;
esac
MOCK

# GitHub: the queue is #480 (or the pinned issue), claimed after the worker
# ran unless the scenario says the worker never got to the label.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_STATE/trace"
ran=false; [ -f "$ALUCARD_TEST_STATE/worker-ran" ] && ran=true
has_jq=false; for _a in "$@"; do [ "$_a" = "--jq" ] && has_jq=true; done
case "$1 $2" in
  "repo view")    echo "example/api" ;;
  "api graphql")  echo "[]" ;;
  "issue view")
    echo '{"number":480,"title":"Queue receipt ingest","body":"A big one.","labels":[{"name":"ready-for-agent"}],"state":"OPEN"}' ;;
  "issue list")
    case "$*" in
      *in-progress*)
        case "${ALUCARD_TEST_CLAIMS:-yes}" in
          yes)   if $ran; then echo "[480]"; else echo "[]"; fi ;;
          other) if $ran; then echo "[481]"; else echo "[]"; fi ;;
          *)     echo "[]" ;;
        esac ;;
      *ready-for-agent*)
        if $has_jq; then echo "[480]"; else
          echo '[{"number":480,"title":"Queue receipt ingest","body":"A big one.","labels":[{"name":"ready-for-agent"}]}]'
        fi ;;
      *) echo "[]" ;;
    esac ;;
  "pr list")      echo "" ;;
  "pr view")      exit 1 ;;
  "pr create")
    while [ $# -gt 0 ]; do
      case "$1" in
        --body)  printf '%s' "$2" > "$ALUCARD_TEST_STATE/pr-body"; shift ;;
        --title) printf '%s' "$2" > "$ALUCARD_TEST_STATE/pr-title"; shift ;;
      esac
      shift
    done
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

WRAPUP_TURNS=9   # not the default, so a frozen number cannot pass
EXTRA_RUN_ARGS=()
PRE_RUN_HOOK=""

run_scenario() {
  local name="$1"; shift
  local root="$TEST_DIR/$name" target remote
  target="$root/target"; remote="$root/remote.git"
  STATE="$root/state"
  mkdir -p "$target" "$STATE"
  : > "$STATE/argv"; : > "$STATE/launches"; : > "$STATE/trace"
  git init -q --bare "$remote"
  git -C "$target" init -q -b main
  git -C "$target" config user.email test@example.invalid
  git -C "$target" config user.name test
  printf '# Target\n' > "$target/README.md"
  git -C "$target" add .
  git -C "$target" commit -qm initial
  git -C "$target" remote add origin "$remote"
  if [ -n "${PRE_RUN_HOOK:-}" ]; then "$PRE_RUN_HOOK" "$target"; fi
  git -C "$target" push -qu origin main

  # Remaining arguments are env assignments; EXTRA_RUN_ARGS are CLI flags.
  set +e
  env PATH="$MOCK_BIN:$PATH" ALUCARD_TEST_STATE="$STATE" \
    ALUCARD_TRANSPORT_RETRY_ATTEMPTS=0 ALUCARD_WRAPUP_MAX_TURNS="$WRAPUP_TURNS" \
    "$@" \
    "$ALUCARD" run "$target" --iterations 1 --no-build \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$root/logs" \
      ${EXTRA_RUN_ARGS[@]+"${EXTRA_RUN_ARGS[@]}"} > "$root/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$root/out")
  EVENTS=$(cut -f2- "$root"/logs/alucard-*/events.log)
  TRACE=$(<"$STATE/trace")
  ARGV=$(<"$STATE/argv")
  LAUNCHES=$(<"$STATE/launches")
  REMOTE_DIR="$remote"
  PROMPT_FILE=$(ls "$root"/logs/alucard-*/prompts/wrapup-1.txt 2>/dev/null || true)
}

echo ""
echo "── exhausted worker, wrap-up writes the handoff ──"
run_scenario handoff ALUCARD_TEST_WRAPUP=handoff
assert_eq "the run completes" "0" "$RC"
assert_eq "two containers ran: the worker, then the wrap-up" "2" "$(grep -c '^run ' <<<"$LAUNCHES")"
worker_wt=$(sed -n 's/^run 1 //p' <<<"$LAUNCHES"); wrap_wt=$(sed -n 's/^run 2 //p' <<<"$LAUNCHES")
assert_eq "the wrap-up runs in the worker's own worktree" "$worker_wt" "$wrap_wt"
wrap_argv=$(sed -n 2p <<<"$ARGV")
assert_contains "under its own turn cap" "--max-turns $WRAPUP_TURNS" "$wrap_argv"
assert_contains "with an output mount for the handoff" ":/work-output:rw" "$wrap_argv"
assert_contains "and named as the wrap-up role" "alucard-wrapup-1-" "$wrap_argv"
if [ -n "$PROMPT_FILE" ]; then
  pass "the wrap-up prompt is archived"
  PROMPT=$(<"$PROMPT_FILE")
  assert_contains "it names the ticket" "issue #480" "$PROMPT"
  assert_contains "it says how the worker stopped" "ran out of turns or hit its time limit" "$PROMPT"
  assert_contains "it carries the worker's last words" "Now starting the frontend panel." "$PROMPT"
  assert_contains "it carries the earlier narration too" "Backend intake done" "$PROMPT"
  assert_contains "it names the base branch" "<base_branch>main</base_branch>" "$PROMPT"
  assert_contains "it carries the exact base commit the worker started from" \
    "<base_sha>$(git --git-dir="$REMOTE_DIR" rev-parse main)</base_sha>" "$PROMPT"
  assert_contains "and is told to compare against that, not the stale ref" \
    '`git log <base_sha>..HEAD --stat`' "$PROMPT"
  assert_not_contains "no instruction compares against origin/<base_branch>" \
    'origin/<base_branch>..HEAD' "$PROMPT"
  assert_eq "a GitHub run carries no local task block" "0" "$(grep -c '^<task>' <<<"$PROMPT" || true)"
  assert_contains "it says what to write" "/work-output/.alucard-handoff" "$PROMPT"
  assert_contains "and what not to do" "Do not push" "$PROMPT"
  assert_eq "it is told its own cap" "1" "$(grep -c "This run gets ${WRAPUP_TURNS} turns" <<<"$PROMPT")"
else
  fail "the wrap-up prompt is archived"
fi
assert_contains "the wrap-up launch is logged" "launching wrap-up agent (max ${WRAPUP_TURNS} turns)" "$EVENTS"
assert_contains "and its handoff" "Wrap-up: handoff written" "$EVENTS"
assert_line_after "the wrap-up runs before the recovery push" \
  "Recovery: worker rc=1 left" "Wrap-up: handoff written" "$EVENTS"

PR_BODY=$(<"$STATE/pr-body")
assert_eq "the recovery PR body opens with Refs" "Refs #480" "$(printf '%s\n' "$PR_BODY" | head -n1)"
assert_contains "and carries the agent's handoff" "## Handoff from the wrap-up agent" "$PR_BODY"
assert_contains "with its text" "panel discards results" "$PR_BODY"
assert_contains "the mechanical disclaimers stay" "does not resume implementation" "$PR_BODY"
assert_contains "the title still names the stop" "exhausted" "$(<"$STATE/pr-title")"
assert_contains "the PR is labeled needs-human" "issues/91/labels" "$TRACE"
assert_contains "the claim label comes off" "gh issue edit 480 --remove-label in-progress" "$TRACE"
[ -f "$STATE/issue-comment-480" ] && pass "the ticket is told where its work went" \
  || fail "the ticket is told where its work went"

branch=$(git --git-dir="$REMOTE_DIR" for-each-ref --format='%(refname:short)' refs/heads/alucard/ | head -n1)
if [ -n "$branch" ]; then
  pass "the branch was pushed"
  assert_eq "with the worker's commit and the wrap-up's" \
    "2" "$(git --git-dir="$REMOTE_DIR" rev-list --count "main..$branch")"
  msgs=$(git --git-dir="$REMOTE_DIR" log --format=%s "main..$branch")
  assert_contains "the leftover file is under the agent's message" "wip: panel wiring — untested" "$msgs"
  assert_not_contains "not the harness's" "wip: alucard recovery" "$msgs"
  assert_contains "the uncommitted file is in the pushed tree" \
    "panel.tsx" "$(git --git-dir="$REMOTE_DIR" ls-tree --name-only "$branch")"
else
  fail "the branch was pushed"
fi

echo ""
echo "── exhausted worker, wrap-up leaves nothing ──"
run_scenario silent ALUCARD_TEST_WRAPUP=silent
assert_eq "the run completes" "0" "$RC"
assert_eq "the wrap-up still ran" "2" "$(grep -c '^run ' <<<"$LAUNCHES")"
assert_contains "its silence is logged" "Wrap-up: agent left no handoff (rc=1)" "$EVENTS"
PR_BODY=$(<"$STATE/pr-body")
assert_not_contains "the body has no empty handoff section" "Handoff from the wrap-up agent" "$PR_BODY"
assert_eq "it still opens with Refs" "Refs #480" "$(printf '%s\n' "$PR_BODY" | head -n1)"
branch=$(git --git-dir="$REMOTE_DIR" for-each-ref --format='%(refname:short)' refs/heads/alucard/ | head -n1)
msgs=$(git --git-dir="$REMOTE_DIR" log --format=%s "main..$branch")
assert_contains "the harness committed the leftovers itself" "wip: alucard recovery — uncommitted worker state" "$msgs"
assert_eq "two commits either way" "2" "$(git --git-dir="$REMOTE_DIR" rev-list --count "main..$branch")"

echo ""
echo "── wrap-up disabled ──"
run_scenario disabled ALUCARD_TEST_WRAPUP=handoff ALUCARD_WRAPUP_MAX_TURNS=0
assert_eq "the run completes" "0" "$RC"
assert_eq "only the worker ran" "1" "$(grep -c '^run ' <<<"$LAUNCHES")"
assert_not_contains "no wrap-up is logged" "Wrap-up:" "$EVENTS"
assert_contains "recovery is mechanical" "wip: alucard recovery — uncommitted worker state" \
  "$(git --git-dir="$REMOTE_DIR" log --format=%s "main..$(git --git-dir="$REMOTE_DIR" for-each-ref --format='%(refname:short)' refs/heads/alucard/ | head -n1)")"

echo ""
echo "── pinned run, worker never labelled the ticket ──"
EXTRA_RUN_ARGS=(--issue 480)
run_scenario pinned ALUCARD_TEST_WRAPUP=handoff ALUCARD_TEST_CLAIMS=no
assert_eq "the run completes" "0" "$RC"
PR_BODY=$(<"$STATE/pr-body")
assert_eq "the recovery PR is attributed to the pinned ticket" "Refs #480" "$(printf '%s\n' "$PR_BODY" | head -n1)"
assert_contains "the wrap-up was told the ticket" "issue #480" "$(<"$PROMPT_FILE")"
[ -f "$STATE/issue-comment-480" ] && pass "the pinned ticket is told where its work went" \
  || fail "the pinned ticket is told where its work went"
assert_contains "the run's attention list names the ticket" "recovery PR #91 holds issue #480" "$EVENTS"
assert_contains "the console usage table labels the wrap-up as its own role" "wrap-up 1" "$OUT"

# A worker in another process claimed #481 while this pinned run was going.
# The repository-wide label diff sees that claim; it is not this run's ticket.
echo ""
echo "── pinned run beside another worker's claim ──"
run_scenario pinned_other ALUCARD_TEST_WRAPUP=handoff ALUCARD_TEST_CLAIMS=other
assert_eq "the run completes" "0" "$RC"
PR_BODY=$(<"$STATE/pr-body")
assert_eq "the recovery PR is attributed to the pin, not the other worker's claim" \
  "Refs #480" "$(printf '%s\n' "$PR_BODY" | head -n1)"
assert_contains "the wrap-up was told the pinned ticket" "issue #480" "$(<"$PROMPT_FILE")"
assert_not_contains "and not the other one" "#481" "$(<"$PROMPT_FILE")"
assert_not_contains "the other worker's claim is left alone" "issue edit 481" "$TRACE"
[ -f "$STATE/issue-comment-481" ] && fail "and its ticket gets no comment from this run" \
  || pass "and its ticket gets no comment from this run"
[ -f "$STATE/issue-comment-480" ] && pass "the pinned ticket is told where its work went" \
  || fail "the pinned ticket is told where its work went"
EXTRA_RUN_ARGS=()

# ── Local task source: the criteria travel with the wrap-up ─────────────────
# The tasks file is the worker's whole specification. The container may not
# have it (an external --tasks path, or an untracked file), so the wrap-up is
# handed the task and parent context the way the reviewer is.
echo ""
echo "── local task, wrap-up gets the criteria ──"
prepare_local_tasks() {
  mkdir -p "$1/.alucard"
  cat > "$1/.alucard/tasks.md" <<'TASKS'
# Upload plan

Constraint that travels with every task: keep uploads under 10 MB.

## [ ] 1: Build the multi-file upload panel

Acceptance: drop zone accepts several files; skipped files are reported.
TASKS
  git -C "$1" add .alucard/tasks.md
  git -C "$1" commit -qm 'plan'
}
PRE_RUN_HOOK=prepare_local_tasks
run_scenario local ALUCARD_TEST_WRAPUP=handoff ALUCARD_TEST_CLAIMS=no
PRE_RUN_HOOK=""
assert_eq "the run completes" "0" "$RC"
PROMPT=$(<"$PROMPT_FILE")
assert_contains "the wrap-up is told which task" "task 1" "$PROMPT"
assert_eq "it gets the task block" "1" "$(grep -c '^<task>' <<<"$PROMPT" || true)"
assert_contains "with the acceptance criteria" "skipped files are reported" "$PROMPT"
assert_contains "and the parent context" "keep uploads under 10 MB" "$PROMPT"
PR_BODY=$(<"$STATE/pr-body")
assert_eq "the recovery PR opens with the task line" "Task: 1" "$(printf '%s\n' "$PR_BODY" | head -n1)"
assert_contains "and carries the handoff" "## Handoff from the wrap-up agent" "$PR_BODY"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
