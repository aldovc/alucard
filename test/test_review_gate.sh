#!/bin/bash
# Covers the review-loop convergence machinery: verdict reconciliation
# (including the BLOCKED escape hatch), the blocked-findings ledger filter, and
# dependency-toolchain detection for the startup preflight.
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
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    pass "$label"
  else
    fail "$label (output does not contain '$needle')"
  fi
}

assert_not_contains() {
  local label="$1" needle="$2" haystack="$3"
  if printf '%s' "$haystack" | grep -qF "$needle"; then
    fail "$label (output unexpectedly contains '$needle')"
  else
    pass "$label"
  fi
}

# Source alucard to load helper functions without running main.
# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_review_gate.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── Test group 1: resolve_review_verdict ─────────────────────────────────────
echo "── resolve_review_verdict ──"

assert_eq "formal APPROVED with no decision file" \
  "APPROVED" "$(resolve_review_verdict "APPROVED" "" || true)"

assert_eq "formal CHANGES_REQUESTED with no decision file" \
  "CHANGES_REQUESTED" "$(resolve_review_verdict "CHANGES_REQUESTED" "" || true)"

# The self-review case: GitHub refuses the review, so the file is all we have.
assert_eq "no formal review falls back to decision file APPROVED" \
  "APPROVED" "$(resolve_review_verdict "" "APPROVED" || true)"

assert_eq "no formal review falls back to decision file CHANGES_REQUESTED" \
  "CHANGES_REQUESTED" "$(resolve_review_verdict "" "CHANGES_REQUESTED" || true)"

assert_eq "no formal review falls back to decision file BLOCKED" \
  "BLOCKED" "$(resolve_review_verdict "" "BLOCKED" || true)"

# GitHub has no BLOCKED state, so a blocked reviewer must post request-changes
# formally. The file has to win or the loop would never see BLOCKED at all.
assert_eq "decision-file BLOCKED overrides formal CHANGES_REQUESTED" \
  "BLOCKED" "$(resolve_review_verdict "CHANGES_REQUESTED" "BLOCKED" || true)"

assert_eq "decision-file BLOCKED overrides formal APPROVED" \
  "BLOCKED" "$(resolve_review_verdict "APPROVED" "BLOCKED" || true)"

# A formal review outranks the file for the states GitHub can express.
assert_eq "formal CHANGES_REQUESTED outranks decision-file APPROVED" \
  "CHANGES_REQUESTED" "$(resolve_review_verdict "CHANGES_REQUESTED" "APPROVED" || true)"

assert_eq "no verdict from either source yields empty" \
  "" "$(resolve_review_verdict "" "" || true)"

assert_eq "garbage decision-file state is ignored" \
  "" "$(resolve_review_verdict "" "LGTM PROBABLY" || true)"

_rc=0
resolve_review_verdict "" "" >/dev/null || _rc=$?
assert_eq "no verdict returns exit 1" "1" "$_rc"

_rc=0
resolve_review_verdict "" "BLOCKED" >/dev/null || _rc=$?
assert_eq "BLOCKED returns exit 0" "0" "$_rc"

# ── Test group 2: blocked-findings ledger filter ─────────────────────────────
echo "── blocked-findings ledger ──"

# Mirrors the jq filter review_gate uses to reload the ledger from the PR.
ledger_filter='[.comments[] | select(.body | startswith("**🤖 Alucard blocked findings**")) | .body] | join("\n\n")'

comments_json=$(cat <<'JSON'
{"comments":[
  {"body":"**🤖 Alucard review cycle 1/10: CHANGES_REQUESTED**\n\nsome finding"},
  {"body":"**🤖 Alucard blocked findings** (cycle 1/10)\n\n- **Finding**: run the Scheduler job manually\n  **Why it cannot be done here**: `gcloud: command not found` (exit 127)"},
  {"body":"a human comment asking a question"},
  {"body":"**🤖 Alucard blocked findings** (cycle 3/10)\n\n- **Finding**: attach a production HTTP 200"}
]}
JSON
)

ledger=$(printf '%s' "$comments_json" | jq -r "$ledger_filter")

assert_contains "ledger picks up the first blocked marker" \
  "gcloud: command not found" "$ledger"
assert_contains "ledger picks up a later blocked marker" \
  "attach a production HTTP 200" "$ledger"
assert_not_contains "ledger excludes review-cycle comments" \
  "some finding" "$ledger"
assert_not_contains "ledger excludes human comments" \
  "a human comment asking a question" "$ledger"

# The human-comment filter feeding the feedback agent must exclude the blocked
# marker too, or blocked findings would be re-injected as "human" findings.
human_filter='[.[] | select(.body | startswith("**🤖 Alucard") | not) | .body] | join("\n\n")'
humans=$(printf '%s' "$comments_json" | jq -r ".comments | $human_filter")

assert_contains "human filter keeps genuine human comments" \
  "a human comment asking a question" "$humans"
assert_not_contains "human filter excludes the blocked-findings marker" \
  "gcloud: command not found" "$humans"
assert_not_contains "human filter excludes review-cycle comments" \
  "some finding" "$humans"

# A PR with no markers yet must yield an empty ledger, not an error.
empty_ledger=$(printf '%s' '{"comments":[{"body":"nothing to see"}]}' | jq -r "$ledger_filter")
assert_eq "ledger is empty when no blocked markers exist" "" "$empty_ledger"

# ── Test group 3: detect_dependency_install ──────────────────────────────────
echo "── detect_dependency_install ──"

mk_repo() {
  local name="$1"
  shift
  local dir="$TMP_ROOT/$name"
  mkdir -p "$dir"
  local f
  for f in "$@"; do
    mkdir -p "$dir/$(dirname "$f")"
    : > "$dir/$f"
  done
  printf '%s' "$dir"
}

repo=$(mk_repo py-root "pyproject.toml")
assert_eq "root pyproject.toml -> uv sync at ." \
  "uv sync	." "$(detect_dependency_install "$repo" || true)"

repo=$(mk_repo py-nested "backend/pyproject.toml")
assert_eq "nested pyproject.toml -> uv sync at backend" \
  "uv sync	backend" "$(detect_dependency_install "$repo" || true)"

repo=$(mk_repo npm-root "package-lock.json")
assert_eq "root package-lock.json -> npm ci at ." \
  "npm ci	." "$(detect_dependency_install "$repo" || true)"

repo=$(mk_repo npm-nested "web/package-lock.json")
assert_eq "nested package-lock.json -> npm ci at web" \
  "npm ci	web" "$(detect_dependency_install "$repo" || true)"

# Root manifest wins over a nested one so a monorepo's top-level project is
# preflighted rather than an arbitrary subdirectory.
repo=$(mk_repo py-both "pyproject.toml" "backend/pyproject.toml")
assert_eq "root manifest wins over nested" \
  "uv sync	." "$(detect_dependency_install "$repo" || true)"

# Python is checked before Node when a repo has both.
repo=$(mk_repo mixed "backend/pyproject.toml" "web/package-lock.json")
assert_eq "python manifest checked before node" \
  "uv sync	backend" "$(detect_dependency_install "$repo" || true)"

# Deterministic pick when several nested manifests exist.
repo=$(mk_repo py-multi "svc-b/pyproject.toml" "svc-a/pyproject.toml")
assert_eq "multiple nested manifests pick deterministically" \
  "uv sync	svc-a" "$(detect_dependency_install "$repo" || true)"

repo=$(mk_repo none "README.md")
_rc=0
detect_dependency_install "$repo" >/dev/null || _rc=$?
assert_eq "no manifest returns exit 1" "1" "$_rc"
assert_eq "no manifest echoes nothing" "" "$(detect_dependency_install "$repo" || true)"

# Depth is bounded at one level — a manifest three deep is not the repo's own.
repo=$(mk_repo too-deep "a/b/pyproject.toml")
_rc=0
detect_dependency_install "$repo" >/dev/null || _rc=$?
assert_eq "manifest deeper than one level is not detected" "1" "$_rc"

# A vendored dependency's own manifest inside .git must never be picked up.
repo=$(mk_repo git-only ".git/pyproject.toml")
_rc=0
detect_dependency_install "$repo" >/dev/null || _rc=$?
assert_eq "manifest inside .git is ignored" "1" "$_rc"

# ── Test group 4: build_inputs_hash ──────────────────────────────────────────
echo "── build_inputs_hash ──"

tool_dir="$TMP_ROOT/tooldir"
mkdir -p "$tool_dir"
printf 'FROM node:24\n' > "$tool_dir/Dockerfile"
printf '#!/bin/bash\nexec "$@"\n' > "$tool_dir/entrypoint.sh"

base_hash=$(build_inputs_hash "$tool_dir")
assert_eq "hash is a sha256 hex digest" "64" "${#base_hash}"
assert_eq "hash is stable across calls" "$base_hash" "$(build_inputs_hash "$tool_dir")"

# mtime must NOT affect the hash: a fresh checkout rewrites every mtime, and an
# mtime-based check would rebuild the image on every run forever.
touch -d "@9000000" "$tool_dir/Dockerfile" "$tool_dir/entrypoint.sh"
assert_eq "mtime changes do not change the hash" \
  "$base_hash" "$(build_inputs_hash "$tool_dir")"

printf 'FROM node:24\nRUN apt-get install -y build-essential\n' > "$tool_dir/Dockerfile"
changed_hash=$(build_inputs_hash "$tool_dir")
if [ "$changed_hash" != "$base_hash" ]; then
  pass "Dockerfile content change changes the hash"
else
  fail "Dockerfile content change changes the hash"
fi

printf '#!/bin/bash\ngit config --global user.name bot\nexec "$@"\n' > "$tool_dir/entrypoint.sh"
if [ "$(build_inputs_hash "$tool_dir")" != "$changed_hash" ]; then
  pass "entrypoint.sh content change changes the hash"
else
  fail "entrypoint.sh content change changes the hash"
fi

# An unrelated file must not force rebuilds.
before_unrelated=$(build_inputs_hash "$tool_dir")
printf 'hello\n' > "$tool_dir/README.md"
assert_eq "unrelated file does not change the hash" \
  "$before_unrelated" "$(build_inputs_hash "$tool_dir")"

# Moving content between the two files must still change the digest — the name
# markers exist for exactly this case.
swap_dir="$TMP_ROOT/tooldir-swap"
mkdir -p "$swap_dir"
printf 'AAA\n' > "$swap_dir/Dockerfile"
printf 'BBB\n' > "$swap_dir/entrypoint.sh"
swap_a=$(build_inputs_hash "$swap_dir")
printf 'BBB\n' > "$swap_dir/Dockerfile"
printf 'AAA\n' > "$swap_dir/entrypoint.sh"
if [ "$(build_inputs_hash "$swap_dir")" != "$swap_a" ]; then
  pass "swapping content between build inputs changes the hash"
else
  fail "swapping content between build inputs changes the hash"
fi

# A missing build input must not be fatal.
empty_dir="$TMP_ROOT/tooldir-empty"
mkdir -p "$empty_dir"
_rc=0
empty_hash=$(build_inputs_hash "$empty_dir") || _rc=$?
assert_eq "missing build inputs is not fatal" "0" "$_rc"
assert_eq "missing build inputs still yields a digest" "64" "${#empty_hash}"

echo ""
echo "Results: $PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
