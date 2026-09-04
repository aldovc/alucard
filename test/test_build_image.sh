#!/bin/bash
# build_image stamps the staleness label and drops the image it supersedes.
# The label matters because ensure_image reads it to decide whether the image
# is current; an unstamped image makes every later run warn or rebuild. The
# cleanup matters because retagging :latest orphans ~1.6GB of layers per
# Dockerfile change.
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
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s\n' "$haystack" | grep -qF -- "$needle"; then
    fail "$label (output unexpectedly contains '$needle')"
  else
    pass "$label"
  fi
}

TEST_DIR=$(mktemp -d /tmp/alucard_test_build_image.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TRACE="$TEST_DIR/trace"
mkdir -p "$MOCK_BIN"

# A stand-in TOOL_DIR so build_inputs_hash has something deterministic to read.
TOOLDIR="$TEST_DIR/tooldir"
mkdir -p "$TOOLDIR"
printf 'FROM scratch\n' > "$TOOLDIR/Dockerfile"
printf '#!/bin/sh\n' > "$TOOLDIR/entrypoint.sh"

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
printf '%s\n' "$*" >> "$ALUCARD_TEST_TRACE"

case "$1 ${2:-}" in
  "image inspect")
    case "$*" in
      *"{{len .RepoTags}}"*) printf '%s\n' "${ALUCARD_TEST_PREV_TAGS:-0}"; exit 0 ;;
      *"{{.Id}}"*)
        # Before the build the tag resolves to the old image, after it to the
        # new one -- exactly how a retag behaves.
        if [ -f "$ALUCARD_TEST_BUILT" ]; then
          [ -n "${ALUCARD_TEST_ID_AFTER:-}" ] || exit 1
          printf '%s\n' "$ALUCARD_TEST_ID_AFTER"
        else
          [ -n "${ALUCARD_TEST_ID_BEFORE:-}" ] || exit 1
          printf '%s\n' "$ALUCARD_TEST_ID_BEFORE"
        fi
        exit 0 ;;
    esac
    exit 1 ;;
  "build "*|"build")
    touch "$ALUCARD_TEST_BUILT"
    exit "${ALUCARD_TEST_BUILD_RC:-0}" ;;
  "rmi "*|"rmi")
    exit "${ALUCARD_TEST_RMI_RC:-0}" ;;
esac
exit 0
MOCK
chmod +x "$MOCK_BIN/docker"
PATH="$MOCK_BIN:$PATH"
export PATH ALUCARD_TEST_TRACE="$TRACE"

# shellcheck disable=SC1090
source "$ALUCARD"

OLD_ID="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
NEW_ID="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"

run_build() {
  : > "$TRACE"
  rm -f "$TEST_DIR/built"
  TOOL_DIR="$TOOLDIR" \
  ALUCARD_TEST_BUILT="$TEST_DIR/built" \
  ALUCARD_TEST_ID_BEFORE="${1-}" \
  ALUCARD_TEST_ID_AFTER="${2-}" \
  ALUCARD_TEST_PREV_TAGS="${3-0}" \
  ALUCARD_TEST_BUILD_RC="${4-0}" \
  ALUCARD_TEST_RMI_RC="${5-0}" \
    build_image "alucard:test" "abc123" >"$TEST_DIR/out" 2>&1
  BUILD_RC=$?
  OUT=$(<"$TEST_DIR/out")
  TRACED=$(<"$TRACE")
}

# ── The label ────────────────────────────────────────────────────────────────
echo "── build-inputs label ──"

run_build "" "$NEW_ID"
assert_contains "build_image stamps the staleness label" \
  "--label alucard.build-inputs=abc123" "$TRACED"

# command_build is the documented entry point and used to bypass build_image
# entirely, producing an unstamped image that defeats ensure_image's check.
: > "$TRACE"
rm -f "$TEST_DIR/built"
_rc=0
TOOL_DIR="$TOOLDIR" \
ALUCARD_TEST_BUILT="$TEST_DIR/built" \
ALUCARD_TEST_ID_BEFORE="" \
ALUCARD_TEST_ID_AFTER="$NEW_ID" \
DEFAULT_IMAGE="alucard:test" \
  command_build >/dev/null 2>&1 || _rc=$?
assert_eq "alucard build succeeds" "0" "$_rc"
assert_contains "alucard build stamps the label too" \
  "--label alucard.build-inputs=" "$(<"$TRACE")"
# The stamp must be the real digest of this TOOL_DIR, not a placeholder.
assert_contains "alucard build stamps the actual build-inputs hash" \
  "--label alucard.build-inputs=$(build_inputs_hash "$TOOLDIR")" "$(<"$TRACE")"

# ── Superseded-image cleanup ─────────────────────────────────────────────────
echo ""
echo "── superseded image cleanup ──"

run_build "$OLD_ID" "$NEW_ID" 0
assert_contains "a superseded untagged image is removed" "rmi $OLD_ID" "$TRACED"
assert_contains "the removal is reported" "Removing superseded image" "$OUT"

# A fully cached rebuild reuses the image ID -- there is nothing superseded.
run_build "$OLD_ID" "$OLD_ID" 0
assert_not_contains "an unchanged image ID is never removed" "rmi" "$TRACED"

# Another tag on the old ID means someone wants it kept.
run_build "$OLD_ID" "$NEW_ID" 2
assert_not_contains "a still-tagged old image is kept" "rmi" "$TRACED"

# Nothing was there to supersede.
run_build "" "$NEW_ID" 0
assert_not_contains "a first build removes nothing" "rmi" "$TRACED"

# ── A failed build must not delete the image the host still depends on ───────
echo ""
echo "── failed build ──"

run_build "$OLD_ID" "$NEW_ID" 0 1 || true
assert_eq "a failed build propagates its exit code" "1" "$BUILD_RC"
assert_not_contains "a failed build removes nothing" "rmi" "$TRACED"

# ── A removal the daemon refuses is advisory, not fatal ──────────────────────
echo ""
echo "── rmi refused ──"

run_build "$OLD_ID" "$NEW_ID" 0 0 1 || true
assert_eq "a refused removal does not fail the build" "0" "$BUILD_RC"
assert_contains "a refused removal warns" "could not remove superseded image" "$OUT"

echo ""
echo "Results: ${PASS} passed, ${FAIL} failed"
[ "$FAIL" -eq 0 ]
