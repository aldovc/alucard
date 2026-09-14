#!/bin/bash
# Covers the toolchain preflight on a repo with more than one dependency
# manifest: every manifest is found, each is verified in its own directory,
# the status names each one, and the worker is handed that status. Preflight
# used to check only the first manifest it found. A worker on a two-manifest
# repo then spent the last turns of its iteration discovering that the second
# directory had nothing installed, after preflight had vouched for the first.
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

assert_starts_with() {
  local label="$1" prefix="$2" text="$3"
  if [[ "$text" == "$prefix"* ]]; then
    pass "$label"
  else
    fail "$label (does not start with '$prefix': ${text:0:80})"
  fi
}

TEST_DIR=$(mktemp -d)
trap 'rm -rf "$TEST_DIR"' EXIT

# shellcheck disable=SC1090
source "$ALUCARD"

# ── detect_dependency_install ────────────────────────────────────────────────
echo "── detection ──"

# A repo layout from a list of files to touch. Echoes its root.
mk() {
  local root="$TEST_DIR/repos/$1" f
  shift
  mkdir -p "$root"
  for f in "$@"; do
    mkdir -p "$root/$(dirname "$f")"
    : > "$root/$f"
  done
  printf '%s' "$root"
}

r=$(mk none README.md)
if detect_dependency_install "$r" >/dev/null; then
  fail "no manifest returns non-zero"
else
  pass "no manifest returns non-zero"
fi
assert_eq "and prints nothing" "" "$(detect_dependency_install "$r" || true)"

r=$(mk root-py pyproject.toml)
assert_eq "a root pyproject is one uv line" $'uv sync\t.' "$(detect_dependency_install "$r")"

r=$(mk root-both pyproject.toml package-lock.json)
assert_eq "a root with both manifests lists uv, then npm" \
  $'uv sync\t.\nnpm ci\t.' "$(detect_dependency_install "$r")"

r=$(mk split backend/pyproject.toml frontend/package-lock.json docs/index.md \
      .git/pyproject.toml deep/er/pyproject.toml)
assert_eq "one level down: every manifest, sorted, nothing from .git or deeper" \
  $'uv sync\tbackend\nnpm ci\tfrontend' "$(detect_dependency_install "$r")"
SPLIT_REPO="$r"

r=$(mk mixed package-lock.json api/pyproject.toml)
assert_eq "the root comes before subdirectories" \
  $'npm ci\t.\nuv sync\tapi' "$(detect_dependency_install "$r")"

# ── toolchain_preflight, two manifests ───────────────────────────────────────
echo ""
echo "── preflight over two manifests ──"

MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
mkdir -p "$MOCK_BIN"
: > "$TRACE"

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

# Traces argv; a `run` reports which directory it was started in and fails
# there when ALUCARD_TEST_FAIL_DIRS names it. Nothing is executed.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
{ printf 'docker'; printf ' %q' "$@"; printf '\n'; } >> "$ALUCARD_TEST_TRACE"
case "$1" in
  image) echo "sha256:test"; exit 0 ;;
  run) ;;
  *) exit 0 ;;
esac
dir=""
while [ $# -gt 0 ]; do
  case "$1" in -w) dir="${2#/work/}"; shift ;; esac
  shift
done
echo "mock install in ${dir}"
for f in ${ALUCARD_TEST_FAIL_DIRS:-}; do
  [ "$f" = "$dir" ] && exit 3
done
exit 0
MOCK

cat > "$MOCK_BIN/git" <<'MOCK'
#!/bin/bash
set -euo pipefail
[ "$1" = "clone" ] && mkdir -p "${!#}"
exit 0
MOCK
chmod +x "$MOCK_BIN/timeout" "$MOCK_BIN/docker" "$MOCK_BIN/git"

# Kept for the end-to-end run below: the unit mocks include a git that does
# nothing, and a real run needs the real one.
ORIG_PATH="$PATH"
export PATH="$MOCK_BIN:$PATH"
export ALUCARD_TEST_TRACE="$TRACE"
IMAGE="alucard:test"
LOG_DIR="$TEST_DIR/logs"; mkdir -p "$LOG_DIR"
TIMEOUT_MIN=7
ENV_FILE="$TEST_DIR/env"; touch "$ENV_FILE"
BASE_BRANCH=main
CREATED_WORKTREES=()
CREATED_CONTAINERS=()
BAKED_BROWSERS_PATH="$TEST_DIR/baked"; mkdir -p "$BAKED_BROWSERS_PATH"
# An uncreatable cache directory keeps the Playwright seeding out of this test.
touch "$TEST_DIR/not-a-dir"
DEFAULT_BROWSERS_DIR="$TEST_DIR/not-a-dir/browsers"
REPO_ABS="$SPLIT_REPO"

: > "$TRACE"; : > "$LOG_DIR/events.log"
toolchain_preflight >"$TEST_DIR/pf.out" 2>&1; out=$(<"$TEST_DIR/pf.out")
trace=$(<"$TRACE")
assert_eq "one container per manifest" "2" "$(grep -c 'docker run' <<<"$trace" || true)"
assert_contains "the backend install runs in backend/" "-w /work/backend" "$trace"
assert_contains "the frontend install runs in frontend/" "-w /work/frontend" "$trace"
assert_starts_with "both fine: the status opens OK" "OK — " "$TOOLCHAIN_STATUS"
assert_contains "and names the backend install" '`uv sync` in backend/ completes' "$TOOLCHAIN_STATUS"
assert_contains "and the frontend install" '`npm ci` in frontend/ completes' "$TOOLCHAIN_STATUS"
assert_contains "and says where to run them" "run each in its own directory" "$TOOLCHAIN_STATUS"
assert_eq "the preflight log has a section per manifest" \
  "2" "$(grep -c '^=== ' "$LOG_DIR/toolchain-preflight.txt" || true)"
assert_contains "each section carries its own output" \
  "mock install in frontend" "$(<"$LOG_DIR/toolchain-preflight.txt")"
assert_eq "one OK event per manifest" \
  "2" "$(grep -c 'Toolchain preflight: OK' "$LOG_DIR/events.log" || true)"
assert_not_contains "nothing is reported broken" "TOOLCHAIN PREFLIGHT FAILED" "$out"

: > "$TRACE"; : > "$LOG_DIR/events.log"
ALUCARD_TEST_FAIL_DIRS=frontend toolchain_preflight >"$TEST_DIR/pf.out" 2>&1; out=$(<"$TEST_DIR/pf.out")
assert_starts_with "one broken: the status opens BROKEN" "BROKEN — " "$TOOLCHAIN_STATUS"
assert_contains "it names the failing install with its rc" \
  '`npm ci` in frontend/ fails inside the container (rc=3)' "$TOOLCHAIN_STATUS"
assert_contains "and the one that works" '`uv sync` in backend/ completes' "$TOOLCHAIN_STATUS"
assert_contains "and scopes the consequence to the broken directory" \
  "CANNOT install dependencies for frontend/" "$TOOLCHAIN_STATUS"
assert_contains "without waiving evidence elsewhere" \
  "For the rest of the repo, missing test evidence is a real gap" "$TOOLCHAIN_STATUS"
assert_contains "the banner names the failing install" '`npm ci` in frontend/ exited 3' "$out"
assert_contains "and shows that install's last lines" "mock install in frontend" "$out"
assert_not_contains "not the other manifest's" "mock install in backend" "$out"
assert_eq "one OK and one BROKEN event" "1|1" \
  "$(grep -c 'Toolchain preflight: OK' "$LOG_DIR/events.log" || true)|$(grep -c 'Toolchain preflight: BROKEN' "$LOG_DIR/events.log" || true)"

ALUCARD_TEST_FAIL_DIRS="backend frontend" toolchain_preflight >"$TEST_DIR/pf.out" 2>&1; out=$(<"$TEST_DIR/pf.out")
assert_starts_with "all broken: the status opens BROKEN" "BROKEN — " "$TOOLCHAIN_STATUS"
assert_contains "and names both failures" '`uv sync` in backend/ fails inside the container (rc=3)' "$TOOLCHAIN_STATUS"
assert_contains "with the whole-repo consequence" \
  "cannot run this repo's lint or test suite" "$TOOLCHAIN_STATUS"
assert_not_contains "and no partial wording" "For the rest of the repo" "$TOOLCHAIN_STATUS"

# ── A single root manifest keeps its one container ───────────────────────────
echo ""
echo "── single root manifest ──"

REPO_ABS="$TEST_DIR/repos/root-py"
: > "$TRACE"
toolchain_preflight >/dev/null 2>&1
assert_eq "exactly one container" "1" "$(grep -c 'docker run' "$TRACE" || true)"
assert_starts_with "the status names the root" \
  'OK — `uv sync` at the repo root completes in the container.' "$TOOLCHAIN_STATUS"

# ── End to end: the worker is handed the status ──────────────────────────────
# The helper being right is not the same as it being wired in. Drive `alucard
# run` against a fixture repo and read the prompt the harness archived — that
# is what the worker actually received.
echo ""
echo "── the worker prompt carries the block ──"

# The unit mocks above include a git that does nothing; building a fixture
# repo and running the harness both need the real one.
export PATH="$ORIG_PATH"

E2E="$TEST_DIR/e2e"
E2E_BIN="$E2E/bin"
TARGET="$E2E/target"
REMOTE="$E2E/remote.git"
mkdir -p "$E2E_BIN" "$TARGET/.alucard"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$E2E/alucard.env"

git init -q --bare "$REMOTE"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '[project]\nname = "fixture"\n' > "$TARGET/pyproject.toml"
printf '# Plan\n\n## [ ] 1: A task\n\nBlocked by: none\n' > "$TARGET/.alucard/tasks.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm initial
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main

cp "$MOCK_BIN/timeout" "$E2E_BIN/timeout"
# The preflight container installs fine; the worker exits clean without a PR.
cat > "$E2E_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
for _a in "$@"; do
  case "$_a" in alucard-preflight-*) echo "installed"; exit 0 ;; esac
done
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
MOCK
cat > "$E2E_BIN/gh" <<'MOCK'
#!/bin/bash
exit 0
MOCK
chmod +x "$E2E_BIN/docker" "$E2E_BIN/gh"

set +e
PATH="$E2E_BIN:$PATH" "$ALUCARD" run "$TARGET" --iterations 1 --no-build \
  --env-file "$E2E/alucard.env" --logs-root "$E2E/logs" > "$E2E/out" 2>&1
rc=$?
set -e
assert_eq "the run completes" "0" "$rc"
[ "$rc" -eq 0 ] || tail -n 20 "$E2E/out" >&2

worker_prompt=""
for f in "$E2E"/logs/alucard-*/prompts/iter-1.txt; do
  [ -f "$f" ] && worker_prompt=$(<"$f")
done
if [ -z "$worker_prompt" ]; then
  fail "the harness archived the worker prompt (see $E2E/out)"
else
  pass "the harness archived the worker prompt"
  # The role prompt documents the tag in prose, so count the tag alone on its
  # line — that is the injected block, and there must be exactly one.
  assert_eq "the worker prompt carries exactly one block" \
    "1" "$(grep -cx '<toolchain_status>' <<<"$worker_prompt" || true)"
  assert_contains "with the verified install and where it belongs" \
    'OK — `uv sync` at the repo root completes in the container.' \
    "$(awk '/^<toolchain_status>$/{f=1;next} /^<\/toolchain_status>$/{f=0} f' <<<"$worker_prompt")"
fi
assert_contains "alucard-worker-prompt.md explains <toolchain_status>" \
  '`<toolchain_status>`' "$(cat "$SCRIPT_DIR/../alucard-worker-prompt.md")"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
