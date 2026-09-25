# shellcheck shell=bash
# Assertion helpers shared by test/test_*.sh.

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

# Herestrings, not `printf | grep -q`: grep exits at the first match, printf
# dies of SIGPIPE on a haystack bigger than the pipe buffer, and under
# `set -o pipefail` a needle that IS present reads as absent.
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

finish() {
  echo
  echo "── $PASS passed, $FAIL failed ──"
  [ "$FAIL" -eq 0 ]
}
