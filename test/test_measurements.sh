#!/bin/bash
# Covers the stage-measurement layer: bucket classification (including the
# binary and rename cases that would otherwise inflate line counts), the
# stage record's base-pinned accounting, and the iteration record's per-role
# token roll-up with codex's absent cost distinguished from a real zero.
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

# shellcheck disable=SC1090
source "$ALUCARD"

TMP_ROOT=$(mktemp -d /tmp/alucard_test_measurements.XXXXXX)
trap 'rm -rf "$TMP_ROOT"' EXIT

# ── Test group 1: numstat classification ─────────────────────────────────────
echo "── measure_classify buckets ──"

CLS=$(printf '%s\n' \
  "10	2	backend/src/app/service.py" \
  "40	1	backend/tests/unit/test_service.py" \
  "5	0	docs/spec/thing.md" \
  "900	3	package-lock.json" \
  | measure_classify)

assert_eq "impl bucket"      "10" "$(echo "$CLS" | jq -r '.impl.added')"
assert_eq "tests bucket"     "40" "$(echo "$CLS" | jq -r '.tests.added')"
assert_eq "confdoc bucket"   "5"  "$(echo "$CLS" | jq -r '.confdoc.added')"
assert_eq "generated bucket" "900" "$(echo "$CLS" | jq -r '.generated.added')"
assert_eq "total added"      "955" "$(echo "$CLS" | jq -r '.added')"
assert_eq "file count"       "4"  "$(echo "$CLS" | jq -r '.files')"
assert_eq "paths retained"   "backend/src/app/service.py" \
  "$(echo "$CLS" | jq -r '.paths[0]')"

# A .test.tsx file outside any tests/ directory still counts as a test.
assert_eq "colocated .test.tsx is a test" "7" \
  "$(printf '7\t0\tsrc/app/Card.test.tsx\n' | measure_classify | jq -r '.tests.added')"

# ── Test group 2: binary files never become line counts ──────────────────────
echo "── binary and rename handling ──"

BIN=$(printf '%s\n' \
  "-	-	docs/verification/pr150/feed-320.png" \
  "3	1	src/app.py" \
  | measure_classify)

assert_eq "binary counted as a file" "1" "$(echo "$BIN" | jq -r '.binary_files')"
assert_eq "binary adds no lines"     "3" "$(echo "$BIN" | jq -r '.added')"
assert_eq "binary bucketed by path"  "1" "$(echo "$BIN" | jq -r '.confdoc.binary_files')"

# measure_classify consumes `--numstat -z` with NULs turned into newlines: a
# rename is a header with an empty third field, then the old path, then the new.
REN=$(printf '4\t2\t\nsrc/old/name.py\nsrc/new/name.py\n' | measure_classify)
assert_eq "rename counted"            "1" "$(echo "$REN" | jq -r '.renames')"
assert_eq "rename attributed to dest" "src/new/name.py" "$(echo "$REN" | jq -r '.paths[0]')"
assert_eq "rename lines counted once" "4" "$(echo "$REN" | jq -r '.added')"

# git's display notation factors the shared prefix out as src/{old => new}/name.py.
# Parsing that instead of the -z form drops the prefix, which corrupts the
# retained path and can flip the bucket — a file moved between two directories
# under tests/ would be booked as implementation.
MOVED=$(printf '1\t1\t\nbackend/tests/unit/helper.py\nbackend/tests/integration/helper.py\n' \
  | measure_classify)
assert_eq "moved test keeps its full path" "backend/tests/integration/helper.py" \
  "$(echo "$MOVED" | jq -r '.paths[0]')"
assert_eq "moved test stays in the tests bucket" "1" \
  "$(echo "$MOVED" | jq -r '.tests.added')"
assert_eq "moved test is not booked as impl" "0" \
  "$(echo "$MOVED" | jq -r '.impl.added')"

# ── Test group 3: stage records are pinned to the base, not chained ──────────
echo "── measure_stage accounting ──"

REPO="$TMP_ROOT/repo"
mkdir -p "$REPO"
git -C "$REPO" init -q
git -C "$REPO" config user.email t@example.com
git -C "$REPO" config user.name Test
mkdir -p "$REPO/src" "$REPO/tests"
echo "base" > "$REPO/src/app.py"
git -C "$REPO" add -A && git -C "$REPO" commit -qm base
BASE_SHA=$(git -C "$REPO" rev-parse HEAD)

printf 'a\nb\nc\n' >> "$REPO/src/app.py"
printf 'x\ny\n' > "$REPO/tests/test_app.py"
git -C "$REPO" add -A && git -C "$REPO" commit -qm worker
WORKER_SHA=$(git -C "$REPO" rev-parse HEAD)

# A later stage that reverts the earlier one: chaining deltas would report
# growth where the base-pinned view correctly reports none.
git -C "$REPO" revert --no-edit HEAD >/dev/null
git -C "$REPO" log -1 --format=%H > /dev/null

LOG_DIR="$TMP_ROOT/logs"
mkdir -p "$LOG_DIR"
REPO_ABS="$REPO"
BASE_BRANCH="main"
MEASURE_PREV_SHA=""

measure_stage 1 "42" worker 0 "$REPO" "$BASE_SHA" "$BASE_SHA" ""

REC=$(head -1 "$LOG_DIR/measurements.jsonl")
assert_eq "stage record type"   "stage" "$(echo "$REC" | jq -r '.record')"
assert_eq "format stamped"      "1"     "$(echo "$REC" | jq -r '.format')"
assert_eq "pr recorded"         "42"    "$(echo "$REC" | jq -r '.pr')"
assert_eq "base sha recorded"   "$BASE_SHA" "$(echo "$REC" | jq -r '.base_sha')"
# HEAD is now the revert commit, so base-pinned impl growth is zero even
# though the intermediate worker stage added three lines.
assert_eq "revert cancels against base" "0" "$(echo "$REC" | jq -r '.from_base.impl.added')"
assert_eq "prev sha advances" "$(git -C "$REPO" rev-parse HEAD)" "$MEASURE_PREV_SHA"

# The same measurement taken at the worker head does see the growth.
rm -f "$LOG_DIR/measurements.jsonl"
git -C "$REPO" checkout -q "$WORKER_SHA"
measure_stage 1 "42" worker 0 "$REPO" "$BASE_SHA" "$BASE_SHA" ""
REC=$(head -1 "$LOG_DIR/measurements.jsonl")
assert_eq "worker head impl growth"  "3" "$(echo "$REC" | jq -r '.from_base.impl.added')"
assert_eq "worker head tests growth" "2" "$(echo "$REC" | jq -r '.from_base.tests.added')"

# A second stage whose previous head is the worker, not the base. from_base
# must report cumulative growth while from_prev reports only this stage's
# increment — the distinction the whole record exists to preserve.
rm -f "$LOG_DIR/measurements.jsonl"
printf 'd\ne\n' >> "$REPO/src/app.py"
printf 'z\n' >> "$REPO/tests/test_app.py"
git -C "$REPO" add -A && git -C "$REPO" commit -qm feedback
MEASURE_PREV_SHA=""
measure_stage 1 "42" feedback 1 "$REPO" "$BASE_SHA" "$WORKER_SHA" ""
REC=$(head -1 "$LOG_DIR/measurements.jsonl")
assert_eq "from_base is cumulative"      "5" "$(echo "$REC" | jq -r '.from_base.impl.added')"
assert_eq "from_prev is this stage only" "2" "$(echo "$REC" | jq -r '.from_prev.impl.added')"
assert_eq "from_base tests cumulative"   "3" "$(echo "$REC" | jq -r '.from_base.tests.added')"
assert_eq "from_prev tests increment"    "1" "$(echo "$REC" | jq -r '.from_prev.tests.added')"

# ── Test group 3b: end-to-end through git, not just the parser ──────────────
echo "── measure_diff_json against real git output ──"

RREPO="$TMP_ROOT/renames"
mkdir -p "$RREPO/backend/tests/unit"
git -C "$RREPO" init -q
git -C "$RREPO" config user.email t@example.com
git -C "$RREPO" config user.name Test
printf 'a\nb\nc\nd\ne\nf\ng\nh\n' > "$RREPO/backend/tests/unit/helper.py"
git -C "$RREPO" add -A && git -C "$RREPO" commit -qm base
RBASE=$(git -C "$RREPO" rev-parse HEAD)
mkdir -p "$RREPO/backend/tests/integration"
git -C "$RREPO" mv backend/tests/unit/helper.py backend/tests/integration/helper.py
printf 'i\n' >> "$RREPO/backend/tests/integration/helper.py"
git -C "$RREPO" add -A && git -C "$RREPO" commit -qm move
RHEAD=$(git -C "$RREPO" rev-parse HEAD)

# `git diff --numstat --find-renames` prints this as
# backend/tests/{unit => integration}/helper.py.
RJ=$(measure_diff_json "$RREPO" "$RBASE" "$RHEAD")
assert_eq "real rename counted"        "1" "$(echo "$RJ" | jq -r '.renames')"
assert_eq "real rename keeps its path" "backend/tests/integration/helper.py" \
  "$(echo "$RJ" | jq -r '.paths[0]')"
assert_eq "real rename stays a test"   "1" "$(echo "$RJ" | jq -r '.tests.added')"

# ── Test group 3c: an unavailable diff says so ──────────────────────────────
echo "── unresolvable diffs are reported, not zeroed ──"

BADJ=$(measure_diff_json "$RREPO" "definitely-not-a-ref" "$RHEAD")
assert_eq "bad ref marked unavailable" "true" "$(echo "$BADJ" | jq -r '.unavailable')"
# The failure must not arrive as a credible zero-growth diff.
assert_eq "bad ref reports no file count" "null" "$(echo "$BADJ" | jq -r '.files')"
assert_eq "bad ref reports no added count" "null" "$(echo "$BADJ" | jq -r '.added')"

EMPTYJ=$(measure_diff_json "$RREPO" "" "$RHEAD")
assert_eq "empty ref marked unavailable" "true" "$(echo "$EMPTYJ" | jq -r '.unavailable')"
assert_eq "empty ref names the reason"   "missing ref" "$(echo "$EMPTYJ" | jq -r '.reason')"

# A genuinely empty diff is still reported as a real measurement of zero.
SAMEJ=$(measure_diff_json "$RREPO" "$RHEAD" "$RHEAD")
assert_eq "identical refs are available" "null" "$(echo "$SAMEJ" | jq -r '.unavailable')"
assert_eq "identical refs count zero files" "0" "$(echo "$SAMEJ" | jq -r '.files')"

# The stage record carries the marker through rather than hiding it.
rm -f "$LOG_DIR/measurements.jsonl"
MEASURE_PREV_SHA=""
measure_stage 1 "42" worker 0 "$RREPO" "no-such-base" "" "" >/dev/null
REC=$(head -1 "$LOG_DIR/measurements.jsonl")
assert_eq "stage surfaces an unavailable base diff" "true" \
  "$(echo "$REC" | jq -r '.from_base.unavailable')"

# ── Test group 4: a measurement failure never fails the run ─────────────────
echo "── failure isolation ──"

rm -f "$LOG_DIR/measurements.jsonl"
set +e
measure_stage 1 "42" worker 0 "$TMP_ROOT/definitely-not-a-repo" "$BASE_SHA" "" ""
RC=$?
set -e
assert_eq "missing repo dir returns 0" "0" "$RC"

set +e
( LOG_DIR="$TMP_ROOT/not-a-dir" measure_emit '{"x":1}' )
RC=$?
set -e
assert_eq "unset log dir returns 0" "0" "$RC"

# ci_gate re-enables `set -e` immediately before recording a stage, and
# pipefail turns an unresolvable base ref into a failing pipeline. Under those
# exact conditions a measurement must not abort the caller.
#
# This runs in a real child process rather than a subshell: bash suppresses
# `set -e` inside any compound command that is part of a `||` list, so
# `( set -e; ... ) || rc=$?` would pass even when the code is broken.
cat > "$TMP_ROOT/under_set_e.sh" <<SETE
set -euo pipefail
source "$ALUCARD"
LOG_DIR="$TMP_ROOT/logs"
REPO_ABS="$REPO"
measure_stage 1 "42" cifix 1 "$REPO" "" "" ""
echo reached-the-end > "$TMP_ROOT/after-set-e"
SETE
rm -f "$TMP_ROOT/after-set-e"
set +e
bash "$TMP_ROOT/under_set_e.sh" >/dev/null 2>&1
RC=$?
set -e
assert_eq "set -e + empty base sha does not abort" "0" "$RC"
assert_eq "caller continues past the measurement" "reached-the-end" \
  "$(cat "$TMP_ROOT/after-set-e" 2>/dev/null)"

# ── Test group 5: iteration roll-up, cost known vs unknown ──────────────────
echo "── measure_iteration roll-up ──"

rm -f "$LOG_DIR/measurements.jsonl"
cat > "$LOG_DIR/iter-7.jsonl" <<'EOF'
{"type":"result","num_turns":10,"total_cost_usd":2.5,"modelUsage":{"m":{"inputTokens":100,"outputTokens":200,"cacheReadInputTokens":300,"cacheCreationInputTokens":40}}}
EOF
cat > "$LOG_DIR/iter-7-review-1.jsonl" <<'EOF'
{"type":"turn.completed","usage":{"input_tokens":500,"cached_input_tokens":200,"output_tokens":50}}
{"type":"item.completed","item":{"type":"command_execution"}}
EOF
cat > "$LOG_DIR/iter-7-review-2.jsonl" <<'EOF'
{"type":"turn.completed","usage":{"input_tokens":700,"cached_input_tokens":100,"output_tokens":70}}
{"type":"item.completed","item":{"type":"command_execution"}}
EOF

measure_iteration 7 "42" 0 900 green APPROVED 2 "$BASE_SHA"
REC=$(grep '"record":"iteration"' "$LOG_DIR/measurements.jsonl" | tail -1)

assert_eq "worker invocations"   "1" "$(echo "$REC" | jq -r '.roles.worker.invocations')"
assert_eq "review invocations"   "2" "$(echo "$REC" | jq -r '.roles.review.invocations')"
assert_eq "review input summed"  "900" "$(echo "$REC" | jq -r '.roles.review.input')"
assert_eq "worker cost recorded" "2.5" "$(echo "$REC" | jq -r '.roles.worker.cost')"
# codex reports no cost: it must stay null, never collapse to 0.
assert_eq "review cost unknown"  "null" "$(echo "$REC" | jq -r '.roles.review.cost')"
assert_eq "review cycles"        "2" "$(echo "$REC" | jq -r '.review_cycles')"
assert_eq "verdict recorded"     "APPROVED" "$(echo "$REC" | jq -r '.review_verdict')"
assert_eq "ci result recorded"   "green" "$(echo "$REC" | jq -r '.ci_result')"

# Completeness must mean every invocation reported a cost. This fixture mixes a
# claude worker that reports $2.50 with two codex reviewers that report none, so
# the total is a lower bound and must not be presented as the whole bill.
assert_eq "partial cost is not complete" "false" "$(echo "$REC" | jq -r '.cost_complete')"
assert_eq "costed invocations counted"   "1" "$(echo "$REC" | jq -r '.costed_invocations')"
assert_eq "total invocations counted"    "3" "$(echo "$REC" | jq -r '.total_invocations')"

# All-claude: every invocation reports a cost, so the total really is complete.
ALL_DIR="$TMP_ROOT/logs_all_claude"
mkdir -p "$ALL_DIR"
for n in 8 8-review-1; do
  cat > "$ALL_DIR/iter-$n.jsonl" <<'EOF'
{"type":"result","num_turns":3,"total_cost_usd":1.5,"modelUsage":{"m":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":30,"cacheCreationInputTokens":4}}}
EOF
done
( LOG_DIR="$ALL_DIR" measure_iteration 8 "43" 0 10 green APPROVED 1 "$BASE_SHA" )
REC_ALL=$(grep '"record":"iteration"' "$ALL_DIR/measurements.jsonl" | tail -1)
assert_eq "all-costed run is complete" "true" "$(echo "$REC_ALL" | jq -r '.cost_complete')"

# No invocations at all is not completeness either.
EMPTY_DIR="$TMP_ROOT/logs_empty"
mkdir -p "$EMPTY_DIR"
( LOG_DIR="$EMPTY_DIR" measure_iteration 9 "44" 0 10 green APPROVED 1 "$BASE_SHA" )
REC_EMPTY=$(grep '"record":"iteration"' "$EMPTY_DIR/measurements.jsonl" | tail -1)
assert_eq "no invocations is not complete" "false" "$(echo "$REC_EMPTY" | jq -r '.cost_complete')"

# An invocation whose log carries no parseable usage — a worker killed by a
# timeout or a transport drop, which is exactly the case most likely to lack a
# final usage record — must still be counted. Skipping it made the run look
# complete while describing only the attempts that survived.
NOUSE_DIR="$TMP_ROOT/logs_no_usage"
mkdir -p "$NOUSE_DIR"
cat > "$NOUSE_DIR/iter-10.jsonl" <<'EOF'
{"type":"result","num_turns":3,"total_cost_usd":2.5,"modelUsage":{"m":{"inputTokens":10,"outputTokens":20,"cacheReadInputTokens":30,"cacheCreationInputTokens":4}}}
EOF
cat > "$NOUSE_DIR/iter-10-review-1.jsonl" <<'EOF'
{"type":"system","subtype":"init"}
{"type":"assistant","message":{"content":[{"type":"text","text":"killed before any usage record"}]}}
EOF
( LOG_DIR="$NOUSE_DIR" measure_iteration 10 "45" 1 60 "" "" 0 "$BASE_SHA" )
REC_NU=$(grep '"record":"iteration"' "$NOUSE_DIR/measurements.jsonl" | tail -1)

assert_eq "an unparseable log still counts as an invocation" "2" \
  "$(echo "$REC_NU" | jq -r '.total_invocations')"
assert_eq "it does not count as costed" "1" \
  "$(echo "$REC_NU" | jq -r '.costed_invocations')"
assert_eq "missing usage makes the run incomplete" "false" \
  "$(echo "$REC_NU" | jq -r '.cost_complete')"
assert_eq "the role keeps the invocation" "1" \
  "$(echo "$REC_NU" | jq -r '.roles.review.invocations')"
assert_eq "the role reports the missing usage" "1" \
  "$(echo "$REC_NU" | jq -r '.roles.review.usage_missing')"
assert_eq "no phantom tokens are added for it" "0" \
  "$(echo "$REC_NU" | jq -r '.roles.review.input')"

# ── Test group 5b: baseline lookup is scoped to one repository ──────────────
echo "── prior-base lookup ──"

LOG_ROOT="$TMP_ROOT/logroot"
mkdir -p "$LOG_ROOT/alucard-20260101-000000" "$LOG_ROOT/alucard-20260102-000000"

# PR numbers are repository-local, and every repository's runs share $LOG_ROOT.
cat > "$LOG_ROOT/alucard-20260101-000000/measurements.jsonl" <<'EOF'
{"format":1,"record":"stage","repo_id":"aldovc/family-brain","pr":"43","stage":"worker","base_sha":"aaaaaaaaaaaa","baseline_source":"pinned"}
EOF
cat > "$LOG_ROOT/alucard-20260102-000000/measurements.jsonl" <<'EOF'
{"format":1,"record":"stage","repo_id":"aldovc/zodiac","pr":"43","stage":"worker","base_sha":"bbbbbbbbbbbb","baseline_source":"merge-base"}
EOF

assert_eq "lookup finds its own repository's base" "aaaaaaaaaaaa" \
  "$(measure_prior_base_for_pr "aldovc/family-brain" 43 | cut -f1)"
assert_eq "lookup does not cross repositories" "bbbbbbbbbbbb" \
  "$(measure_prior_base_for_pr "aldovc/zodiac" 43 | cut -f1)"
assert_eq "an unknown repository finds nothing" "" \
  "$(measure_prior_base_for_pr "aldovc/home-cluster" 43)"

# Provenance survives the round trip: an approximated base must not be
# promoted to exact just because a later run read it out of a file.
assert_eq "pinned provenance is preserved"     "pinned" \
  "$(measure_prior_base_for_pr "aldovc/family-brain" 43 | cut -f2)"
assert_eq "merge-base provenance is preserved" "merge-base" \
  "$(measure_prior_base_for_pr "aldovc/zodiac" 43 | cut -f2)"

# Records written before repo_id existed are ignored, not matched on PR alone.
mkdir -p "$LOG_ROOT/alucard-20251231-000000"
cat > "$LOG_ROOT/alucard-20251231-000000/measurements.jsonl" <<'EOF'
{"format":1,"record":"stage","pr":"99","stage":"worker","base_sha":"cccccccccccc"}
EOF
assert_eq "records without a repo id are skipped" "" \
  "$(measure_prior_base_for_pr "aldovc/family-brain" 99)"

# ── Test group 5c: repository identity is stable across run modes ───────────
echo "── measure_repo_id ──"

IDREPO="$TMP_ROOT/idrepo"
mkdir -p "$IDREPO"
git -C "$IDREPO" init -q
git -C "$IDREPO" remote add origin "https://github.com/aldovc/zodiac.git"
assert_eq "https remote yields owner/name" "aldovc/zodiac" "$(measure_repo_id "$IDREPO")"
git -C "$IDREPO" remote set-url origin "git@github.com:aldovc/zodiac.git"
assert_eq "ssh remote yields the same id"  "aldovc/zodiac" "$(measure_repo_id "$IDREPO")"
# The same repository is reached through a cached clone in some run modes; the
# id must not change with the path.
CACHED="$TMP_ROOT/cache/aldovc/zodiac"
mkdir -p "$CACHED"
git -C "$CACHED" init -q
git -C "$CACHED" remote add origin "https://github.com/aldovc/zodiac.git"
assert_eq "a cached clone has the same id"  "aldovc/zodiac" "$(measure_repo_id "$CACHED")"

NOREMOTE="$TMP_ROOT/noremote"
mkdir -p "$NOREMOTE"
git -C "$NOREMOTE" init -q
assert_eq "no remote falls back to the directory name" "noremote" \
  "$(measure_repo_id "$NOREMOTE")"

# ── Test group 6: prompt archiving ──────────────────────────────────────────
echo "── prompt archive ──"

measure_archive_prompt "iter-7" "the dispatched prompt"
assert_eq "prompt archived verbatim" "the dispatched prompt" \
  "$(cat "$LOG_DIR/prompts/iter-7.txt")"

echo ""
echo "Passed: $PASS  Failed: $FAIL"
[ "$FAIL" -eq 0 ]
