#!/bin/bash
# The entrypoint refuses to start an agent in an empty checkout, with the exit
# code the harness maps to no_worktree, and otherwise points git at a
# credential store beside the git config before starting the agent.
set -euo pipefail

exec </dev/null

SCRIPT_DIR="$(cd -P "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$SCRIPT_DIR/lib.sh"
ENTRYPOINT="$SCRIPT_DIR/../entrypoint.sh"
NO_WORKTREE_RC=78

TEST_DIR=$(mktemp -d /tmp/alucard_test_entrypoint.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT

# ── The entrypoint refuses an empty checkout ────────────────────────────────
echo "── entrypoint: empty checkout ──"

EMPTY="$TEST_DIR/empty"; mkdir -p "$EMPTY"
set +e
err=$(cd "$EMPTY" && bash "$ENTRYPOINT" touch "$TEST_DIR/agent-ran" 2>&1)
rc=$?
set -e
assert_eq "an empty checkout stops the container with the harness's no-worktree code" \
  "$NO_WORKTREE_RC" "$rc"
assert_contains "and says the worktree is missing" "worktree is missing" "$err"
[ -e "$TEST_DIR/agent-ran" ] && fail "the agent command never runs" || pass "the agent command never runs"

# ── The entrypoint starts the agent in a real checkout ──────────────────────
# Run from a subdirectory, as the preflight container is (-w /work/<dir>).
# Every path the entrypoint writes is redirected, so this leaves nothing in
# /tmp or the caller's home.
echo ""
echo "── entrypoint: real checkout ──"

CHECKOUT="$TEST_DIR/checkout"
mkdir -p "$CHECKOUT/sub"
git -C "$CHECKOUT" init -q -b main
git -C "$CHECKOUT" config user.email test@example.invalid
git -C "$CHECKOUT" config user.name test
printf 'seed\n' > "$CHECKOUT/README.md"
git -C "$CHECKOUT" add .
git -C "$CHECKOUT" commit -qm initial
CFG="$TEST_DIR/cfg"
set +e
(
  cd "$CHECKOUT/sub" && \
  HOME="$TEST_DIR/home" GIT_CONFIG_GLOBAL="$CFG/gitconfig" GH_CONFIG_DIR="$CFG/gh" \
  GITHUB_TOKEN=test-token bash "$ENTRYPOINT" touch "$TEST_DIR/agent-ran-ok"
) > "$TEST_DIR/entry.out" 2>&1
rc=$?
set -e
assert_eq "a checkout lets the agent start" "0" "$rc"
[ -e "$TEST_DIR/agent-ran-ok" ] && pass "and the agent command runs" \
  || fail "and the agent command runs ($(<"$TEST_DIR/entry.out"))"
if [ -f "$CFG/git-credentials" ]; then
  pass "the credential store sits beside the git config, not at a fixed /tmp path"
  assert_contains "and carries the token" "x-access-token:test-token@github.com" "$(<"$CFG/git-credentials")"
  assert_contains "and git is pointed at it" "$CFG/git-credentials" "$(<"$CFG/gitconfig")"
else
  fail "the credential store sits beside the git config, not at a fixed /tmp path"
fi

finish
