#!/bin/bash
# Covers the shared engineering-policy fragment: that it reaches the worker,
# reviewer and feedback roles, that it does NOT reach ci-fix (whose own prompt
# carries a tighter guard), that the worker's mode section still comes last,
# and that the superseded blanket rules were removed from the role prompts
# rather than left to contradict the fragment.
set -euo pipefail

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ALUCARD="$SCRIPT_DIR/../alucard"
ROOT="$SCRIPT_DIR/.."

PASS=0
FAIL=0

pass() { echo "PASS: $1"; PASS=$((PASS + 1)); }
fail() { echo "FAIL: $1" >&2; FAIL=$((FAIL + 1)); }

# These match with a herestring rather than the `printf | grep -q` idiom the
# other test files use. One haystack here is the whole 138 KB alucard source:
# `grep -q` exits at the first match, `printf` then dies of SIGPIPE, and under
# `set -o pipefail` the pipeline reports 141 — so a needle that IS present reads
# as absent. Small haystacks fit the pipe buffer and hide it. Do not "simplify"
# these back into a pipe.
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

# Source alucard to load helper functions without running main.
# shellcheck disable=SC1090
source "$ALUCARD"

# A line unique to the fragment, so a match cannot come from a role prompt.
POLICY_MARK="Similar-looking code is not automatically duplication"

# ── The fragment reaches the three roles that get it ─────────────────────────
echo "── policy assembly ──"

worker_github=$(compose_role_prompt alucard-worker-prompt.md alucard-worker-github-prompt.md)
worker_local=$(compose_role_prompt alucard-worker-prompt.md alucard-worker-local-prompt.md)
reviewer=$(compose_role_prompt alucard-reviewer-prompt.md)
feedback=$(compose_role_prompt alucard-feedback-prompt.md)

assert_contains "worker (github mode) carries the policy" "$POLICY_MARK" "$worker_github"
assert_contains "worker (file mode) carries the policy"   "$POLICY_MARK" "$worker_local"
assert_contains "reviewer carries the policy"             "$POLICY_MARK" "$reviewer"
assert_contains "feedback carries the policy"             "$POLICY_MARK" "$feedback"

assert_contains "worker (github mode) keeps its mode section" \
  "$(head -1 "$ROOT/alucard-worker-github-prompt.md")" "$worker_github"
assert_contains "worker (file mode) keeps its mode section" \
  "$(head -1 "$ROOT/alucard-worker-local-prompt.md")" "$worker_local"

# The worker prompt tells the agent the Mode section is "at the end of these
# instructions", so the policy must be spliced in ahead of it.
mode_line=$(printf '%s\n' "$worker_github" | grep -nF \
  "$(head -1 "$ROOT/alucard-worker-github-prompt.md")" | head -1 | cut -d: -f1)
policy_line=$(printf '%s\n' "$worker_github" | grep -nF "$POLICY_MARK" | head -1 | cut -d: -f1)
if [ -n "$policy_line" ] && [ -n "$mode_line" ] && [ "$policy_line" -lt "$mode_line" ]; then
  pass "worker mode section still comes after the policy"
else
  fail "worker mode section no longer last (policy at '$policy_line', mode at '$mode_line')"
fi

# ── CI-fix is deliberately excluded ──────────────────────────────────────────
echo "── ci-fix exclusion ──"

assert_not_contains "ci-fix prompt file has no policy text" \
  "$POLICY_MARK" "$(cat "$ROOT/alucard-ci-fix-prompt.md")"
assert_not_contains "ci-fix is not composed with the policy" \
  "compose_role_prompt alucard-ci-fix-prompt.md" "$(cat "$ALUCARD")"
assert_contains "ci-fix prompt keeps its own tighter guard" \
  "Edit only the files needed to fix the failing checks" \
  "$(cat "$ROOT/alucard-ci-fix-prompt.md")"

# ── doctor checks for the fragment ───────────────────────────────────────────
echo "── doctor ──"

assert_contains "doctor's required files include the fragment" \
  "alucard-engineering-policy.md" "$(printf '%s\n' "${REQUIRED_TOOL_FILES[@]}")"

# ── Superseded copies are gone from the role prompts ─────────────────────────
echo "── superseded rules removed ──"

assert_not_contains "worker no longer repeats per criterion" \
  "Repeat per criterion" "$(cat "$ROOT/alucard-worker-prompt.md")"
assert_not_contains "worker has no blanket two-copies rule" \
  "No logic block appears in two places" "$(cat "$ROOT/alucard-worker-prompt.md")"
assert_not_contains "worker has no blanket magic-value rule" \
  "carries domain meaning is a named constant" "$(cat "$ROOT/alucard-worker-prompt.md")"
assert_not_contains "reviewer has no blanket duplicated-logic finding" \
  "The fix is extraction, not tolerance" "$(cat "$ROOT/alucard-reviewer-prompt.md")"
assert_not_contains "reviewer has no magic-values checklist" \
  "### Magic values" "$(cat "$ROOT/alucard-reviewer-prompt.md")"
assert_not_contains "reviewer no longer mandates polymorphism" \
  "Stringly-typed dispatch" "$(cat "$ROOT/alucard-reviewer-prompt.md")"
assert_not_contains "feedback has no blanket magic-value rule" \
  "literal with domain meaning that should be a named constant" \
  "$(cat "$ROOT/alucard-feedback-prompt.md")"

echo
echo "── $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
