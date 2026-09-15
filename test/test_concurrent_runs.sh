#!/bin/bash
# Two alucard processes on one repository must not share anything they both
# write. Four `continue` runs started a second apart shared everything (#92):
# two minted the same iteration id, one cloned into the other's worktree, a
# finishing run's EXIT trap deleted a worktree another run was still using,
# every run fetched the cached clone twice, and all of them wrote one log dir.
# The contract now: a run's worktrees live under a root only it uses, its log
# dir is its own even in a shared second, the cache is fetched once per
# process, and git writes to the shared clone take turns under a lock.
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

TEST_DIR=$(mktemp -d /tmp/alucard_test_concurrent.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT

# shellcheck disable=SC1090
source "$ALUCARD"

# ── claim_log_dir: a shared second does not mean a shared directory ─────────
echo "── log dir ──"

LOGS="$TEST_DIR/logs"; mkdir -p "$LOGS"
first=$(claim_log_dir "$LOGS" 20260914-101500)
assert_eq "the first run keeps the plain timestamped name" \
  "$LOGS/alucard-20260914-101500" "$first"
[ -d "$first" ] && pass "and the directory exists" || fail "and the directory exists"
second=$(claim_log_dir "$LOGS" 20260914-101500)
assert_eq "a run in the same second gets its pid appended" \
  "$LOGS/alucard-20260914-101500-$$" "$second"
[ -d "$second" ] && pass "and that directory exists too" || fail "and that directory exists too"
[ -d "$first" ] && pass "the first run's directory is untouched" || fail "the first run's directory is untouched"

# The worktree root is claimed under the repository in its own right: two runs
# with different --logs-root values can both hold "alucard-<stamp>" as a log
# dir name, and must still not meet at the same worktree path.
WTS="$TEST_DIR/repo-wts/.alucard-worktrees"
first_wt=$(claim_unique_dir "$WTS/alucard-20260914-101500")
assert_eq "the first run gets the worktree root named after its log dir" \
  "$WTS/alucard-20260914-101500" "$first_wt"
second_wt=$(claim_unique_dir "$WTS/alucard-20260914-101500")
assert_eq "a second run wanting the same name gets its pid appended" \
  "$WTS/alucard-20260914-101500-$$" "$second_wt"
[ -d "$first_wt" ] && [ -d "$second_wt" ] && pass "both roots exist" || fail "both roots exist"

# ── repo_lock_file: where the lock lives ────────────────────────────────────
echo ""
echo "── lock file ──"

DEFAULT_CACHE_DIR="$TEST_DIR/cache"
assert_eq "a cached clone is locked by a sibling file, which exists before the clone does" \
  "$TEST_DIR/cache/example/api.lock" "$(repo_lock_file "$TEST_DIR/cache/example/api")"

LOCAL="$TEST_DIR/local"
git -C "$TEST_DIR" init -q -b main local
git -C "$LOCAL" config user.email test@example.invalid
git -C "$LOCAL" config user.name test
printf 'seed\n' > "$LOCAL/README.md"
git -C "$LOCAL" add .
git -C "$LOCAL" commit -qm initial
assert_eq "a local checkout is locked inside its own git dir" \
  "$LOCAL/.git/alucard.lock" "$(repo_lock_file "$LOCAL")"

# A checkout that is itself a git worktree has a .git *file*; the lock goes
# where that file points, not into a path that cannot hold it.
git -C "$LOCAL" worktree add -q "$TEST_DIR/local-wt" -b wt-branch
assert_eq "a git-worktree checkout is locked in its resolved git dir" \
  "$LOCAL/.git/worktrees/local-wt/alucard.lock" "$(repo_lock_file "$TEST_DIR/local-wt")"

# ── with_repo_lock: the command runs, and it waits its turn ─────────────────
echo ""
echo "── with_repo_lock ──"

LOCK="$TEST_DIR/a.lock"
HAVE_FLOCK=false
with_repo_lock "$LOCK" touch "$TEST_DIR/ran-unlocked"
[ -f "$TEST_DIR/ran-unlocked" ] && pass "without flock the command still runs" \
  || fail "without flock the command still runs"
[ -e "$LOCK" ] && fail "without flock no lock file is created" || pass "without flock no lock file is created"

if command -v flock >/dev/null 2>&1; then
  HAVE_FLOCK=true
  with_repo_lock "$LOCK" touch "$TEST_DIR/ran-locked"
  [ -f "$TEST_DIR/ran-locked" ] && pass "with flock the command runs" || fail "with flock the command runs"
  [ -e "$LOCK" ] && pass "and the lock file is created" || fail "and the lock file is created"

  # A holder takes the lock and keeps it until told to let go. The waiter must
  # not run its command until then — that is the whole point.
  (
    exec 9>"$LOCK"
    flock 9
    touch "$TEST_DIR/held"
    while [ ! -e "$TEST_DIR/release" ]; do sleep 0.05; done
  ) &
  HOLDER=$!
  for _ in $(seq 1 100); do [ -e "$TEST_DIR/held" ] && break; sleep 0.05; done
  [ -e "$TEST_DIR/held" ] || fail "test setup: holder never took the lock"

  with_repo_lock "$LOCK" touch "$TEST_DIR/waited" &
  WAITER=$!
  sleep 0.5
  [ -e "$TEST_DIR/waited" ] && fail "the command does not run while another process holds the lock" \
    || pass "the command does not run while another process holds the lock"
  touch "$TEST_DIR/release"
  wait "$HOLDER" "$WAITER" || true
  [ -e "$TEST_DIR/waited" ] && pass "and runs once the lock is released" \
    || fail "and runs once the lock is released"

  # A lock nobody releases is reported, not waited on forever.
  (
    exec 9>"$TEST_DIR/stuck.lock"
    flock 9
    touch "$TEST_DIR/stuck-held"
    while [ ! -e "$TEST_DIR/stuck-release" ]; do sleep 0.05; done
  ) &
  STUCK=$!
  for _ in $(seq 1 100); do [ -e "$TEST_DIR/stuck-held" ] && break; sleep 0.05; done
  REPO_LOCK_WAIT=1
  set +e
  err=$( { with_repo_lock "$TEST_DIR/stuck.lock" touch "$TEST_DIR/never"; } 2>&1 )
  rc=$?
  set -e
  REPO_LOCK_WAIT=600
  touch "$TEST_DIR/stuck-release"
  wait "$STUCK" || true
  [ "$rc" -ne 0 ] && pass "a lock that is never released fails the command" \
    || fail "a lock that is never released fails the command"
  assert_contains "and says which lock it waited for" "stuck.lock" "$err"
  [ -e "$TEST_DIR/never" ] && fail "the command does not run after the wait times out" \
    || pass "the command does not run after the wait times out"
else
  echo "SKIP: flock not installed — lock serialization not exercised"
fi

# ── remote_branch_sha: the fetch and its read hold the lock together ────────
# The answer lands in FETCH_HEAD, one file per clone, so two runs sharing a
# cached clone could otherwise read each other's branch.
echo ""
echo "── remote_branch_sha ──"

ORIGIN="$TEST_DIR/origin.git"
git init -q --bare "$ORIGIN"
git -C "$LOCAL" remote add origin "$ORIGIN"
git -C "$LOCAL" push -q -u origin main
rm -f "$LOCAL/.git/alucard.lock"
sha=$(remote_branch_sha "$LOCAL" main)
assert_eq "resolves the branch to the remote's SHA" "$(git -C "$LOCAL" rev-parse HEAD)" "$sha"
if [ "$HAVE_FLOCK" = true ]; then
  [ -e "$LOCAL/.git/alucard.lock" ] && pass "and did so under the repo lock" \
    || fail "and did so under the repo lock"
fi
if remote_branch_sha "$LOCAL" no-such-branch >/dev/null 2>&1; then
  fail "a branch the remote lacks resolves to nothing"
else
  pass "a branch the remote lacks resolves to nothing"
fi

# ── End to end: one `continue` on a cached clone another run is using ───────
echo ""
echo "── continue beside another run ──"

MOCK_BIN="$TEST_DIR/bin"; mkdir -p "$MOCK_BIN"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"
REMOTE="$TEST_DIR/remote.git"
SEED="$TEST_DIR/seed"
CACHE="$TEST_DIR/cache"
BRANCH="feat/under-review"

git init -q --bare "$REMOTE"
mkdir -p "$SEED"
git -C "$SEED" init -q -b main
git -C "$SEED" config user.email test@example.invalid
git -C "$SEED" config user.name test
printf '# Target\n' > "$SEED/README.md"
git -C "$SEED" add .
git -C "$SEED" commit -qm initial
git -C "$SEED" remote add origin "$REMOTE"
git -C "$SEED" push -qu origin main
git -C "$SEED" checkout -q -b "$BRANCH"
printf 'change\n' >> "$SEED/README.md"
git -C "$SEED" commit -qam change
git -C "$SEED" push -q origin "$BRANCH"

# The cached clone the way resolve_repo would have left it after a first run,
# with another run's worktree already sitting under it.
mkdir -p "$CACHE/example"
git clone -q "$REMOTE" "$CACHE/example/api"
OTHER_WT="$CACHE/example/api/.alucard-worktrees/alucard-20260914-101459/iter-1"
mkdir -p "$OTHER_WT"
printf 'someone else is using this\n' > "$OTHER_WT/keep"

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) printf '%s\n' "$*" >> "$ALUCARD_TEST_ARGV" ;;
  *) exit 0 ;;
esac
for _a in "$@"; do
  case "$_a" in
    *:/work-output:rw)
      printf 'APPROVED\n' > "${_a%%:*}/.alucard-review"
      printf 'looks fine\n' > "${_a%%:*}/.alucard-review-body" ;;
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

cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "$1 $2" in
  "pr checks") echo "no checks reported" >&2; exit 1 ;;
  "pr list")   printf '77\n' ;;
  "pr view")
    case "$*" in
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

# From a directory that has no example/api of its own, so the shorthand goes
# to the cache rather than resolving as a local path.
CWD="$TEST_DIR/cwd"; mkdir -p "$CWD"
RUN_LOGS="$TEST_DIR/run-logs"
: > "$TEST_DIR/argv"
set +e
(
  cd "$CWD" && \
  PATH="$MOCK_BIN:$PATH" ALUCARD_CACHE_DIR="$CACHE" \
  ALUCARD_TEST_BRANCH="$BRANCH" ALUCARD_TEST_ARGV="$TEST_DIR/argv" \
  "$ALUCARD" continue 77 example/api --no-build --max-review-cycles 1 \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$RUN_LOGS"
) > "$TEST_DIR/out" 2>&1
RC=$?
set -e
OUT=$(<"$TEST_DIR/out")
ARGV=$(<"$TEST_DIR/argv")

assert_eq "the continue completes" "0" "$RC"
assert_eq "the cached clone is refreshed once, not once per resolve" \
  "1" "$(grep -c 'Refreshing cached clone of example/api' <<<"$OUT" || true)"
assert_contains "the iteration id carries the PR number" "ALUCARD_ITER=continue-77-" "$ARGV"

RUN_DIR=$(ls -1d "$RUN_LOGS"/alucard-* 2>/dev/null | head -n1)
RUN_ID=$(basename "${RUN_DIR:-}")
if [[ "$RUN_ID" =~ ^alucard-[0-9]{8}-[0-9]{6}$ ]]; then
  pass "an uncontested run keeps the plain timestamped log dir name"
else
  fail "an uncontested run keeps the plain timestamped log dir name (got '$RUN_ID')"
fi
WT_MOUNT=$(grep -oE -- '-v [^ ]+:/work:ro' <<<"$ARGV" | head -n1 | sed 's/^-v //; s/:\/work:ro$//')
assert_eq "the reviewer's worktree lives under a root named after this run" \
  "$CACHE/example/api/.alucard-worktrees/$RUN_ID" "$(dirname "$WT_MOUNT")"
[ -f "$OTHER_WT/keep" ] && pass "the other run's worktree survives this run's cleanup" \
  || fail "the other run's worktree survives this run's cleanup"
[ -e "$CACHE/example/api/.alucard-worktrees/$RUN_ID" ] \
  && fail "this run's own root is gone when it ends" \
  || pass "this run's own root is gone when it ends"
if command -v flock >/dev/null 2>&1; then
  [ -e "$CACHE/example/api.lock" ] && pass "git writes to the cache went through its lock file" \
    || fail "git writes to the cache went through its lock file"
fi

# ── End to end: the worktree name is already taken under the repository ─────
# Another run, with a different --logs-root, holds this second's name under
# .alucard-worktrees. Pre-take the next few seconds' names so the run under
# test is certain to find its own taken, whichever second it lands in.
echo ""
echo "── continue when another run holds this second's worktree name ──"

TAKEN=()
for off in 0 1 2 3 4 5 6 7; do
  d="$CACHE/example/api/.alucard-worktrees/alucard-$(date -d "+${off} sec" +%Y%m%d-%H%M%S)"
  mkdir -p "$d/iter-1"
  printf 'another run\n' > "$d/iter-1/keep"
  TAKEN+=("$d")
done
: > "$TEST_DIR/argv"
set +e
(
  cd "$CWD" && \
  PATH="$MOCK_BIN:$PATH" ALUCARD_CACHE_DIR="$CACHE" \
  ALUCARD_TEST_BRANCH="$BRANCH" ALUCARD_TEST_ARGV="$TEST_DIR/argv" \
  "$ALUCARD" continue 77 example/api --no-build --max-review-cycles 1 \
    --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/run-logs-2"
) > "$TEST_DIR/out2" 2>&1
RC=$?
set -e
ARGV=$(<"$TEST_DIR/argv")
assert_eq "the continue completes" "0" "$RC"
RUN_ID_2=$(basename "$(ls -1d "$TEST_DIR"/run-logs-2/alucard-* | head -n1)")
WT_MOUNT=$(grep -oE -- '-v [^ ]+:/work:ro' <<<"$ARGV" | head -n1 | sed 's/^-v //; s/:\/work:ro$//')
WT_PARENT=$(basename "$(dirname "$WT_MOUNT")")
if [[ "$WT_PARENT" =~ ^alucard-[0-9]{8}-[0-9]{6}-[0-9]+$ ]]; then
  pass "the worktree root gets a pid suffix instead of the taken name"
else
  fail "the worktree root gets a pid suffix instead of the taken name (got '$WT_PARENT')"
fi
[ "$WT_PARENT" != "$RUN_ID_2" ] && pass "so it differs from the log dir name this time" \
  || fail "so it differs from the log dir name this time"
all_kept=true
for d in "${TAKEN[@]}"; do [ -f "$d/iter-1/keep" ] || all_kept=false; done
[ "$all_kept" = true ] && pass "every other run's worktree is untouched" \
  || fail "every other run's worktree is untouched"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
