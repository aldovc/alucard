# Pilot runbook: reviewer class-sweep (arm 1)

Status: ready to run, not yet run. Date: 2026-09-08.
Procedure for the first paired comparison in `lean-agent-loop.md`. Written to
be followed literally, because the arms are only comparable if the setup is.

## What is being compared

One variable: the **Sweep a finding's class before you report it** section in
`alucard-reviewer-prompt.md` (commit `ae844ce`). Nothing else differs.

- **Control arm** — the reviewer prompt as of `e8df226`, before that section.
- **Sweep arm** — the reviewer prompt as of `ae844ce`.

The shared policy fragment is *not* in this round. It is arm 2, and the spec's
delivery order puts it after this one specifically so the two are separately
attributable.

## Preconditions

- **Run the arms sequentially, never concurrently.** They push to the same
  origin and both label their PRs `alucard`. If the run loop's head-branch
  lookup misses, it falls back to "newest open `alucard`-labelled PR created
  since this iteration started" — with both arms live, that can adopt the other
  arm's PR and review it. Sequential runs make this impossible.
- **Merge nothing between the arms.** Both must start from the same base
  commit. `alucard` fetches `origin/<base>` at each iteration start, so any
  merge in between moves the second arm's baseline and voids the pairing.
  `measurements.jsonl` will record `base_drifted: true` if this happens
  anyway — check it rather than trusting the procedure.
- **Pin the image.** Use the same `--image` for both arms, and record its ID.
  The `run` record captures `image_id`, so a mismatch is detectable after the
  fact, but pinning avoids the wasted round.
- **Pin model, effort, and limits.** Same `alucard.env`, same
  `--timeout-minutes`, same `--max-review-cycles` for both arms. The `run`
  record captures the resolved per-role settings.
- Confirm `git -C <alucard> status` is clean before each arm. The `run` record
  stamps `alucard_dirty`, and a dirty tree means the prompt digest describes
  something not in any commit.

## Task selection

Three tasks, run by **both** arms. From the baseline audit:

1. **Behaviour change on existing helpers** — family-brain, a
   `control_home_device` domain extension in the #402/#403/#427 shape. This is
   the slot that matters most: it exercises the trust boundary that produced
   most of the real findings, and it is where the six-cycle loops happened.
2. **Test-heavy application change** — family-brain. Zodiac cannot test this
   arm (its reviewer settled all fifteen audited PRs on cycle 1 and contributed
   zero post-worker lines), so both reviewer-side slots stay in family-brain.
3. **Configuration/docs change** — family-brain or zodiac.

Do not invent features to benchmark. Pick real queued work.

## Setup

Give each arm its own tasks file and its own logs root, so neither can claim,
close, or consume the other's task. `reconcile_tasks_file` only inspects PRs
recorded in its own file, so separate files are complete isolation.

```bash
PILOT=~/alucard-pilot/arm1
mkdir -p "$PILOT"/{control,sweep}

# Same three tasks, one copy per arm.
cp arm1-tasks.md "$PILOT/control/tasks.md"
cp arm1-tasks.md "$PILOT/sweep/tasks.md"
```

## Running

```bash
cd ~/aldovc/alucard

# ── Control arm ──────────────────────────────────────────────────────────────
git checkout e8df226 -- alucard-reviewer-prompt.md
alucard run ~/aldovc/family-brain \
  --tasks "$PILOT/control/tasks.md" \
  --logs-root "$PILOT/control/logs" \
  --iterations 3 --timeout-minutes 30 --max-review-cycles 10

# Take the arm's PRs out of the `alucard`-labelled pool before the next arm
# runs, so the fallback lookup cannot reach them. Do not merge them.
gh pr list --repo aldovc/family-brain --label alucard --state open \
  --json number --jq '.[].number' \
  | xargs -I{} gh pr edit {} --repo aldovc/family-brain --remove-label alucard

# ── Sweep arm ────────────────────────────────────────────────────────────────
git checkout ae844ce -- alucard-reviewer-prompt.md
alucard run ~/aldovc/family-brain \
  --tasks "$PILOT/sweep/tasks.md" \
  --logs-root "$PILOT/sweep/logs" \
  --iterations 3 --timeout-minutes 30 --max-review-cycles 10
```

Keep every PR from both arms unmerged. Keep all failures and retries in the
results — a retried iteration is part of what the arm cost.

## Reading the results

Both recipes read `measurements.jsonl` and need nothing else.

```bash
# Per-PR cost. This is the arm's headline: cycles, invocations, wall time.
# Sorted by PR so the two arms line up as pairs rather than in run order.
for arm in control sweep; do
  jq -r --arg arm "$arm" 'select(.record=="iteration")
    | [$arm, .pr, .review_cycles, .duration_s,
       ([.roles[].invocations]|add), (.roles.review.invocations // 0),
       .review_verdict, .ci_result,
       (if .cost_complete then "$\(.roles|[.[].cost//0]|add)" else "partial" end)]
    | @tsv' "$PILOT/$arm/logs"/alucard-*/measurements.jsonl
done | sort -k2,2n -k1,1 \
  | column -t -N ARM,PR,CYCLES,SECONDS,INVOCATIONS,REVIEWS,VERDICT,CI,COST

# Final diff per PR. This should come out roughly unchanged between arms.
for arm in control sweep; do
  jq -rs --arg arm "$arm" '[.[]|select(.record=="stage")]
    | group_by(.pr)[] | (sort_by(.ts) | last) as $final
    | [$arm, $final.pr, $final.stage, $final.cycle,
       $final.from_base.tests.added, $final.from_base.impl.added,
       $final.from_base.added] | @tsv' \
    "$PILOT/$arm/logs"/alucard-*/measurements.jsonl
done | sort -k2,2n -k1,1 \
  | column -t -N ARM,PR,LAST_STAGE,CYCLE,TESTS,IMPL,TOTAL
```

Both read every `measurements.jsonl` under the arm, so a `continue` run's
records are picked up alongside the original run's — which is how PR #444's
review rounds would have been counted had this existed then.

Cost prints as `partial` whenever any invocation reported none, which is every
run where the reviewer is on codex. That is the honest label, not a defect.

## Judging it

The arm targets cycles, not lines. Read in this order.

1. **Acceptance and coverage first.** Did each task's acceptance criteria get
   met, and is the failure coverage the control arm ended up with still present?
   A cheaper run that drops a real finding is a failure, not a win.
2. **Are the findings the same?** Take the control arm's findings across all its
   cycles and check the sweep arm raised the same ones, consolidated. This is
   the whole claim. A sweep arm that ends in two cycles because it found less is
   the failure mode, not the result.
3. **Cycles per PR.** The number the arm is meant to move.
4. **Totals, not per-cycle figures.** Sweeping makes each cycle search more, so
   per-invocation reviewer cost should be expected to *rise*. The arm wins only
   if cycles fall faster than per-cycle cost climbs. Compare total invocations,
   total reviewer tool output, and wall time.
5. **Did any finding sprawl past the PR?** The predicted way this backfires is a
   reviewer reading "find every instance" as licence to audit the surrounding
   system. Check for findings naming files outside the diff, and for feedback
   agents that could not act on a finding.

Flat totals mean the instruction reorganised the work without reducing it.
That is a real result and should be recorded as one.

Three paired tasks are exploratory. They can show a direction; they cannot
support a percentage claim or generalise to all work.

## Recording

Save the two tables and a short verdict beside this file as
`lean-agent-loop-pilot-arm1-results.md`, linking the PR numbers and the log
directories. Note anything that broke the pairing — a merge between arms, a
`base_drifted: true`, an image mismatch — because a broken pairing is worth
knowing about and cheap to miss.
