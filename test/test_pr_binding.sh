#!/bin/bash
# Two runs on one repository must not step on each other's branch or PR.
# The branch a worker pushes carries the run id (and the ticket or task when
# the harness knows it), so two runs starting an iteration in the same second
# no longer mint the same alucard/iter-N-<epoch>. And the gates take the PR
# the harness found: a worker that renames its branch is still gated on its
# own PR, and an alucard PR another run opened in the same window is never
# adopted, because its head is not a commit in this worker's worktree.
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

TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
mkdir -p "$MOCK_BIN"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

# ── The branch name ──────────────────────────────────────────────────────────
echo "── mint_branch ──"

# shellcheck disable=SC1090
source "$ALUCARD"

WT_ROOT="/repo/.alucard-worktrees/alucard-20260915-101459"
PIN_ISSUE=""
DISPATCHED_TASK_ID=""
assert_eq "queue mode: the run id and the iteration" \
  "alucard/20260915-101459/iter-1" "$(mint_branch 1)"
assert_eq "a first attempt gets no attempt suffix" \
  "alucard/20260915-101459/iter-3" "$(mint_branch 3 1)"
assert_eq "a retry appends its attempt" \
  "alucard/20260915-101459/iter-1-2" "$(mint_branch 1 2)"

WT_ROOT="/repo/.alucard-worktrees/alucard-20260915-101459-4242"
assert_eq "a run that found its second taken carries the pid suffix, so the two branches differ" \
  "alucard/20260915-101459-4242/iter-1" "$(mint_branch 1)"

PIN_ISSUE=84
assert_eq "a pinned issue is in the name from the start" \
  "alucard/20260915-101459-4242/iter-1-issue-84" "$(mint_branch 1)"

PIN_ISSUE=""
DISPATCHED_TASK_ID="7"
assert_eq "a local task is in the name from the start" \
  "alucard/20260915-101459-4242/iter-1-task-7" "$(mint_branch 1)"
DISPATCHED_TASK_ID="a b/c"
assert_eq "task ids are made safe for a ref name" \
  "alucard/20260915-101459-4242/iter-1-task-a-b-c" "$(mint_branch 1)"
DISPATCHED_TASK_ID=""

# ── End to end ───────────────────────────────────────────────────────────────

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

# The worker, and later the reviewer (told apart by the /work-output mount).
#   foreign — changes nothing and opens nothing; another run's PR appears
#             in the window and must not be adopted
#   renamed — commits, renames its branch, pushes; its PR is found by head
#             SHA although no PR exists on the harness branch
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
wt=""
for _a in "$@"; do
  case "$_a" in
    *:/work-output:rw)
      printf 'APPROVED\n' > "${_a%%:*}/.alucard-review"
      printf 'looks fine\n' > "${_a%%:*}/.alucard-review-body"
      printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"reviewed"}'
      exit 0 ;;
    *:/work:rw) wt="${_a%%:*}" ;;
  esac
done
touch "$ALUCARD_TEST_STATE/worker-ran"
git -C "$wt" rev-parse --abbrev-ref HEAD > "$ALUCARD_TEST_STATE/harness-branch"
case "$ALUCARD_TEST_SCENARIO" in
  foreign)
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"nothing to do"}' ;;
  renamed)
    printf 'fixed\n' > "$wt/fix.txt"
    git -C "$wt" add -A
    git -C "$wt" -c user.name=w -c user.email=w@example.invalid commit -qm 'fix: small fix'
    git -C "$wt" branch -m feature/renamed
    git -C "$wt" push -qu origin HEAD
    git -C "$wt" rev-parse HEAD > "$ALUCARD_TEST_STATE/worker-head"
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"opened PR #56"}' ;;
esac
MOCK

# GitHub. No PR ever exists on the harness branch. The alucard-labeled list
# carries another run's PR #55 (newest) whose head is a commit this worktree
# has never seen, and — once this worker has pushed — its own PR #56 on the
# renamed branch.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_STATE/trace"
has_jq=false; for _a in "$@"; do [ "$_a" = "--jq" ] && has_jq=true; done
case "$1 $2" in
  "repo view")    echo "example/api" ;;
  "api graphql")  echo "[]" ;;
  "issue list")
    case "$*" in
      *in-progress*) echo "[]" ;;
      *ready-for-agent*)
        if $has_jq; then echo "[481]"; else
          echo '[{"number":481,"title":"Small fix","body":"A small one.","labels":[{"name":"ready-for-agent"}]}]'
        fi ;;
      *) echo "[]" ;;
    esac ;;
  "pr list")
    case "$*" in
      *--head*) printf '' ;;
      *--label*)
        printf '{"number":55,"createdAt":"2999-01-01T00:00:02Z","headRefName":"alucard/20260915-000000/iter-1","headRefOid":"1111111111111111111111111111111111111111"}\n'
        if [ -f "$ALUCARD_TEST_STATE/worker-head" ]; then
          printf '{"number":56,"createdAt":"2999-01-01T00:00:01Z","headRefName":"feature/renamed","headRefOid":"%s"}\n' \
            "$(<"$ALUCARD_TEST_STATE/worker-head")"
        fi ;;
    esac ;;
  "pr view")
    case "$*" in
      *number,title,body*) printf '{"number":56,"title":"fix: small fix","body":"Closes #481"}\n' ;;
      *headRefOid*)        cat "$ALUCARD_TEST_STATE/worker-head" 2>/dev/null || printf '\n' ;;
      *comments*)          printf '{"comments":[]}\n' ;;
      *reviews*)           printf '\n' ;;
      *body*)              printf 'Closes #481\n' ;;
      *)                   printf '{}\n' ;;
    esac ;;
  "pr checks") echo "no checks reported" >&2; exit 1 ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/timeout" "$MOCK_BIN/docker" "$MOCK_BIN/gh"

run_scenario() {
  local name="$1"
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
  "$ALUCARD" run "$target" --iterations 1 --no-build --max-review-cycles 1 \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$root/logs" > "$root/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$root/out")
  EVENTS=$(cut -f2- "$root"/logs/alucard-*/events.log)
  TRACE=$(<"$STATE/trace")
  RUN_ID=$(basename "$(ls -1d "$root"/logs/alucard-* | head -n1)")
}

echo ""
echo "── another run's PR in the window ──"
run_scenario foreign
assert_eq "the run completes" "0" "$RC"
assert_contains "the harness saw the other run's PR and said whose it is not" \
  "PR #55 on alucard/20260915-000000/iter-1 opened during this iteration, but its head is not this worker's" "$EVENTS"
assert_not_contains "it did not adopt it" "PR found" "$EVENTS"
assert_not_contains "so no gate ran on it" "=== CI gate" "$OUT"
assert_not_contains "no PR comment went to it" "pr comment 55" "$TRACE"

echo ""
echo "── the worker renamed its branch ──"
run_scenario renamed
assert_eq "the run completes" "0" "$RC"
assert_eq "the branch the worker started on is named after this run" \
  "alucard/${RUN_ID#alucard-}/iter-1" "$(<"$STATE/harness-branch")"
assert_contains "the other run's PR is still passed over" \
  "PR #55 on alucard/20260915-000000/iter-1 opened during this iteration, but its head is not this worker's" "$EVENTS"
assert_contains "the worker's own PR is found by its head commit" \
  "PR found (non-harness branch): #56 on feature/renamed" "$EVENTS"
assert_contains "the CI gate runs on that PR" "=== CI gate: PR #56 on feature/renamed" "$OUT"
assert_contains "and so does the review gate" "=== Review gate: PR #56 on feature/renamed" "$OUT"
assert_contains "the review reaches a verdict on it" "Review gate: PR #56 approved" "$EVENTS"
assert_not_contains "no gate looked the PR up by its branch name" \
  "pr list --head feature/renamed" "$TRACE"
assert_contains "the token-usage table is posted to that PR" "pr comment 56" "$TRACE"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
