# Pilot runbook: reviewer class-sweep (arm 1)

Status: ready to run, not yet run. Date: 2026-09-08.
Procedure for the first paired comparison in `lean-agent-loop.md`. Written to
be followed literally, because the arms are only comparable if the setup is.

## What is being compared

One variable: the **Sweep a finding's class before you report it** section in
`alucard-reviewer-prompt.md`. Nothing else differs.

- **Sweep arm** — whatever branch carries the change under test; below this is
  `feat/lean-loop-step-2`.
- **Control arm** — a branch cut from the sweep arm's exact HEAD at pilot time,
  with only `alucard-reviewer-prompt.md` reverted to its pre-sweep state.

The control arm is *generated from the sweep HEAD when the pilot starts*, not
maintained as a long-lived branch: any further commit to the sweep branch would
otherwise make the two diverge in files that have nothing to do with the
experiment. Generating it also keeps both checkouts clean and committed —
reverting a prompt in a working tree instead would leave staged changes, set
`alucard_dirty` in the run record, and make the recorded prompt digest describe
a tree that is in no commit.

The shared policy fragment is *not* in this round. It is arm 2, and the spec's
delivery order puts it after this one specifically so the two are separately
attributable.

## Design: one worker, two reviewers

**Both arms must review byte-identical code.** This is a reviewer experiment,
and letting each arm run its own worker would break its central criterion: two
workers given the same task produce different implementations containing
different defects, so "did both reviewers find the same findings" would be
comparing two different sets of bugs. An arm could then look better purely
because its worker wrote better code.

So the worker runs **once per task**, and its output is forked into both arms:

1. **Seed run** — `alucard run` with `--max-review-cycles 0`, which skips the
   review gate entirely (`seq 1 0` is empty). The CI gate still runs, so CI-fix
   commits are included; that is correct, because both arms should start from
   the same CI-green code. This produces a seed PR whose head is `W`.
2. **Fork** — push `W` to two new branches and open one PR per arm from it.
   Both PRs have the same head SHA and the same body.
3. **Review loops** — `alucard continue <PR>` on each arm PR, using that arm's
   checkout. `continue` enters the review gate without running a worker, and
   takes the PR number explicitly, so no lookup can wander to another PR.

## Preconditions

- **Run everything sequentially.** Seed runs use `alucard run`, whose PR lookup
  falls back to "newest open `alucard`-labelled PR created since this iteration
  started" when the head-branch lookup misses. Sequential seed runs make
  crossover impossible. The arm PRs are created below **without** the `alucard`
  label, so they can never be adopted by that fallback — and no bulk relabelling
  of the repository's PRs is needed or wanted.
- **Merge nothing for the duration.** Keep every pilot PR unmerged.
- **Pin the image, model, effort, and limits.** Same `--image`, same
  `--env-file`, same `--timeout-minutes`, same `--max-review-cycles` for both
  arms. Each `run` record captures the resolved settings and `image_id`, so a
  mismatch is detectable afterwards — but pinning avoids the wasted round.

## Setup: two clean checkouts

Each arm runs from its own checkout of the alucard repository, and is invoked by
explicit path. Do not restore an old prompt into a working branch: that leaves
staged changes, makes `alucard_dirty` true, and means the prompt digest in the
records describes something that is not in any commit. Do not rely on `cd` plus
a bare `alucard` either — that runs whatever is on `PATH`, not the checkout.

```bash
PILOT=~/alucard-pilot/arm1
mkdir -p "$PILOT"
cd ~/aldovc/alucard

SWEEP_BRANCH=feat/lean-loop-step-2
PRE_SWEEP=e8df226            # last commit before the class-sweep section

# Capture the sweep SHA once. Everything below pins to it, so a commit landing
# mid-pilot cannot move an arm underneath the comparison.
SWEEP_SHA=$(git rev-parse "$SWEEP_BRANCH")

# Detached, at that SHA. `git worktree add <path> <branch>` fails when the
# branch is already checked out somewhere — which it is, in this very
# repository — so the branch name cannot be used here.
git worktree add --detach "$PILOT/sweep-tool" "$SWEEP_SHA"

# Generate the control arm from the same SHA, so the two differ in one file.
git branch -f pilot/arm1-control "$SWEEP_SHA"
git worktree add "$PILOT/control-tool" pilot/arm1-control
git -C "$PILOT/control-tool" checkout "$PRE_SWEEP" -- alucard-reviewer-prompt.md
git -C "$PILOT/control-tool" commit -qm "pilot(control): reviewer prompt without the class-sweep section"

# alucard.env is gitignored, so neither checkout has one; pass it explicitly.
ENVFILE=~/aldovc/alucard/alucard.env
IMAGE=ghcr.io/aldovc/alucard:latest

# Must print exactly one path. Anything else and the arms are not comparable.
git -C "$PILOT/control-tool" diff --name-only HEAD "$SWEEP_SHA"

# Both checkouts must be clean; a dirty tree makes the recorded prompt digest
# describe something that is not in any commit.
git -C "$PILOT/control-tool" status --porcelain
git -C "$PILOT/sweep-tool"   status --porcelain

# Record what actually ran. The `run` record stamps alucard_rev too, so these
# can be checked against the artifact afterwards.
git -C "$PILOT/control-tool" rev-parse HEAD
git -C "$PILOT/sweep-tool"   rev-parse HEAD
```

## Task selection

Three tasks, seeded once each. From the baseline audit:

1. **Behaviour change on existing helpers** — family-brain, a device-control
   domain extension in the #402/#403/#427 shape. The slot
   that matters most: it exercises the trust boundary that produced most real
   findings, and it is where the six-cycle loops happened.
2. **Test-heavy application change** — family-brain. Zodiac cannot test this
   arm (its reviewer settled all fifteen audited PRs on cycle 1 and contributed
   zero post-worker lines), so both reviewer-side slots stay in family-brain.
3. **Configuration/docs change** — family-brain or zodiac.

Do not invent features to benchmark. Pick real queued work.

## Seeding from a historical worker head

A fresh seed is a gamble. Two in a row failed to leave a repeated defect behind
— #446 produced one finding at one site, #460 got all three of its selected
sites right — and each cost a worker run and an inspection to find that out. A
merged PR whose review loop is already known to have found one class one site at
a time is a seed whose repetition is documented rather than hoped for, and
replaying it costs no worker invocation at all.

The worker head is the PR's first commit when the worker committed once; the
base is its parent. Replaying #427 this way produced a diff byte-identical to
the historical one, so the technique is sound. Two mechanics bite:

- **The arms cannot target `main`.** A merged PR's change is already in `main`,
  squash-merged, along with every fix its review loop produced. A PR against
  `main` would diff against a tree holding the answers and would almost
  certainly conflict on merge. Pin a base branch at the historical base instead
  and open both arms against it.
- **CI does not fire for a pinned base.** The workflows list `main` alone under
  `on.pull_request.branches`, so a pull request into `pilot/*` reports no checks
  and `ci_gate` skips silently. Widen that filter to include `pilot/**` — on the
  base branch only, never on an arm, or the workflow change joins the diff both
  reviewers read; and never the `push` filter, so nothing builds off a pilot
  branch.

**The limit that decides most candidates.** A historical PR is only usable if
the arm's own prompt does not describe it. See the contamination precondition
below: for the class-sweep arm, the two PRs with the clearest repeated-class
loops in the corpus — #427 and #444 — are the prompt's worked examples, which
rules both out.

## Running

```bash
REPO=~/aldovc/family-brain
TOOL="$PILOT/control-tool/alucard"   # the seed's reviewer prompt is unused
TASK=ha-cover                        # repeat this block per task

# ── 1. Seed: one worker, no review ───────────────────────────────────────────
"$TOOL" run "$REPO" \
  --tasks "$PILOT/$TASK-tasks.md" \
  --logs-root "$PILOT/seed/$TASK" \
  --env-file "$ENVFILE" --image "$IMAGE" \
  --iterations 1 --timeout-minutes 30 --max-review-cycles 0

# Resolve the seed from *this run's own records*, never from a repository-wide
# PR query: if the seed run failed or found no eligible task, the newest open
# `alucard`-labelled PR is somebody else's work, and the fork and close below
# would then operate on it.
SEED_MEAS="$PILOT/seed/$TASK"/alucard-*/measurements.jsonl
SEED_PR=$(jq -rs '[.[]|select(.record=="stage" and .stage=="worker")]|.[0].pr // empty' $SEED_MEAS)
SEED_CI=$(jq -rs '[.[]|select(.record=="iteration")]|.[0].ci_result // empty' $SEED_MEAS)

# W is the *last* recorded stage head, not the worker's. When the seed's CI
# fails, the CI-fix agent commits on top and the worker head is no longer what
# either arm should review; the head that went green is. With no CI repair the
# two coincide, which is why this held on the first live task without proving
# the CI-fix case.
W=$(jq -rs '[.[]|select(.record=="stage")]|sort_by(.ts)|last|.head_sha // empty' $SEED_MEAS)

[ -n "$SEED_PR" ] || { echo "STOP: seed run for $TASK produced no PR"; exit 1; }
[ "$SEED_CI" = "green" ] || { echo "STOP: seed PR #$SEED_PR CI is '${SEED_CI:-unknown}'"; exit 1; }

# Cross-check against GitHub: the branch must be this run's, and the head must
# still be what the run recorded. This is the guard that catches a W taken from
# the wrong stage — it stops rather than forking from a stale head.
#
# Read the head with `git ls-remote` rather than `gh pr view --json headRefOid`:
# gh 2.45.0 rejects some of these *RefOid fields, and git does not care.
SEED_BRANCH=$(gh pr view "$SEED_PR" --repo aldovc/family-brain --json headRefName --jq .headRefName)
SEED_HEAD=$(git -C "$REPO" ls-remote origin "refs/heads/$SEED_BRANCH" | cut -f1)
case "$SEED_BRANCH" in
  alucard/iter-*) ;;
  *) echo "STOP: seed PR #$SEED_PR is on '$SEED_BRANCH', not an alucard branch"; exit 1 ;;
esac
[ "$SEED_HEAD" = "$W" ] || { echo "STOP: seed PR #$SEED_PR head moved since the run"; exit 1; }

gh pr view "$SEED_PR" --repo aldovc/family-brain --json body --jq .body > "$PILOT/$TASK-body.md"

# ── Consolidation precondition, for a task chosen to test sweeping ───────────
# The instruction only does anything when the seed leaves the *same* defect at
# more than one site. A task can be picked for having several places that need
# one change and still produce a seed that gets them all right. Read the seed
# diff now, before either arm runs, and write down which repeated defects
# survive. If none do, the pair is inconclusive for consolidation — record that
# and either pick another task or run it knowing what it can and cannot show.
git -C "$REPO" diff "$(git -C "$REPO" merge-base origin/main "$W")" "$W" \
  > "$PILOT/$TASK-seed.diff"
echo "Inspect $PILOT/$TASK-seed.diff and record surviving repeated defects before running the arms."

# ── Contamination precondition, for any seed used to test sweeping ───────────
# The sweep section carries two worked examples, and they are real PRs from this
# repository: #427's unescaped-untrusted-string loop, three of its sites named
# outright, and #444's automation-action allowlist loop. Seeding from either
# hands the sweep arm this seed's answer while the control arm, whose prompt
# predates the section, gets nothing — so the pair measures the example, not the
# instruction. Read the examples and rule out any seed they describe.
sed -n '/^## Sweep a finding.s class/,/^## /p' "$PILOT/sweep-tool/alucard-reviewer-prompt.md"


# ── 2. Fork the identical worker head into both arms ─────────────────────────
# No `alucard` label: `continue` takes an explicit PR number, and leaving the
# label off keeps these PRs out of any fallback lookup.
for arm in control sweep; do
  git -C "$REPO" push -q origin "$W:refs/heads/pilot/$TASK-$arm"
  pr=$(gh pr create --repo aldovc/family-brain \
        --head "pilot/$TASK-$arm" --base main \
        --title "pilot($arm): $TASK" --body-file "$PILOT/$TASK-body.md" \
        | grep -o '[0-9]*$')
  printf '%s\t%s\t%s\t%s\n' "$TASK" "$arm" "$pr" "$W" >> "$PILOT/mapping.tsv"
done

# The seed PR is a staging artifact. Close it, do not merge it.
gh pr close "$SEED_PR" --repo aldovc/family-brain

# ── 3. Review loops, one per arm, sequentially ───────────────────────────────
# The head is checked *immediately before* each arm starts, while it is still
# expected to equal W. Checking afterwards would fail on every successful arm,
# because feedback commits necessarily move the head. Both the verified initial
# head and the final head are recorded for the comparison below.
while IFS=$'\t' read -r task arm pr w; do
  [ "$task" = "$TASK" ] || continue

  before=$(git -C "$REPO" ls-remote origin "refs/heads/pilot/$task-$arm" | cut -f1)
  if [ "$before" != "$w" ]; then
    echo "SKIP: $task/$arm PR#$pr starts at $before, not the shared head $w"
    continue
  fi

  "$PILOT/$arm-tool/alucard" continue "$pr" "$REPO" \
    --logs-root "$PILOT/$arm/logs" \
    --env-file "$ENVFILE" --image "$IMAGE" \
    --timeout-minutes 30 --max-review-cycles 10

  after=$(git -C "$REPO" ls-remote origin "refs/heads/pilot/$task-$arm" | cut -f1)
  printf '%s\t%s\t%s\t%s\t%s\n' "$task" "$arm" "$pr" "$w" "$after" \
    >> "$PILOT/observed.tsv"
done < "$PILOT/mapping.tsv"
```

`mapping.tsv` is the pairing record — task, arm, PR, shared worker head.
`observed.tsv` adds the final head each arm reached, and is what the result
recipes below read: paired PRs have different numbers, and the additions each
arm made have to be measured from `W`, not from the PR's base.

## Check the pairing before judging anything

`base_drifted` does **not** detect a merge between arms: each arm pins its own
base, so both can report `false` while starting from different commits. Compare
the pairs directly.

```bash
# Every task's two arms must share one worker head. Any output here voids the pair.
awk -F'\t' '{k=$1; if (h[k] != "" && h[k] != $4) print "MISMATCH: " k; h[k]=$4}' \
  "$PILOT/mapping.tsv"

# The start-of-arm head check already happened inline, immediately before each
# `continue`, and an arm that failed it was skipped rather than recorded. So the
# check here is simply that every planned arm actually ran. Re-querying heads now
# would fail on every successful arm, since feedback commits move them.
comm -13 <(cut -f1,2 "$PILOT/observed.tsv" | sort) \
         <(cut -f1,2 "$PILOT/mapping.tsv" | sort) \
  | sed 's/^/DID NOT RUN: /'

# Belt and braces: no run should have seen its base move underneath it.
jq -r 'select(.record=="stage" and .base_drifted==true)
  | "DRIFTED: \(.pr) \(.stage)"' "$PILOT"/*/logs/*/measurements.jsonl
```

Note the arm PRs are cut from `W` but based on `main`, so their recorded base is
the merge base — `baseline_source` will read `merge-base`, not `pinned`. That is
expected here and is why the check above compares heads rather than bases.

## Reading the results

Effort for one PR can be split across several runs — an original run plus any
`continue` — so the recipe aggregates by PR before comparing, then joins to
`mapping.tsv` so the arms line up by *task* rather than by PR number.

```bash
for arm in control sweep; do
  jq -rs --arg arm "$arm" '[.[]|select(.record=="iteration")]
    | group_by(.repo_id + "#" + .pr)[]
    | [ $arm, (.[0].pr),
        ([.[].review_cycles]|add),
        ([.[].duration_s]|add),
        ([.[]|.roles|to_entries[]|.value.invocations]|add),
        ([.[]|.roles.review.invocations // 0]|add),
        (.[-1].review_verdict),
        (if ([.[].cost_complete]|all) then "known" else "partial" end),
        "\([.[].costed_invocations]|add)/\([.[].total_invocations]|add)" ]
    | @tsv' "$PILOT/$arm/logs"/*/measurements.jsonl
done | awk -F'\t' 'NR==FNR{task[$3]=$1; head[$3]=$4; next}
    {printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n",
       task[$2], $1, $2, substr(head[$2],1,8), $3, $4, $5, $6, $7, $8, $9}' \
    "$PILOT/mapping.tsv" - \
  | sort -k1,1 -k2,2 \
  | column -t -N TASK,ARM,PR,WORKER_HEAD,CYCLES,SECONDS,INVOCATIONS,REVIEWS,VERDICT,COST,COSTED

# What each arm's review loop added on top of the shared worker head.
#
# This walks observed.tsv and diffs W..final directly, rather than reading stage
# records, for two reasons. Stage records are written only when something moves
# the head, so an arm that approved on cycle 1 with no edits produces none at
# all and would vanish from the table — yet "changed nothing" is exactly the
# result worth seeing. And `from_base` in a stage record is measured from the
# PR's base, which includes the whole seed implementation; the question here is
# what the *review loop* added above W.
#
# Sourcing the harness reuses measure_diff_json, so these numbers are bucketed
# by the same rules as the artifact rather than by a second copy of them.
source "$PILOT/sweep-tool/alucard"

while IFS=$'\t' read -r task arm pr w final; do
  git -C "$REPO" fetch -q origin "pilot/$task-$arm" 2>/dev/null || true
  measure_diff_json "$REPO" "$w" "$final" \
    | jq -r --arg t "$task" --arg a "$arm" --arg p "$pr" \
        'if .unavailable then [$t,$a,$p,"UNAVAILABLE",.reason,"",""]
         else [$t,$a,$p,(.tests.added|tostring),(.impl.added|tostring),
               (.confdoc.added|tostring),(.added|tostring)] end
         | @tsv'
done < "$PILOT/observed.tsv" \
  | sort -k1,1 -k2,2 \
  | column -t -N TASK,ARM,PR,TESTS,IMPL,CONFDOC,TOTAL_ABOVE_W
```

A row of zeros is a real and interesting result: it means that arm's reviewer
approved without requesting a change. Do not read it as missing data.

`COST` reads `partial` whenever any invocation reported none, which is every run
with a codex reviewer, and `COSTED` gives the ratio behind it — `0/1` means no
figure at all, `1/3` means the total is a lower bound. Both must be printed:
computing completeness and then dropping it from the table is how a partial
total gets read as a whole one.

## Judging it

The arm was *designed* to target cycles. The first live pair did something else:
sweep found a real invariant defect that control approved, then spent an extra
cycle and 62 lines fixing it. That is a quality gain at higher cost, and it is a
legitimate outcome — see `lean-agent-loop-pilot-arm1-results.md`.

So there are three shapes of result, not two:

- **Fewer cycles, same findings, same diff** — the efficiency win the arm was
  built for.
- **More cycles, more lines, a defect the control arm missed** — a quality win.
  Worth keeping, but it is not a savings claim and must never be reported as one.
- **Flat totals, same findings** — the instruction reorganised the work without
  reducing it. A real result; record it as one.

**A one-cycle approval is not a win until you know what it missed.** That is the
trap this pilot walked into and caught: on the raw numbers control looked
strictly better — one cycle, no added lines, less wall time — and it was the
worse outcome. Never score an arm on cycles before step 1 below.

Read in this order.

1. **Acceptance and coverage first.** Did each task's acceptance criteria stay
   met, and is the failure coverage the control arm ended up with still present
   in the sweep arm? A cheaper run that drops a real finding is a failure.
2. **Are the findings the same?** Take the control arm's findings across all its
   cycles and check the sweep arm raised the same ones, consolidated. Both arms
   reviewed identical code, so this comparison is now meaningful — that is the
   whole reason for the shared-worker design. Judge each arm's later findings
   against the revisions its own feedback agent made, since the two diverge as
   soon as the first feedback commit lands.

   Where the arms disagree about whether something is a defect at all, settle it
   independently rather than by reading the reviews: check out the seed head,
   apply the disputed test, and confirm it fails there and passes with the fix.
   That is what turned the first pair's disagreement into evidence.
3. **Cycles per task.** The number the arm is meant to move.
4. **Totals, not per-cycle figures.** Sweeping makes each cycle search more, so
   per-invocation reviewer cost should be expected to *rise*. The arm wins only
   if cycles fall faster than per-cycle cost climbs.
5. **Did any finding sprawl past the PR?** The predicted way this backfires is a
   reviewer reading "find every instance" as licence to audit the surrounding
   system. Check for findings naming files outside the diff, and for feedback
   agents that could not act on a finding. An expanded file set is not automatic
   evidence of sprawl: the first pair's sweep arm touched an extra file to move a
   helper and avoid a circular import, which is a direct dependency of the fix,
   not an unrelated audit.

**Consolidation cannot be observed unless the seed repeats a defect.** The first
pair's sweep arm raised one finding at one helper, so it tested the procedure and
the reviewer's judgement but said nothing about the behaviour the instruction
actually changes.

Choosing a task with several places needing one change is necessary but not
sufficient: the seed worker may get them all right, and then there is nothing to
consolidate. That case is **inconclusive**, and must be recorded as a fourth
outcome distinct from the three above — not as a failed sweep. A reviewer that
finds nothing to consolidate because nothing repeats has behaved correctly, and
scoring it as a failure would punish the arm for the worker's competence.

This is why the seed diff is read before either arm runs. Deciding afterwards
whether repeated defects "were really there" invites reading the answer back out
of the arms' behaviour, which is the comparison itself.

Flat totals mean the instruction reorganised the work without reducing it. That
is a real result and should be recorded as one.

Three paired tasks are exploratory. They can show a direction; they cannot
support a percentage claim or generalise to all work.

## Recording

Save the two tables, `mapping.tsv`, `observed.tsv`, and a short verdict as
`lean-agent-loop-pilot-arm1-results.md`, linking PR numbers and log directories.
Note anything that broke a pairing — a mismatched worker head, an arm that was
skipped because its PR did not start at `W`, a drifted base, an image mismatch —
because a broken pairing is worth knowing about and cheap to miss.

## Cleanup

```bash
git -C ~/aldovc/alucard worktree remove "$PILOT/control-tool"
git -C ~/aldovc/alucard worktree remove "$PILOT/sweep-tool"
```

Close the pilot PRs; do not merge them. Delete the `pilot/*` branches once the
results are recorded.
