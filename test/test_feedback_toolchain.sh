#!/bin/bash
# Covers the dependency-preflight result reaching the feedback agent, not only
# the reviewer. The feedback agent is the one required to run lint and tests,
# so a run where it never learns the install command ends in a failed
# verification against a bare interpreter.
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

# Herestring, not a pipe: `printf | grep -q` under pipefail returns 141 on a
# large haystack, because grep exits before printf has finished writing.
assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

TMP_ROOT=$(mktemp -d /tmp/alucard_test_feedback_toolchain.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

MOCK_BIN="$TMP_ROOT/bin"; mkdir -p "$MOCK_BIN"
TARGET="$TMP_ROOT/target"
REMOTE="$TMP_ROOT/remote.git"
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

cp "$SCRIPT_DIR/fixtures/credentials.env" "$TMP_ROOT/alucard.env"

# One docker mock for every scenario: the preflight container exits with
# whatever ALUCARD_TEST_PREFLIGHT_RC says, so a repo whose dependencies do not
# build can be driven without a real broken image.
cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  run) ;;
  *) exit 0 ;;
esac
for _a in "$@"; do
  case "$_a" in
    alucard-preflight-*)
      echo "mock install output"
      exit "${ALUCARD_TEST_PREFLIGHT_RC:-0}" ;;
    *:/work-output:rw)
      printf 'CHANGES_REQUESTED\n' > "${_a%%:*}/.alucard-review"
      printf 'fix the thing\n' > "${_a%%:*}/.alucard-review-body" ;;
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

# Two cycles, so cycle 1's CHANGES_REQUESTED launches a feedback agent rather
# than hitting the cycle-exhaustion exit.
run_continue() {
  rm -rf "$TMP_ROOT/logs"
  set +e
  PATH="$MOCK_BIN:$PATH" ALUCARD_TEST_BRANCH="$BRANCH" \
    ALUCARD_TEST_PREFLIGHT_RC="${1:-0}" \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 2 \
      --env-file "$TMP_ROOT/alucard.env" --logs-root "$TMP_ROOT/logs" \
      > "$TMP_ROOT/out" 2>&1
  set -e
}

# Both role prompts *document* <toolchain_status> in prose, so a substring match
# is true whether or not anything was injected. The injected block opens with
# the tag alone on its line; each captured prompt must carry exactly one.
blocks_per_prompt() {
  local pattern="$1" f n out=""
  for f in "$TMP_ROOT"/logs/alucard-*/prompts/*"$pattern"*.txt; do
    [ -f "$f" ] || continue
    n=$(grep -cx '<toolchain_status>' "$f" || true)
    out="${out}${n}"
  done
  printf '%s' "$out"
}

# The status text itself: what sits between the tag and its closer in the first
# captured prompt of that role.
injected_status() {
  local pattern="$1" f
  for f in "$TMP_ROOT"/logs/alucard-*/prompts/*"$pattern"*.txt; do
    [ -f "$f" ] || continue
    awk '/^<toolchain_status>$/{flag=1;next} /^<\/toolchain_status>$/{flag=0} flag' "$f"
    return 0
  done
}

echo "── no manifest: preflight skipped ──"
run_continue 0
if [ -z "$(blocks_per_prompt review)" ] || [ -z "$(blocks_per_prompt feedback)" ]; then
  fail "harness captured reviewer and feedback prompts (see $TMP_ROOT/out)"
else
  pass "harness captured reviewer and feedback prompts"

  assert_eq "every reviewer prompt carries exactly one block" \
    "11" "$(blocks_per_prompt review)"
  assert_eq "the feedback prompt carries exactly one block" \
    "1" "$(blocks_per_prompt feedback)"
  assert_contains "the feedback agent is told the preflight was skipped" \
    "No Python or Node manifest found" "$(injected_status feedback)"
  assert_eq "both agents are told the same thing" \
    "$(injected_status review)" "$(injected_status feedback)"
fi

# A repo whose dependencies do build: the whole point is that the feedback
# agent gets the install command *before* it tries to verify anything.
echo
echo "── dependencies install: the command reaches the feedback agent ──"
printf '[project]\nname = "fixture"\n' > "$TARGET/pyproject.toml"
run_continue 0
assert_eq "the feedback prompt still carries exactly one block" \
  "1" "$(blocks_per_prompt feedback)"
assert_contains "the feedback agent is given the verified install command" \
  'OK — `uv sync` completes in the container.' "$(injected_status feedback)"
assert_eq "both agents are told the same thing" \
  "$(injected_status review)" "$(injected_status feedback)"

echo
echo "── dependencies do not install: the feedback agent is told so ──"
run_continue 1
assert_eq "the feedback prompt still carries exactly one block" \
  "1" "$(blocks_per_prompt feedback)"
assert_contains "the feedback agent is told it cannot install" \
  'BROKEN — `uv sync` fails' "$(injected_status feedback)"
assert_contains "and told what that costs it" \
  "cannot run this repo's lint or test suite" "$(injected_status feedback)"
assert_eq "both agents are told the same thing" \
  "$(injected_status review)" "$(injected_status feedback)"

# An injected block nobody explained is noise: the role prompt has to say what
# the agent should do with it.
echo
echo "── the role prompt documents the input ──"
assert_contains "alucard-feedback-prompt.md explains <toolchain_status>" \
  '`<toolchain_status>`' "$(cat "$SCRIPT_DIR/../alucard-feedback-prompt.md")"

echo
echo "── $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
