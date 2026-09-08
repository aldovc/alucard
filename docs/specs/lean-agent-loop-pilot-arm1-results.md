# Reviewer class-sweep: first live paired pilot

Date: 2026-09-08. Status: one task completed; expansion paused for interpretation.

## Outcome

The live procedure works through seed, CI, both `continue` calls, feedback,
re-review, and artifact capture. **This is a coverage improvement in this pair,
not an efficiency win.** Control approved a defect that sweep found and fixed.
A single stochastic pair cannot establish that the prompt caused the difference.

Task: [family-brain #446](https://github.com/aldovc/family-brain/issues/446),
preserve an automation's alias on update. The local task chose documenting
unsupported renames, not implementing them. No deployment or live HA changes.

| Arm | PR | Verdict | Review cycles | Agent invocations | Loop seconds | Test additions above seed | Implementation additions above seed |
|---|---|---|---:|---:|---:|---:|---:|
| Control | [#462](https://github.com/aldovc/family-brain/pull/462) | APPROVED | 1 | 1 reviewer | 91 | 0 | 0 |
| Sweep | [#463](https://github.com/aldovc/family-brain/pull/463) | APPROVED | 2 | 2 reviewers + 1 feedback | 341 | 37 | 25 |

Sweep's total diff above the seed is +62/-12 across five files, not the +162/-17
six-file diff measured from main. The seed itself added 58 test lines and 46
implementation lines (+104/-9). Control's zero is a verified empty diff, despite
there being no control stage record.

Loop seconds include CI waiting and orchestration, exclude the shared seed and
dependency preflight. Control waited for its initial CI; sweep's initial CI was
already complete. Reviewer invocation times were 46s (control) versus 66s + 37s
(sweep); sweep feedback took 180s. Do not treat the wall-time ratio as model speed.

## What the extra work bought

Sweep's [cycle-1 finding](https://github.com/aldovc/family-brain/pull/463#issuecomment-5585328499)
identified an introduced invariant gap: the new alias-preservation helper copied
any nonempty alias returned by HA. The execution path deliberately re-reads that
config after confirmation because it can be edited directly in the meantime.
A mismatched live alias could therefore be written with the old immutable ID,
violating the ticket's explicit `slug(alias) == object_id == config id` criterion.
Control's review approved this same implementation.

Feedback validates the live alias against the expected object ID at both preview
and write time, retaining the parser's canonical fallback on mismatch. It moves
the existing slug helper into `home_common.py` to avoid a circular import and
preserves its existing import surface. This expands the changed-file set for a
direct dependency reason, not an unrelated audit. Sweep approved on cycle 2;
there were no later findings, known blockers, or CI-fix invocations.

Independent validation used a separate checkout and the pinned container, without
changing either arm:

- Seed implementation + sweep's final test file: **32 passed, 1 failed**.
- Exact same test file + sweep implementation: **33 passed**.
- The sole failure is
  `test_automation_update_ignores_live_alias_desynced_by_direct_ha_edit_at_confirm_time`:
  seed writes `Something Else Entirely` where the test expects `FB: Hall Light`.
- Existing colon/mixed-case preservation and supplied-alias rejection tests
  remain passing. This evidence is mocked application-level behavior, not a
  claim about a live HA deployment.
- All 17 Alucard shell test scripts passed in the isolated sweep checkout.
  ShellCheck was not run for this pilot.

This does not exercise consolidation of multiple findings of the same class:
sweep raised one finding at the shared helper. It also shows why a one-cycle
approval must not be scored as a win without checking what it missed.

## Usage, with missing cost kept explicit

| Role total | Input (uncached) | Output | Cache read | Cache write reported | Known cost |
|---|---:|---:|---:|---:|---:|
| Control reviewer | 42,777 | 2,519 | 288,347 | 42,747 | unknown |
| Sweep reviewers, both cycles | 88,949 | 5,215 | 647,178 | 88,880 | unknown |
| Sweep feedback | 2,948 | 13,923 | 2,681,491 | 75,198 | $0.8665042 |

These are artifact fields, not a cross-provider normalized billing total. For
Codex, uncached input is reported input minus cached input; cache-write input is
also reported separately and must not be added again to that input total.
Both arms have `cost_complete:false`: control has 0/1 costed invocations, sweep
1/3. All invocations have parseable usage. The shared seed cost is recorded
separately in the JSON; it is not charged to one arm or counted twice.

## Pairing and provenance

- Shared seed `W`: `cf508fa974404e22f844095b3b62cd5e7b4605c5`.
- Base: `93435e2687665a9cf64084a14a670c6e064c6d90`; upstream main checked
  before seeding, between arms, and after completion, with no movement.
- Control final: `cf508fa974404e22f844095b3b62cd5e7b4605c5`.
- Sweep final: `79fe095cf5090b92bae7a75a0c30458240f34696`.
- Sweep tool: `41283742dc4f973d353566d5faa111019abae1f4`.
- Control tool: `65839ff3f9414bf61579c1506d96a2ac221076a4`, generated from
  sweep by restoring only `alucard-reviewer-prompt.md` from `e8df226`.
- Both tool checkouts clean; only that one file differs.
- Image pinned by ID:
  `sha256:25b08ef1db6542136839ecc5b35069b903a642f3d74c3928106f479020866261`.
- Same original env file, providers, model settings, effort, and limits. Reviewer
  and CI-fix: Codex `gpt-5.6-terra`; worker/feedback: Claude `sonnet`; effort
  unset. Both arms: 30-minute invocation timeout, maximum 10 review cycles,
  `--no-build`. Seed: one iteration, zero review cycles.
- Identical arm bodies, complete acceptance context, no initial comments and no
  `alucard` label. Each arm head verified equal to W immediately before running.
- Continued runs correctly record `baseline_source:merge-base`; feedback records
  the shared base and W as its previous head.

GitHub Actions independently confirmed success at the measured heads:
[seed](https://github.com/aldovc/family-brain/actions/runs/34227067073),
[control](https://github.com/aldovc/family-brain/actions/runs/34227334295),
[sweep seed](https://github.com/aldovc/family-brain/actions/runs/34227339796),
[sweep final](https://github.com/aldovc/family-brain/actions/runs/34228038393).

## Operational observations

- Used independent local clones instead of worktrees, leaving the user's two
  working copies untouched. Runtime code and prompts were not changed mid-run.
- Check-rollup permissions failed; Alucard's `gh run list` fallback actually
  ran and waited for successful Actions results. Formal self-reviews were
  rejected; decision files, audit comments, and measurements worked.
- Host `gh` 2.45.0 does not support `gh pr view --json baseRefOid`. The
  inspection used supported fields and git for base checks. Continue found its
  merge base, so its last-resort current-base fallback was not exercised.
- The seed needed no CI repair. The runbook's extraction of W from the worker
  stage therefore worked here, but still does not validate the case where CI-fix
  advances the seed head. That case needs the final CI-green recorded head,
  cross-checked against GitHub, before another task relies on it.
- Read cost completeness directly from the artifact: the runbook's rendered
  summary currently drops its computed partial/known field.
- Staging PR [#461](https://github.com/aldovc/family-brain/pull/461) was closed
  only after verifying both forks. Source issue #446 remains open with its
  original labels. Arm PRs and branches are retained, unmerged, for inspection;
  destructive pilot cleanup is deferred. **Do not merge control #462: it misses
  the validated invariant regression.** Sweep #463 contains the stronger fix.
- No other queued task or repository was run. Nothing committed in the primary
  Alucard checkout.

## Evidence and next decision

Machine-readable measurements and pairing:
[lean-agent-loop-pilot-arm1-446.json](lean-agent-loop-pilot-arm1-446.json).

Raw JSONL, exact dispatched prompts, events, task/body snapshots, mapping,
observed heads, and independent pass/fail output are preserved under
`logs/pilot-arm1-20260908-446/` (gitignored, 2.5 MB at capture), not only in /tmp.
The original isolated checkouts remain at `/tmp/alucard-pilot-446.8CmoPW`.

### Selected next task: family-brain #460

`feat(finance): receipt ingest correctness`. Chosen because its implementation
must touch several sites of the same defect class, which is the one thing this
pair could not test. Site inventory taken from the current tree, not the issue
text (the issue's line references were checked and are accurate):

| Class | Sites | Where |
|---|---:|---|
| Unmatched query does not exclude non-`processed` rows | 3 | `finance/receipts/repository_psql.py` `SQL_COUNT_UNMATCHED:74`, `SQL_LIST_UNMATCHED_RECEIPTS:122`, `SQL_LIST_UNMATCHED_IN_RANGE:131` — reached from `household_observer.py`, `matcher.py`, `service.py` |
| Write is not idempotent under retry | 2 | `repository_psql.py` `create_receipt_items:230`, `update_receipt_extraction:291` |
| Synchronous Pillow decode on an async path | 2 | `receipts/service.py` `_resize_image:27`, and the new pre-extraction downscale |

All sites sit in files the change must touch, so a sweeping reviewer can report
them without leaving the PR's scope — which the previous task could not offer.
Three independent classes also means a failed sweep on one is still observable
against the other two.

**#447 was rejected, with evidence.** Its class — a tool's `content` asserting
requested rather than applied identity — has exactly one site in the codebase:
`home_routines_edit.py:351`. Every sibling write tool (bookstack books, shelves,
chapters and pages, tasks, calendar, `home_entities`) already names the applied
result. A sweep there would correctly find nothing to consolidate, so the task
cannot test the instruction either way.

Treat this as a successful operational pilot with unequal quality outcomes.
Before expanding, incorporate the final-CI-head and visible-cost caveats into the
procedure. Then select one further real, multi-site task where consolidation can
actually be observed, retaining the current prompt as an experimental arm.
Repeat paired reviews before attributing a quality or efficiency effect to the
prompt; do not roll out a savings claim or add the shared policy fragment yet.
