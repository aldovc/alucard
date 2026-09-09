#!/bin/bash
# Covers the feedback-failure exit from the review loop: the stall decision,
# the remote-SHA lookup it rests on, and the bookkeeping run_feedback_once
# does around the agent invocation.
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

# Source alucard to load helper functions without running main.
# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_feedback_stall.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── feedback_stalled ─────────────────────────────────────────────────────────
echo "── feedback_stalled ──"

stalled() { if feedback_stalled "$1" "$2"; then echo yes; else echo no; fi; }

assert_eq "failed agent that pushed nothing is a stall" \
  "yes" "$(stalled 1 false)"
assert_eq "failed agent that pushed is not a stall" \
  "no" "$(stalled 1 true)"
# An unreadable SHA must never be read as "no progress" — that would end the
# loop on a network blip while the agent's work sits on the branch.
assert_eq "failed agent with an unknown SHA is not a stall" \
  "no" "$(stalled 1 unknown)"
assert_eq "successful agent is never a stall" \
  "no" "$(stalled 0 false)"
assert_eq "successful agent that pushed is never a stall" \
  "no" "$(stalled 0 true)"

# ── remote_branch_sha ────────────────────────────────────────────────────────
echo "── remote_branch_sha ──"

ORIGIN="$TMP_ROOT/origin.git"
WORK="$TMP_ROOT/work"
git init --quiet --bare "$ORIGIN"
git init --quiet "$WORK"
git -C "$WORK" config user.email t@example.com
git -C "$WORK" config user.name Test
git -C "$WORK" remote add origin "$ORIGIN"
echo one > "$WORK/f"
git -C "$WORK" add f
git -C "$WORK" commit --quiet -m one
git -C "$WORK" branch -M feature
git -C "$WORK" push --quiet -u origin feature

sha1=$(remote_branch_sha "$WORK" feature)
assert_eq "resolves the pushed branch to its real SHA" \
  "$(git -C "$WORK" rev-parse HEAD)" "$sha1"

# Push from a *separate* clone, the way a disposable agent worktree does. The
# host checkout's refs/remotes/origin/feature is stale afterwards, so only a
# real fetch sees the new commit — reading the tracking ref would report the
# branch as unmoved and strand the loop.
AGENT="$TMP_ROOT/agent"
git clone --quiet "$ORIGIN" "$AGENT"
git -C "$AGENT" config user.email t@example.com
git -C "$AGENT" config user.name Test
git -C "$AGENT" checkout --quiet feature
echo two > "$AGENT/f"
git -C "$AGENT" commit --quiet -am two
git -C "$AGENT" push --quiet origin feature
sha2=$(remote_branch_sha "$WORK" feature)
if [ "$sha1" != "$sha2" ]; then
  pass "sees a new SHA after the branch moves"
else
  fail "sees a new SHA after the branch moves (both '$sha1')"
fi

if remote_branch_sha "$WORK" no-such-branch >/dev/null 2>&1; then
  fail "fails on a branch the remote does not have"
else
  pass "fails on a branch the remote does not have"
fi

# ── run_feedback_once bookkeeping ────────────────────────────────────────────
echo "── run_feedback_once ──"

# Stub the container-facing parts. invoke_agent stands in for the agent: it
# optionally pushes (as a real agent would) and exits with a chosen code.
STUB_RC=0
STUB_PUSHES=false
make_agent_clone() { mkdir -p "$2"; }
log_event() { :; }
measure_stage() { :; }
invoke_agent() {
  # Pushes from the agent clone, not the host checkout, so the host's
  # remote-tracking ref stays stale exactly as it does in a real run.
  if [ "$STUB_PUSHES" = "true" ]; then
    git -C "$AGENT" fetch --quiet origin feature
    git -C "$AGENT" reset --quiet --hard FETCH_HEAD
    echo "$RANDOM-$(date +%s%N)" > "$AGENT/f"
    git -C "$AGENT" commit --quiet -am agent
    git -C "$AGENT" push --quiet origin feature
  fi
  return "$STUB_RC"
}

REPO_ABS="$WORK"
WT_ROOT="$TMP_ROOT/wt"
LOG_DIR="$TMP_ROOT/logs"
mkdir -p "$WT_ROOT" "$LOG_DIR"
CREATED_WORKTREES=()
DEFAULT_FEEDBACK_MAX_TURNS=50
DEFAULT_FEEDBACK_MAX_BUDGET=2

run_case() {
  STUB_RC="$1"; STUB_PUSHES="$2"
  FEEDBACK_RC=""; FEEDBACK_ADVANCED=""
  run_feedback_once 1 feature "$ORIGIN" 99 "findings" 1 >/dev/null 2>&1
}

run_case 0 false
assert_eq "clean agent records rc=0"            "0"     "$FEEDBACK_RC"
assert_eq "clean agent that pushed nothing sees no advance" \
  "false" "$FEEDBACK_ADVANCED"

run_case 1 false
assert_eq "failed agent records its exit code"  "1"     "$FEEDBACK_RC"
assert_eq "failed agent that pushed nothing sees no advance" \
  "false" "$FEEDBACK_ADVANCED"
assert_eq "that combination is the stall case"  "yes"   "$(stalled "$FEEDBACK_RC" "$FEEDBACK_ADVANCED")"

run_case 1 true
assert_eq "failed agent that pushed records its exit code" "1" "$FEEDBACK_RC"
assert_eq "failed agent that pushed sees the advance"      "true" "$FEEDBACK_ADVANCED"
assert_eq "that combination is not a stall"     "no"    "$(stalled "$FEEDBACK_RC" "$FEEDBACK_ADVANCED")"

run_case 0 true
assert_eq "clean agent that pushed records rc=0" "0"    "$FEEDBACK_RC"
assert_eq "clean agent that pushed sees the advance" "true" "$FEEDBACK_ADVANCED"

# ── add_pr_label ─────────────────────────────────────────────────────────────
echo "── add_pr_label ──"

# `gh pr edit --add-label` resolves the PR through GraphQL and fails outright
# where the Projects-classic deprecation applies — exit 1, no label. The helper
# must go through REST instead, and must never take a run down with it.
GH_ARGS="$TMP_ROOT/gh-args"
GH_STUB_RC=0
gh() { printf '%s\n' "$*" >> "$GH_ARGS"; return "$GH_STUB_RC"; }
EVENTS="$TMP_ROOT/events"
log_event() { printf '%s\n' "$*" >> "$EVENTS"; }

: > "$GH_ARGS"; : > "$EVENTS"
add_pr_label "$WORK" 99 needs-human
recorded=$(cat "$GH_ARGS")

if grep -q "issues/99/labels" <<<"$recorded"; then
  pass "labels through the REST issues endpoint"
else
  fail "labels through the REST issues endpoint (called: $recorded)"
fi
if grep -q "pr edit" <<<"$recorded"; then
  fail "does not use the GraphQL-backed pr edit path (called: $recorded)"
else
  pass "does not use the GraphQL-backed pr edit path"
fi
if grep -q "labels\[\]=needs-human" <<<"$recorded"; then
  pass "passes the label in the REST array form"
else
  fail "passes the label in the REST array form (called: $recorded)"
fi
assert_eq "says nothing when the call succeeds" "" "$(cat "$EVENTS")"

: > "$GH_ARGS"; : > "$EVENTS"
GH_STUB_RC=1
set +e
add_pr_label "$WORK" 99 needs-human
label_rc=$?
set -e
assert_eq "a failed label never fails the caller" "0" "$label_rc"
if grep -q "Could not label PR #99" "$EVENTS"; then
  pass "a failed label is logged rather than swallowed"
else
  fail "a failed label is logged rather than swallowed"
fi

echo
echo "── $PASS passed, $FAIL failed ──"
[ "$FAIL" -eq 0 ]
