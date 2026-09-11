#!/bin/bash
# Covers the shared Playwright browser cache (#85).
#
# The bug was silent and only cost time: the image bakes one browser revision,
# a target repo pinning a different Playwright version cannot see it, and a
# fresh container per iteration means the resulting download is paid again
# every iteration. Three things have to hold. The repo's own pinned Playwright
# has to be what populates the cache, because only it knows which revision it
# wants. The cache has to reach the agent containers. And when any of it fails,
# nothing may be mounted — an unseeded cache at PLAYWRIGHT_BROWSERS_PATH would
# hide the browser the image already carries, which is worse than the bug.
set -euo pipefail

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

# Herestring, not a pipe: `printf | grep -q` under pipefail returns 141 when
# grep exits before printf has finished writing.
assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    pass "$label"
  else
    fail "$label (missing '$needle')"
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    fail "$label (unexpected '$needle')"
  else
    pass "$label"
  fi
}

TEST_DIR=$(mktemp -d /tmp/alucard_test_playwright_cache.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT

MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
mkdir -p "$MOCK_BIN"
touch "$TRACE"

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then
  shift 3
  # The harness's own timeout is minutes long. Tests that need it to actually
  # fire substitute a short one; the rest skip the wait entirely.
  [ -n "${ALUCARD_TEST_OUTER_TIMEOUT:-}" ] || exec "$@"
  exec /usr/bin/timeout "$ALUCARD_TEST_OUTER_TIMEOUT" "$@"
fi
exec /usr/bin/timeout "$@"
MOCK

# Traces every invocation, and for `run` also executes the script it was handed
# — in ALUCARD_TEST_EXEC_DIR, with the -e variables it was passed and with the
# browser-cache bind mount resolved back to its host directory. That is how the
# assertions below exercise the real composed preflight command rather than a
# copy of it kept in step by hand.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
{
  printf 'docker'
  printf ' %q' "$@"
  printf '\n'
} >> "$ALUCARD_TEST_TRACE"

if [ "$1" = "image" ]; then
  printf '%s\n' "${ALUCARD_TEST_IMAGE_ID:-sha256:aaaa}"
  exit 0
fi
[ "$1" = "run" ] || exit 0

script="${!#}"
mount_host=""
mount_target=""
env_vars=()
while [ $# -gt 0 ]; do
  case "$1" in
    -e) env_vars+=("${2:-}"); shift ;;
    -v) if [ "${2:-}" != "${2#*:/opt/alucard-browsers}" ]; then
          mount_host="${2%%:*}"
          mount_target="/opt/alucard-browsers"
        fi
        shift ;;
  esac
  shift
done

# Stand in for the bind mount: a container path the harness chose only exists
# on this host under the directory it mounted there.
for v in ${env_vars+"${env_vars[@]}"}; do
  if [ -n "$mount_target" ]; then
    v="${v//$mount_target/$mount_host}"
  fi
  export "${v?}"
done

cd "$ALUCARD_TEST_EXEC_DIR"
bash -c "$script"
MOCK

cat > "$MOCK_BIN/git" <<'MOCK'
#!/bin/bash
set -euo pipefail
[ "$1" = "clone" ] && mkdir -p "${!#}"
exit 0
MOCK

chmod +x "$MOCK_BIN/timeout" "$MOCK_BIN/docker" "$MOCK_BIN/git"
PATH="$MOCK_BIN:$PATH"
export PATH ALUCARD_TEST_TRACE="$TRACE"

# shellcheck disable=SC1090
source "$ALUCARD"

IMAGE="alucard:test"
LOG_DIR="$TEST_DIR/logs"; mkdir -p "$LOG_DIR"
TIMEOUT_MIN=7
ENV_FILE="$TEST_DIR/env"; touch "$ENV_FILE"
BASE_BRANCH=main
CREATED_WORKTREES=()
CREATED_CONTAINERS=()

# The target repo: an npm manifest, so preflight runs, plus a node_modules
# holding a stand-in for the repo's own pinned Playwright.
REPO_ABS="$TEST_DIR/repo"
mkdir -p "$REPO_ABS"
touch "$REPO_ABS/package-lock.json"

WORK="$TEST_DIR/work"
mkdir -p "$WORK/node_modules/.bin"
cat > "$WORK/node_modules/.bin/playwright" <<'PW'
#!/bin/bash
echo "playwright $* -> ${PLAYWRIGHT_BROWSERS_PATH:-unset}" >> "$ALUCARD_TEST_PW_LOG"
exit "${ALUCARD_TEST_PW_EXIT:-0}"
PW
chmod +x "$WORK/node_modules/.bin/playwright"
cat > "$MOCK_BIN/npm" <<'MOCK'
#!/bin/bash
exit "${ALUCARD_TEST_NPM_EXIT:-0}"
MOCK
chmod +x "$MOCK_BIN/npm"

PW_LOG="$TEST_DIR/pw.log"
export ALUCARD_TEST_PW_LOG="$PW_LOG"
export ALUCARD_TEST_EXEC_DIR="$WORK"

# Stands in for the browsers the image bakes at /opt/ms-playwright.
BAKED="$TEST_DIR/baked"
mkdir -p "$BAKED/chromium-1243/chrome-linux"
echo "baked-chrome" > "$BAKED/chromium-1243/chrome-linux/chrome"
BAKED_BROWSERS_PATH="$BAKED"

# ── Seeding the cache from the image ─────────────────────────────────────────
echo "── seeding ──"

CACHE="$TEST_DIR/browsers"
DEFAULT_BROWSERS_DIR="$CACHE"
: > "$TRACE"; : > "$PW_LOG"
toolchain_preflight >/dev/null

assert_eq "the seeded cache is the one containers mount" "$CACHE" "$BROWSERS_CACHE_HOST"
assert_eq "the image's baked browser is handed to the cache" \
  "baked-chrome" "$(cat "$CACHE/chromium-1243/chrome-linux/chrome" 2>/dev/null || true)"
if [ -f "$CACHE/.alucard-seeded" ]; then
  pass "a completed seed leaves its marker"
else
  fail "a completed seed leaves its marker"
fi

# Preflight is the only container the harness starts for this. Seeding from a
# container of its own would show up as an extra agent run everywhere that
# counts them.
assert_eq "preflight still starts exactly one container" \
  "1" "$(grep -c 'docker run' "$TRACE" || true)"

# Re-copying the whole browser directory on every run would pay for nothing.
mkdir -p "$CACHE/chromium-1243/chrome-linux"
echo "local-edit" > "$CACHE/chromium-1243/chrome-linux/chrome"
toolchain_preflight >/dev/null
assert_eq "an already-seeded cache is left alone" \
  "local-edit" "$(cat "$CACHE/chromium-1243/chrome-linux/chrome")"

# A rebuilt image can bake a newer browser. The marker records which image the
# cache was seeded from, so the copy is redone once and only once per image —
# otherwise a repo carrying no Playwright of its own would keep using whatever
# the first image ever seeded.
mkdir -p "$CACHE/chromium-1243/chrome-linux"
echo "stale" > "$CACHE/chromium-1243/chrome-linux/chrome"
ALUCARD_TEST_IMAGE_ID="sha256:bbbb" toolchain_preflight >/dev/null
assert_eq "a rebuilt image re-seeds the cache" \
  "baked-chrome" "$(cat "$CACHE/chromium-1243/chrome-linux/chrome")"

# The seed runs before the dependency install, not after, so a repo whose
# dependencies will not install still leaves a cache every later container can
# use — and does not silently cost them the baked browser as well.
DEFAULT_BROWSERS_DIR="$TEST_DIR/browsers-broken-deps"
ALUCARD_TEST_NPM_EXIT=3 toolchain_preflight >/dev/null
assert_eq "a failed dependency install still leaves a seeded cache" \
  "$TEST_DIR/browsers-broken-deps" "$BROWSERS_CACHE_HOST"
DEFAULT_BROWSERS_DIR="$CACHE"

# ── The repo's own Playwright is what fills the cache ────────────────────────
echo ""
echo "── repo-pinned install ──"

: > "$PW_LOG"
toolchain_preflight >/dev/null
assert_contains "the repo's pinned Playwright installs the browser" \
  "playwright install chromium" "$(<"$PW_LOG")"
assert_contains "it installs into the mounted cache, not the image's copy" \
  "-> $CACHE" "$(<"$PW_LOG")"

# ── What every later container gets ──────────────────────────────────────────
echo ""
echo "── sandbox mount ──"

docker_sandbox_args
assert_contains "agents mount the cache" \
  "-v $CACHE:/opt/alucard-browsers:rw" "${DOCKER_SANDBOX_ARGS[*]}"
assert_contains "agents are pointed at it" \
  "-e PLAYWRIGHT_BROWSERS_PATH=/opt/alucard-browsers" "${DOCKER_SANDBOX_ARGS[*]}"
# Without this, installing for one repo deletes the browsers of every other —
# playwright prunes what its link files no longer point at, and preflight's
# node_modules is gone by the next run.
assert_contains "installs do not evict the other repos' browsers" \
  "-e PLAYWRIGHT_SKIP_BROWSER_GC=1" "${DOCKER_SANDBOX_ARGS[*]}"
# The image's own copy has to stay where it is: /usr/local/bin/chromium is a
# symlink into it, and the seed reads from it.
assert_not_contains "the image's baked directory is never mounted over" \
  "/opt/ms-playwright" "${DOCKER_SANDBOX_ARGS[*]}"

BROWSERS_CACHE_HOST=""
docker_sandbox_args
assert_not_contains "no cache means no mount" \
  "/opt/alucard-browsers" "${DOCKER_SANDBOX_ARGS[*]}"

# ── The safety property ──────────────────────────────────────────────────────
echo ""
echo "── falling back to the baked browser ──"

# A seed that cannot read the image's browsers must leave the mount off, not
# mount an empty directory over the path the repos rely on.
BAKED_BROWSERS_PATH="$TEST_DIR/no-such-baked-dir"
DEFAULT_BROWSERS_DIR="$TEST_DIR/browsers-unseeded"
toolchain_preflight >/dev/null
assert_eq "a failed seed mounts nothing" "" "$BROWSERS_CACHE_HOST"
BAKED_BROWSERS_PATH="$BAKED"

# A copy that dies part-way must not look finished. `tar cf` fails on an
# unreadable member while `tar xf` still exits 0 on what it received, so without
# pipefail the marker goes down on a torn cache and every later container trusts
# it. Root reads everything, so there is nothing to tear.
if [ "$(id -u)" != "0" ]; then
  TORN="$TEST_DIR/baked-torn"
  mkdir -p "$TORN/chromium-1243"
  echo readable > "$TORN/chromium-1243/ok"
  echo secret > "$TORN/unreadable"
  chmod 000 "$TORN/unreadable"
  BAKED_BROWSERS_PATH="$TORN"
  DEFAULT_BROWSERS_DIR="$TEST_DIR/browsers-torn"
  toolchain_preflight >/dev/null
  assert_eq "a torn copy is not marked as seeded" "" "$BROWSERS_CACHE_HOST"
  chmod 644 "$TORN/unreadable"
  BAKED_BROWSERS_PATH="$BAKED"
fi

# Same for a cache directory that cannot be created at all.
touch "$TEST_DIR/not-a-dir"
DEFAULT_BROWSERS_DIR="$TEST_DIR/not-a-dir/browsers"
toolchain_preflight >/dev/null
assert_eq "an uncreatable cache dir mounts nothing" "" "$BROWSERS_CACHE_HOST"

# ── What the added steps may and may not do to the preflight verdict ─────────
echo ""
echo "── preflight exit status ──"

DEFAULT_BROWSERS_DIR="$CACHE"
toolchain_preflight >/dev/null
assert_contains "a healthy repo reports its toolchain OK" "OK —" "$TOOLCHAIN_STATUS"

# The browser is a nice-to-have; dependencies installing is what the status
# line reports, and what the reviewer keys its evidence demands off.
ALUCARD_TEST_PW_EXIT=1 toolchain_preflight >/dev/null
assert_contains "a browser that will not install does not condemn the toolchain" \
  "OK —" "$TOOLCHAIN_STATUS"

# And the reverse: the added steps must not paper over a real install failure.
: > "$PW_LOG"
ALUCARD_TEST_NPM_EXIT=3 toolchain_preflight >/dev/null
assert_contains "a failed dependency install still reports BROKEN" \
  "BROKEN —" "$TOOLCHAIN_STATUS"
assert_eq "the browser step is skipped when dependencies never installed" \
  "" "$(<"$PW_LOG")"

# The browser install gets its own timeout, well inside the preflight
# container's. Sharing the outer one means a hung download kills the container
# and reports BROKEN for a repo whose dependencies installed perfectly well —
# which tells every agent, and the reviewer, not to expect test evidence.
: > "$PW_LOG"
cat > "$WORK/node_modules/.bin/playwright" <<'PW'
#!/bin/bash
exec sleep 20
PW
chmod +x "$WORK/node_modules/.bin/playwright"
ALUCARD_TEST_OUTER_TIMEOUT="5s" PLAYWRIGHT_INSTALL_TIMEOUT="1s" \
  toolchain_preflight >/dev/null
assert_contains "a hung browser install does not condemn the toolchain" \
  "OK —" "$TOOLCHAIN_STATUS"

cat > "$WORK/node_modules/.bin/playwright" <<'PW'
#!/bin/bash
echo "playwright $* -> ${PLAYWRIGHT_BROWSERS_PATH:-unset}" >> "$ALUCARD_TEST_PW_LOG"
exit "${ALUCARD_TEST_PW_EXIT:-0}"
PW
chmod +x "$WORK/node_modules/.bin/playwright"

# A repo with no Playwright at all must not have a failure invented for it.
: > "$PW_LOG"
ALUCARD_TEST_EXEC_DIR="$REPO_ABS" toolchain_preflight >/dev/null
assert_contains "a repo without Playwright preflights clean" "OK —" "$TOOLCHAIN_STATUS"
assert_eq "and nothing pretends to install a browser for it" "" "$(<"$PW_LOG")"

echo ""
echo "passed: $PASS  failed: $FAIL"
[ "$FAIL" -eq 0 ]
