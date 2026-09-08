#!/bin/bash
# Measurement records written by the gates, driven end-to-end through
# `alucard continue` (which reaches the review gate without running a worker).
#
# Covers two ways the records went blank in review: an ordinary *formal*
# approval, where the reviewer's decision file does not override GitHub, and a
# continued run's identity and baseline, which must not be re-derived from the
# base branch as it stands now.
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

TEST_DIR=$(mktemp -d /tmp/alucard_test_measure_gates.XXXXXX)
trap 'rm -rf "$TEST_DIR"' EXIT
MOCK_BIN="$TEST_DIR/bin"
TARGET="$TEST_DIR/target"
REMOTE="$TEST_DIR/remote.git"
mkdir -p "$MOCK_BIN"
cp "$SCRIPT_DIR/fixtures/credentials.env" "$TEST_DIR/alucard.env"

BRANCH="feat/under-review"
git init -q --bare "$REMOTE"
mkdir -p "$TARGET"
git -C "$TARGET" init -q -b main
git -C "$TARGET" config user.email test@example.invalid
git -C "$TARGET" config user.name test
printf '# Under review\n' > "$TARGET/README.md"
git -C "$TARGET" add .
git -C "$TARGET" commit -qm initial
git -C "$TARGET" remote add origin "$REMOTE"
git -C "$TARGET" push -qu origin main
FORK_POINT=$(git -C "$TARGET" rev-parse HEAD)
git -C "$TARGET" checkout -q -b "$BRANCH"
printf 'change\n' >> "$TARGET/README.md"
git -C "$TARGET" commit -qam change
git -C "$TARGET" push -q origin "$BRANCH"
PR_HEAD=$(git -C "$TARGET" rev-parse HEAD)
git -C "$TARGET" checkout -q main

# The base branch advances after the PR forked, exactly as it does when other
# PRs merge overnight. A continued run that re-derives its base from the base
# branch would compare this PR against a commit it never branched from.
printf 'unrelated work from another PR\n' >> "$TARGET/README.md"
git -C "$TARGET" commit -qam unrelated
git -C "$TARGET" push -q origin main
ADVANCED_BASE=$(git -C "$TARGET" rev-parse HEAD)

cat > "$MOCK_BIN/docker" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "${1:-}" in
  image|rm|kill) exit 0 ;;
  run) ;;
  *) exit 0 ;;
esac
printf '{"type":"result","subtype":"success","is_error":false,"result":"reviewed"}\n'
exit 0
MOCK

cat > "$MOCK_BIN/timeout" <<'MOCK'
#!/bin/bash
set -euo pipefail
if [ "$1" = "--kill-after" ]; then shift 2; fi
shift
exec "$@"
MOCK

# A *formal* GitHub approval with no reviewer decision file: resolve_review_verdict
# returns the same state GitHub already reports, so the decision-file override
# branch never fires.
cat > "$MOCK_BIN/gh" <<'MOCK'
#!/bin/bash
set -euo pipefail
case "$1 $2" in
  "pr checks")
    echo "no checks reported" >&2
    exit 1 ;;
  "pr list") printf '77\n' ;;
  "pr view")
    case "$*" in
      *state,headRefName*) printf '{"state":"OPEN","headRefName":"%s"}\n' "$ALUCARD_TEST_BRANCH" ;;
      *comments*)          printf '{"comments":[]}\n' ;;
      *reviews*)           printf 'APPROVED\n' ;;
      *headRefOid*)        printf '%s\n' "$ALUCARD_TEST_PR_HEAD" ;;
      *baseRefOid*)        printf '%s\n' "$ALUCARD_TEST_ADVANCED_BASE" ;;
      *)                   printf '{}\n' ;;
    esac ;;
  *) : ;;
esac
MOCK
chmod +x "$MOCK_BIN/docker" "$MOCK_BIN/timeout" "$MOCK_BIN/gh"

run_continue() {
  rm -rf "$TEST_DIR/logs"
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_BRANCH="$BRANCH" \
  ALUCARD_TEST_PR_HEAD="$PR_HEAD" \
  ALUCARD_TEST_ADVANCED_BASE="$ADVANCED_BASE" \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 1 \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" \
      > "$TEST_DIR/out" 2>&1
  set -e
  MEAS=$(cat "$TEST_DIR/logs"/alucard-*/measurements.jsonl 2>/dev/null || true)
}

run_continue

# ── A formal approval is recorded ───────────────────────────────────────────
echo "── formal review outcome ──"

ITER_REC=$(printf '%s\n' "$MEAS" | jq -c 'select(.record == "iteration")' | tail -1)
assert_eq "an iteration record is written" "iteration" \
  "$(printf '%s' "$ITER_REC" | jq -r '.record // "MISSING"')"
assert_eq "a formal approval is recorded as the verdict" "APPROVED" \
  "$(printf '%s' "$ITER_REC" | jq -r '.review_verdict')"
assert_eq "the cycle it settled on is recorded" "1" \
  "$(printf '%s' "$ITER_REC" | jq -r '.review_cycles')"

# ── A continued run carries the run identity ────────────────────────────────
echo "── continued-run identity ──"

RUN_REC=$(printf '%s\n' "$MEAS" | jq -c 'select(.record == "run")' | head -1)
assert_eq "a continued run writes a run record" "run" \
  "$(printf '%s' "$RUN_REC" | jq -r '.record // "MISSING"')"
assert_eq "the run record names the mode" "continue" \
  "$(printf '%s' "$RUN_REC" | jq -r '.task_source')"
assert_eq "the image is identified" "true" \
  "$(printf '%s' "$RUN_REC" | jq -r '(.image | length > 0)')"
assert_eq "the prompt digest is recorded" "true" \
  "$(printf '%s' "$RUN_REC" | jq -r '(.prompt_digest | length > 0)')"
assert_eq "per-role settings are recorded" "true" \
  "$(printf '%s' "$RUN_REC" | jq -r '(.roles | has("REVIEWER"))')"

# ── The baseline is the fork point, not the advanced base branch ────────────
echo "── continued-run baseline ──"

BASE_USED=$(printf '%s' "$ITER_REC" | jq -r '.base_sha')
assert_eq "the baseline is the PR's fork point" "$FORK_POINT" "$BASE_USED"
assert_eq "the baseline is not the advanced base branch" "false" \
  "$([ "$BASE_USED" = "$ADVANCED_BASE" ] && echo true || echo false)"
assert_eq "the baseline's provenance is labelled" "merge-base" \
  "$(printf '%s' "$ITER_REC" | jq -r '.baseline_source')"

# ── A prior run's base is reused, but only from the same repository ─────────
echo "── prior-run baseline, scoped by repository ──"

REPO_ID=$(bash -c 'source "$1"; measure_repo_id "$2"' _ "$ALUCARD" "$TARGET")

# Another repository's run, same PR number. PR numbers are repository-local, so
# this must not be picked up — it would silently measure this PR against a base
# from a completely different codebase.
seed_prior() {  # $1 repo_id, $2 base_sha, $3 baseline_source
  rm -rf "$TEST_DIR/logs"
  mkdir -p "$TEST_DIR/logs/alucard-20260101-000000"
  printf '{"format":1,"record":"stage","repo_id":"%s","pr":"77","stage":"worker","base_sha":"%s","baseline_source":"%s"}\n' \
    "$1" "$2" "$3" > "$TEST_DIR/logs/alucard-20260101-000000/measurements.jsonl"
}

run_continue_keeping_logs() {
  set +e
  PATH="$MOCK_BIN:$PATH" \
  ALUCARD_TEST_BRANCH="$BRANCH" \
  ALUCARD_TEST_PR_HEAD="$PR_HEAD" \
  ALUCARD_TEST_ADVANCED_BASE="$ADVANCED_BASE" \
    "$ALUCARD" continue 77 "$TARGET" --no-build --max-review-cycles 1 \
      --env-file "$TEST_DIR/alucard.env" --logs-root "$TEST_DIR/logs" \
      > "$TEST_DIR/out" 2>&1
  set -e
  MEAS=$(cat "$TEST_DIR/logs"/alucard-*/measurements.jsonl 2>/dev/null | jq -c 'select(.record=="iteration")' | tail -1)
}

seed_prior "someone/other-repo" "ffffffffffff" "pinned"
run_continue_keeping_logs
assert_eq "another repository's PR 77 is not reused" "false" \
  "$([ "$(printf '%s' "$MEAS" | jq -r '.base_sha')" = "ffffffffffff" ] && echo true || echo false)"
assert_eq "it falls back to this PR's fork point" "$FORK_POINT" \
  "$(printf '%s' "$MEAS" | jq -r '.base_sha')"

seed_prior "$REPO_ID" "ffffffffffff" "pinned"
run_continue_keeping_logs
assert_eq "this repository's pinned base is reused" "ffffffffffff" \
  "$(printf '%s' "$MEAS" | jq -r '.base_sha')"
assert_eq "a reused pinned base is labelled prior-run" "prior-run" \
  "$(printf '%s' "$MEAS" | jq -r '.baseline_source')"

# An approximated base does not become exact by being read back out of a file.
seed_prior "$REPO_ID" "ffffffffffff" "merge-base"
run_continue_keeping_logs
assert_eq "a reused approximate base keeps its provenance" "merge-base" \
  "$(printf '%s' "$MEAS" | jq -r '.baseline_source')"

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
