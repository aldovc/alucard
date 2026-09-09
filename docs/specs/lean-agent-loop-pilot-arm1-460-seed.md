# #460 seed inspection — before either review arm

Date: 2026-09-09. Status: seed completed and inspected; no review arms launched.

## Decision

**Inconclusive for consolidation.** The worker implemented the selected filter
at all three sites. No surviving repeated omission remains in that class, and
the bounded inspection of the full seed diff did not identify another repeated
defect suitable for this comparison. This is not a failed sweep or an approval
of the PR: there are separately reproduced acceptance gaps below.

Seed: [family-brain #464](https://github.com/aldovc/family-brain/pull/464),
implementing [issue #460](https://github.com/aldovc/family-brain/issues/460).
The PR remains open, unmerged, and without formal reviews. Source issue #460
remains open. No findings were posted to GitHub or injected into agent prompts.

## Frozen seed and selected sites

- Tool: `ae25183e7d62dfe5801bcf9606ecfb911ff7e567`, clean isolated checkout.
- Base: `93435e2687665a9cf64084a14a670c6e064c6d90`.
- Final CI-green head: `1ed3fb30fe509a2b2a9102c8398f34a95b1ae2f3`.
- Branch: `alucard/iter-1-1788914966`.
- [Backend CI](https://github.com/aldovc/family-brain/actions/runs/34297514373)
  succeeded at that exact head. No CI-fix commits or reviewer invocations.
- Upstream main and seed branch were rechecked after inspection: neither moved.

All locations below are in `backend/src/family_brain/finance/receipts/repository_psql.py`
at the frozen head.

| Query | Predicate line | Surviving omission |
|---|---:|---|
| SQL_COUNT_UNMATCHED | 81 | None |
| SQL_LIST_UNMATCHED_RECEIPTS | 143 | None |
| SQL_LIST_UNMATCHED_IN_RANGE | 152 | None |

Each has `WHERE transaction_id IS NULL AND status = 'processed'`.
The three observer checks passed against the actual imported SQL constants.
These are structural predicate checks, not PostgreSQL integration tests.
The migration also defaults existing rows to processed and constrains the four
status values. It was inspected, not applied to a live database.

## Separate acceptance gaps, not multi-site evidence

Both observations are in the same provider-error handler in
`backend/src/family_brain/finance/receipts/service.py:156`.

1. **Provider failure is recorded but swallowed (line 160).** The extractor now
   raises, but the service catches the provider error and returns a failed
   receipt. An isolated RateLimitError probe confirms failed status was recorded
   yet the original exception never reached the caller. This conflicts with the
   issue's requested raise-through behavior. Existing service tests explicitly
   expect a normal return, so passing tests do not resolve that contract gap.
   This does not request implementing the later job queue.
2. **Secondary storage failure prevents recording the extraction failure
   (line 159).** The handler uploads an image before calling
   `mark_receipt_failed`. If that upload raises, the status remains processing
   and no failed-record call occurs. The independent probe reproduces exactly
   that outcome.

Observer-only probes: **3 passed, 2 failed**, exit 1, for the reasons above.
The seed's existing three receipt test modules passed independently in the same
container: **26 passed**. The worker reported a full-suite result of
2,558 passed / 4 skipped; GitHub's lint-and-test job independently succeeded.

The two failing scenarios are not two repeated implementation sites. Do not
reclassify this seed as consolidation-eligible merely because two tests fail.
No fix was applied; only a separate validation clone contains the probe file.
This is a bounded seed assessment, not an exhaustive merge review.

## Footprint and effort

Final diff from base: **10 files, +473/-107**:

- Tests: +269/-21.
- Implementation: +187/-86.
- Migration files, classified as confdoc by the harness: +17/-0.

Completed seed: 838 seconds including CI/orchestration; worker invocation
765 seconds, 117 turns. Usage: 9,641 input, 69,225 output, 13,236,054 cache-read,
197,024 cache-write tokens. Reported cost: **$3.8417738**.

An earlier seed on 2026-09-08 was interrupted. Its /tmp checkout, logs, and
terminal session were gone on resumption; neither its unique remote branch
`alucard/iter-1-1788872996` nor a corresponding/newer PR was found. Its usage and
cost cannot be recovered. Thus the completed run's `cost_complete:true` applies
only to that run; **overall experiment cost is incomplete**, not $3.84 total.

The restart kept the same base, tool revision, image, model settings, and task
scope, but necessarily generated a fresh implementation. Artifacts are now
persistent from the start, not copied out of /tmp after completion.

## Evidence and next decision

[Machine-readable assessment](lean-agent-loop-pilot-arm1-460.json).

Persistent, gitignored artifacts: `logs/pilot-arm1-20260909-460/`:

- `seed/alucard-20260909-094923/`: measurements, events, raw worker JSONL,
  exact dispatched prompt, dependency preflight.
- `receipt-460-seed.diff`: full frozen seed diff.
- `source-issue.json`, `receipt-460-tasks.md`, `setup-manifest.json`.
- `test_pilot_seed_contract.py`, `seed-contract-probe.txt`,
  `existing-receipt-tests.txt`.

Persistent isolated checkouts: `.alucard/pilot-460-20260909/`.
Image:
`sha256:25b08ef1db6542136839ecc5b35069b903a642f3d74c3928106f479020866261`.
Seed command used one iteration, 30-minute timeout, zero review cycles,
`--no-build`, and the original gitignored env file.

Do not spend paired-review runs on this seed to claim consolidation. The next
decision is how to correct/review #464 normally and which separate seed can
supply genuine repeated defects for the experiment. No further task, fix,
review arm, deployment, merge, or cleanup was performed.
