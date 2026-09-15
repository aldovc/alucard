#!/bin/bash
# The container side of #92. A reviewer whose /work had been deleted by another
# run reviewed an empty directory, ran `gh pr view` with no remote to resolve
# the repo from, and posted the failure as a high-severity human block. Two
# guards: the entrypoint refuses to start an agent in an empty checkout, with
# an exit code the harness maps to a fresh worktree and, failing that, to a
# harness fault; and every container gets GH_REPO so gh never needs the
# checkout's remote.
set -euo pipefail

# The mocks stand in for tools that read stdin; see test_timeout_recovery.sh.
exec </dev/null

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"
ENTRYPOINT="$SCRIPT_DIR/../entrypoint.sh"

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

TEST_DIR=$(mktemp -d /tmp/alucard_test_container_ctx.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

# shellcheck disable=SC1090
source "$ALUCARD"

# ── The entrypoint refuses an empty checkout ────────────────────────────────
echo "── entrypoint: empty checkout ──"

EMPTY="$TEST_DIR/empty"; mkdir -p "$EMPTY"
set +e
err=$(cd "$EMPTY" && bash "$ENTRYPOINT" touch "$TEST_DIR/agent-ran" 2>&1)
rc=$?
set -e
assert_eq "an empty checkout stops the container with the harness's no-worktree code" \
  "$NO_WORKTREE_RC" "$rc"
assert_contains "and says the worktree is missing" "worktree is missing" "$err"
[ -e "$TEST_DIR/agent-ran" ] && fail "the agent command never runs" || pass "the agent command never runs"

# ── The entrypoint starts the agent in a real checkout ──────────────────────
# Run from a subdirectory, as the preflight container is (-w /work/<dir>).
# Every path the entrypoint writes is redirected, so this leaves nothing in
# /tmp or the caller's home.
echo ""
echo "── entrypoint: real checkout ──"

CHECKOUT="$TEST_DIR/checkout"
mkdir -p "$CHECKOUT/sub"
git -C "$CHECKOUT" init -q -b main
git -C "$CHECKOUT" config user.email test@example.invalid
git -C "$CHECKOUT" config user.name test
printf 'seed\n' > "$CHECKOUT/README.md"
git -C "$CHECKOUT" add .
git -C "$CHECKOUT" commit -qm initial
CFG="$TEST_DIR/cfg"
set +e
(
  cd "$CHECKOUT/sub" && \
  HOME="$TEST_DIR/home" GIT_CONFIG_GLOBAL="$CFG/gitconfig" GH_CONFIG_DIR="$CFG/gh" \
  GITHUB_TOKEN=test-token bash "$ENTRYPOINT" touch "$TEST_DIR/agent-ran-ok"
) > "$TEST_DIR/entry.out" 2>&1
rc=$?
set -e
assert_eq "a checkout lets the agent start" "0" "$rc"
[ -e "$TEST_DIR/agent-ran-ok" ] && pass "and the agent command runs" \
  || fail "and the agent command runs ($(<"$TEST_DIR/entry.out"))"
if [ -f "$CFG/git-credentials" ]; then
  pass "the credential store sits beside the git config, not at a fixed /tmp path"
  assert_contains "and carries the token" "x-access-token:test-token@github.com" "$(<"$CFG/git-credentials")"
  assert_contains "and git is pointed at it" "$CFG/git-credentials" "$(<"$CFG/gitconfig")"
else
  fail "the credential store sits beside the git config, not at a fixed /tmp path"
fi

# ── Classification and the texts ────────────────────────────────────────────
echo ""
echo "── classification ──"

assert_eq "the entrypoint's code classifies as no_worktree for claude" \
  "no_worktree" "$(classify_agent_failure "$TEST_DIR/no-such-log" claude "$NO_WORKTREE_RC")"
assert_eq "and for codex" \
  "no_worktree" "$(classify_agent_failure "$TEST_DIR/no-such-log" codex "$NO_WORKTREE_RC")"
# The code is checked before any log parse: a stray log with a real result in
# it must not turn a never-started agent into an exhausted or clean one.
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"stale"}' > "$TEST_DIR/stale.jsonl"
assert_eq "the code wins over whatever a stale log says" \
  "no_worktree" "$(classify_agent_failure "$TEST_DIR/stale.jsonl" claude "$NO_WORKTREE_RC")"
retry_with_fresh_worktree no_worktree && pass "no_worktree earns a fresh worktree" || fail "no_worktree earns a fresh worktree"
retry_with_fresh_worktree transport && pass "transport still does" || fail "transport still does"
retry_with_fresh_worktree exhausted && fail "exhausted does not" || pass "exhausted does not"
retry_with_fresh_worktree wedged && fail "a mid-run wedge does not" || pass "a mid-run wedge does not"
assert_contains "the worker stop is described as a never-started checkout" \
  "checkout was empty" "$(describe_worker_stop no_worktree)"
assert_contains "the no-verdict reason calls it a harness fault" "harness fault" "$(no_verdict_reason no_worktree 78)"
assert_contains "and says nothing was reviewed" "Nothing was reviewed" "$(no_verdict_reason no_worktree 78)"

# ── github_repo_slug ────────────────────────────────────────────────────────
echo ""
echo "── github_repo_slug ──"

SLUG="$TEST_DIR/slug"
git -C "$TEST_DIR" init -q -b main slug
assert_eq "no origin gives no slug" "" "$(github_repo_slug "$SLUG")"
git -C "$SLUG" remote add origin git@github.com:example/api.git
assert_eq "an SSH origin" "example/api" "$(github_repo_slug "$SLUG")"
git -C "$SLUG" remote set-url origin https://github.com/example/web
assert_eq "an HTTPS origin" "example/web" "$(github_repo_slug "$SLUG")"
git -C "$SLUG" remote set-url origin https://github.com/example/web.git
assert_eq "an HTTPS origin with .git" "example/web" "$(github_repo_slug "$SLUG")"
git -C "$SLUG" remote set-url origin "$TEST_DIR/remote.git"
assert_eq "a local path origin gives no slug" "" "$(github_repo_slug "$SLUG")"
git -C "$SLUG" remote set-url origin ../remote.git
assert_eq "a relative path that looks like owner/repo gives no slug" "" "$(github_repo_slug "$SLUG")"

# ── invoke_agent hands GH_REPO to the container ─────────────────────────────
echo ""
echo "── GH_REPO in the container ──"

MOCK_BIN="$TEST_DIR/bin"; mkdir -p "$MOCK_BIN"
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
printf '%s\n' "$*" >> "$ALUCARD_TEST_ARGV"
wt=""
for _a in "$@"; do
  case "$_a" in *:/work:r?) wt="${_a%%:*}" ;; esac
done
printf 'launch %s\n' "$wt" >> "$ALUCARD_TEST_TRACE"
_n=$(grep -c '^launch ' "$ALUCARD_TEST_TRACE")
_line=$(sed -n "${_n}p" "${ALUCARD_TEST_SCRIPT:-/dev/null}" 2>/dev/null || true)
[ -n "$_line" ] || _line='0 {"type":"result","subtype":"success","is_error":false,"result":"done"}'
_rc="${_line%% *}"
_json="${_line#* }"
# The entrypoint's refusal prints nothing the harness parses.
[ "$_rc" = "78" ] || printf '%s\n' "$_json"
exit "$_rc"
MOCK
cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout"

export PATH="$MOCK_BIN:$PATH"
export ALUCARD_TEST_ARGV="$TEST_DIR/argv" ALUCARD_TEST_TRACE="$TEST_DIR/trace"
: > "$ALUCARD_TEST_ARGV"; : > "$ALUCARD_TEST_TRACE"
TIMEOUT_MIN=7
ENV_FILE="$TEST_DIR/alucard.env"
IMAGE="alucard-test-image"
stream_text='.'
codex_stream_text='.'
CREATED_CONTAINERS=()

GH_REPO_SLUG="example/api"
invoke_agent iter-1 "$TEST_DIR/agent.jsonl" 10 1 'prompt' -v "$CHECKOUT:/work:rw" >/dev/null 2>&1 || true
ARGV=$(<"$ALUCARD_TEST_ARGV")
assert_contains "a github.com origin becomes GH_REPO in the container" "-e GH_REPO=example/api" "$ARGV"
if grep -qE -- '-e GH_REPO=example/api .* alucard-test-image' <<<"$ARGV"; then
  pass "set before the image, where docker reads env flags"
else
  fail "set before the image, where docker reads env flags"
fi

: > "$ALUCARD_TEST_ARGV"; : > "$ALUCARD_TEST_TRACE"
GH_REPO_SLUG=""
invoke_agent iter-1 "$TEST_DIR/agent2.jsonl" 10 1 'prompt' -v "$CHECKOUT:/work:rw" >/dev/null 2>&1 || true
assert_not_contains "a non-GitHub origin sets no GH_REPO" "GH_REPO=" "$(<"$ALUCARD_TEST_ARGV")"

# ── End to end: a worker whose checkout was empty gets a fresh one ──────────
echo ""
echo "── worker: empty checkout, then a fresh worktree ──"

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'gh'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_GH_TRACE"
has_jq=false; for _a in "$@"; do [ "$_a" = "--jq" ] && has_jq=true; done
case "$1 $2" in
  "repo view")   echo "example/api" ;;
  "api graphql") echo "[]" ;;
  "issue list")
    case "$*" in
      *ready-for-agent*)
        if $has_jq; then echo "[480]"; else
          echo '[{"number":480,"title":"A ticket","body":"Do the thing.","labels":[{"name":"ready-for-agent"}]}]'
        fi ;;
      *) echo "[]" ;;
    esac ;;
  "pr checks")   echo "no checks reported" >&2; exit 1 ;;
  "pr list")     printf '%s\n' "${ALUCARD_TEST_PR:-}" ;;
  "pr view")
    case "$*" in
      *state,headRefName*) printf '{"state":"OPEN","headRefName":"%s"}\n' "${ALUCARD_TEST_BRANCH:-}" ;;
      *comments*)          printf '{"comments":[]}\n' ;;
      *reviews*)           printf '\n' ;;
      *headRefOid*)        printf 'deadbeef\n' ;;
      *)                   printf '\n' ;;
    esac ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/gh"
export ALUCARD_TEST_GH_TRACE="$TEST_DIR/gh-trace"

TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
BRANCH="feat/under-review"
git init -q --bare "$REMOTE"
mkdir -p "$TARGET"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Target\n' > "$TARGET/README.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm initial
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main
git -C "$TARGET" checkout -q -b "$BRANCH"
printf 'change\n' >> "$TARGET/README.md"
git -C "$TARGET" commit -qam change
git -C "$TARGET" push -q origin "$BRANCH"
git -C "$TARGET" checkout -q main

CLEAN='{"type":"result","subtype":"success","is_error":false,"result":"nothing to do"}'

run_worker() {
  : > "$ALUCARD_TEST_ARGV"; : > "$ALUCARD_TEST_TRACE"; : > "$ALUCARD_TEST_GH_TRACE"
  rm -rf "$TEST_DIR/logs"
  set +e
  ALUCARD_TEST_SCRIPT="$TEST_DIR/script" ALUCARD_TRANSPORT_RETRY_ATTEMPTS=1 \
    "$ALUCARD" run "$TARGET" --iterations 1 --no-build \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
  RC=$?
  set -e
  OUT=$(<"$TEST_DIR/out")
  EVENTS=$(cut -f2- "$TEST_DIR"/logs/alucard-*/events.log)
  LAUNCHES=$(grep '^launch ' "$ALUCARD_TEST_TRACE" | sed 's/^launch //')
}

{ printf '78 {}\n'; printf '0 %s\n' "$CLEAN"; } > "$TEST_DIR/script"
run_worker
assert_eq "the run completes" "0" "$RC"
assert_eq "an empty checkout buys the worker a second attempt" "2" "$(wc -l <<<"$LAUNCHES" | tr -d ' ')"
assert_contains "the first attempt is classed no_worktree" "attempt 1/2: rc=78 class=no_worktree" "$EVENTS"
assert_contains "and the retry is logged as such" \
  "no_worktree failure on attempt 1 — retrying with a fresh worktree" "$EVENTS"
first=$(sed -n 1p <<<"$LAUNCHES"); second=$(sed -n 2p <<<"$LAUNCHES")
[ "$first" != "$second" ] && pass "the second attempt mounts a different worktree" \
  || fail "the second attempt mounts a different worktree (both '$first')"
assert_eq "the fresh worktree is the attempt-suffixed one" "iter-1-2" "$(basename "$second")"
assert_contains "the second attempt runs clean" "attempt 2/2: rc=0 class=clean" "$EVENTS"
assert_not_contains "nothing is opened for a worker that never started" "pr create" "$(<"$ALUCARD_TEST_GH_TRACE")"

echo ""
echo "── worker: empty checkout on every attempt ──"
{ printf '78 {}\n'; printf '78 {}\n'; } > "$TEST_DIR/script"
run_worker
assert_eq "the run still completes" "0" "$RC"
assert_eq "attempts stop at the budget" "2" "$(wc -l <<<"$LAUNCHES" | tr -d ' ')"
assert_eq "only one retry is logged" "1" "$(grep -c 'retrying with a fresh worktree' <<<"$EVENTS" || true)"
assert_contains "the final class is no_worktree" "attempt 2/2: rc=78 class=no_worktree" "$EVENTS"
assert_not_contains "no recovery PR for an agent that never ran" "pr create" "$(<"$ALUCARD_TEST_GH_TRACE")"

# ── End to end: a reviewer whose checkout was empty ─────────────────────────
echo ""
echo "── reviewer: empty checkout on every attempt ──"

: > "$ALUCARD_TEST_ARGV"; : > "$ALUCARD_TEST_TRACE"; : > "$ALUCARD_TEST_GH_TRACE"
rm -rf "$TEST_DIR/logs"
{ printf '78 {}\n'; printf '78 {}\n'; } > "$TEST_DIR/script"
set +e
ALUCARD_TEST_SCRIPT="$TEST_DIR/script" ALUCARD_TRANSPORT_RETRY_ATTEMPTS=1 \
ALUCARD_TEST_PR=77 ALUCARD_TEST_BRANCH="$BRANCH" \
  "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 1 \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" > "$TEST_DIR/out" 2>&1
RC=$?
set -e
EVENTS=$(cut -f2- "$TEST_DIR"/logs/alucard-*/events.log)
GH=$(<"$ALUCARD_TEST_GH_TRACE")
assert_eq "the continue completes" "0" "$RC"
assert_eq "an empty checkout buys the reviewer a second attempt" \
  "2" "$(grep -c '^launch ' "$ALUCARD_TEST_TRACE" || true)"
assert_contains "the retry is logged" "no_worktree failure on attempt 1 — retrying reviewer" "$EVENTS"
assert_contains "the PR is flagged for a human as unreviewed" "flagging for human" "$EVENTS"
assert_contains "with the needs-human label" "issues/77/labels" "$GH"
assert_contains "the comment calls it a harness fault" "harness fault" "$GH"
assert_contains "and says the PR was never reviewed" "has not been reviewed" "$GH"
assert_not_contains "it is not posted as a BLOCKED verdict" "BLOCKED" "$GH"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
