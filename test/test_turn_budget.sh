#!/bin/bash
# Covers the per-role turn cap reaching the agent that is running under it:
# the block's shape, that the number is the one the harness enforces rather
# than a fixed default, and that a codex-backed role is told nothing (no
# --max-turns is passed there, so a cap would be a claim nothing enforces).
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

assert_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if grep -qF -- "$needle" <<<"$haystack"; then
    fail "$label (output unexpectedly contains '$needle')"
  else
    pass "$label"
  fi
}

# The number, not the sentence around it: assertions here must survive a
# reworded block but fail on a wrong or frozen value.
assert_has_number() {
  local label="$1" number="$2" haystack="$3"
  if grep -qw -- "$number" <<<"$haystack"; then
    pass "$label"
  else
    fail "$label (no '$number' in: $haystack)"
  fi
}

# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_turn_budget.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── append_turn_budget ───────────────────────────────────────────────────────
echo "── append_turn_budget ──"

base="<instructions>x</instructions>
<pr_num>1</pr_num>"

withb=$(append_turn_budget "$base" claude 45)
assert_contains "the original prompt survives intact" "<pr_num>1</pr_num>" "$withb"
assert_eq "the block opens exactly once, on its own line" \
  "1" "$(grep -cx '<turn_budget>' <<<"$withb")"
assert_eq "the block is closed exactly once, on its own line" \
  "1" "$(grep -cx '</turn_budget>' <<<"$withb")"
assert_has_number "the block states the cap" "45" "$withb"

# A cap that changes must show up as a changed number — a hardcoded default
# would pass every other assertion here.
seven=$(append_turn_budget "$base" claude 7)
assert_has_number "a different cap yields a different number" "7" "$seven"
assert_not_contains "no stale default is carried over" "45" "$seven"

# Codex is invoked without --max-turns, so there is no cap to announce.
assert_eq "a codex role's prompt is byte-identical" \
  "$base" "$(append_turn_budget "$base" codex 45)"
assert_eq "a missing cap leaves the prompt byte-identical" \
  "$base" "$(append_turn_budget "$base" claude "")"

# ── End to end: the block reaches the real prompts ───────────────────────────
# The helper being right is not the same as it being wired in. Drive `alucard
# continue` against a fixture repo and read the prompts the harness captures to
# disk — those are what the agents actually received.
echo "── assembled prompts ──"

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

# Records its own argv so the number in the prompt can be compared against the
# cap actually handed to the CLI.
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

# Values no default uses, so a frozen number cannot pass by coincidence.
REVIEWER_TURNS=41
FEEDBACK_TURNS=37

# Two cycles, so cycle 1's CHANGES_REQUESTED launches a feedback agent rather
# than hitting the cycle-exhaustion exit.
run_continue() {
  rm -rf "$TMP_ROOT/logs"
  : > "$TMP_ROOT/argv"
  set +e
  env PATH="$MOCK_BIN:$PATH" ALUCARD_TEST_BRANCH="$BRANCH" \
    ALUCARD_TEST_ARGV="$TMP_ROOT/argv" \
    ALUCARD_REVIEWER_MAX_TURNS="$REVIEWER_TURNS" \
    ALUCARD_FEEDBACK_MAX_TURNS="$FEEDBACK_TURNS" \
    "$@" \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 2 \
      --env-file "$TMP_ROOT/alucard.env" --logs-root "$TMP_ROOT/logs" \
      > "$TMP_ROOT/out" 2>&1
  set -e
}

# Both role prompts *document* <turn_budget> in prose, so a substring match is
# true whether or not anything was injected. The injected block is the tag on a
# line of its own; count those, one digit per captured prompt.
blocks_per_prompt() {
  local pattern="$1" f n out=""
  for f in "$TMP_ROOT"/logs/alucard-*/prompts/*"$pattern"*.txt; do
    [ -f "$f" ] || continue
    n=$(grep -cx '<turn_budget>' "$f" || true)
    out="${out}${n}"
  done
  printf '%s' "$out"
}

# Only what is inside the block, so an unrelated number elsewhere in the prompt
# cannot satisfy the number assertions.
block_text() {
  local pattern="$1" f
  for f in "$TMP_ROOT"/logs/alucard-*/prompts/*"$pattern"*.txt; do
    [ -f "$f" ] || continue
    sed -n '/^<turn_budget>$/,/^<\/turn_budget>$/p' "$f"
  done
}

run_continue
if [ -z "$(blocks_per_prompt review)" ] || [ -z "$(blocks_per_prompt feedback)" ]; then
  fail "harness captured reviewer and feedback prompts (see $TMP_ROOT/out)"
else
  pass "harness captured reviewer and feedback prompts"

  assert_eq "every reviewer prompt carries exactly one block" \
    "11" "$(blocks_per_prompt review)"
  assert_eq "the feedback prompt carries exactly one block" \
    "1" "$(blocks_per_prompt feedback)"
  assert_has_number "the reviewer is told its own cap" \
    "$REVIEWER_TURNS" "$(block_text review)"
  assert_has_number "the feedback agent is told its own cap" \
    "$FEEDBACK_TURNS" "$(block_text feedback)"
  assert_not_contains "the reviewer is not told the feedback role's cap" \
    "$FEEDBACK_TURNS" "$(block_text review)"

  # The number the agent reads and the cap the CLI enforces must be one value.
  assert_contains "the reviewer's cap is the one passed to the CLI" \
    "--max-turns $REVIEWER_TURNS" "$(cat "$TMP_ROOT/argv")"
  assert_contains "the feedback agent's cap is the one passed to the CLI" \
    "--max-turns $FEEDBACK_TURNS" "$(cat "$TMP_ROOT/argv")"
fi

# ── Codex: no cap is enforced, so none is announced ──────────────────────────
echo "── codex ──"

run_continue ALUCARD_PROVIDER=codex
if [ -z "$(blocks_per_prompt review)" ] || [ -z "$(blocks_per_prompt feedback)" ]; then
  fail "harness captured codex prompts (see $TMP_ROOT/out)"
else
  pass "harness captured codex prompts"

  assert_eq "no block in a codex reviewer prompt" \
    "00" "$(blocks_per_prompt review)"
  assert_eq "no block in a codex feedback prompt" \
    "0" "$(blocks_per_prompt feedback)"
  assert_not_contains "no cap is passed to codex either" \
    "--max-turns" "$(cat "$TMP_ROOT/argv")"
fi

# A mixed run: the claude role still gets its block when another role is codex.
run_continue ALUCARD_PROVIDER=codex ALUCARD_REVIEWER_PROVIDER=claude
assert_eq "a claude role keeps its block alongside a codex one" \
  "11" "$(blocks_per_prompt review)"
assert_eq "the codex role beside it still gets none" \
  "0" "$(blocks_per_prompt feedback)"

echo
echo "── $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
