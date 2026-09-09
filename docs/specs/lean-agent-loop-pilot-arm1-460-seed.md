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
No findings from this assessment were posted to GitHub or injected into agent
prompts; the PR was corrected and merged independently. See the outcome below.

## Frozen seed and selected sites

- Tool: `ae25183e7d62dfe5801bcf9606ecfb911ff7e567`, clean isolated checkout.
- Base: `93435e2687665a9cf64084a14a670c6e064c6d90`.
- Final CI-green head: `1ed3fb30fe509a2b2a9102c8398f34a95b1ae2f3`.
- Branch: `alucard/iter-1-1788914966`.
- [Backend CI](https://github.com/aldovc/family-brain/actions/runs/34297514373)
  succeeded at that exact head. No CI-fix commits or reviewer invocations.
- Upstream main and seed branch were rechecked after inspection: neither moved.

All three locations are in one query module at the frozen head. The exact
queries and line numbers are recorded with the run artifacts rather than here,
since the target repository is private.

| Selected site | Surviving omission |
|---|---|
| Query 1 of 3 | None |
| Query 2 of 3 | None |
| Query 3 of 3 | None |

Each carries the required predicate.
The three observer checks passed against the actual imported query constants.
These are structural predicate checks, not PostgreSQL integration tests.
The accompanying migration was inspected, not applied to a live database.

## Separate acceptance gaps, not multi-site evidence

Both observations are in the same provider-error handler, a few lines apart.

1. **A provider failure is recorded but then swallowed.** The service catches
   the error and returns a normal result, so the original exception never
   reaches the caller. An isolated probe confirms it. This conflicts with the
   raise-through behaviour the issue asked for. Existing tests expect the normal
   return, so a passing suite does not resolve the contract gap.
2. **A secondary failure inside the handler prevents the first one being
   recorded at all.** The handler performs a second, fallible step before
   writing the failure down. If that step raises, the record is never written
   and the row is left mid-flight. The independent probe reproduces exactly
   that outcome.

Observer-only probes: **3 passed, 2 failed**, exit 1, for the reasons above.
The seed's three existing test modules for the touched area passed
independently in the same container: **26 passed**. The worker reported a full-suite result of
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

The machine-readable assessment is kept with the run artifacts under `logs/`,
not here: it lists the private repository's changed files verbatim.

Persistent, gitignored artifacts: `logs/pilot-arm1-20260909-460/`:

- `seed/alucard-20260909-094923/`: measurements, events, raw worker JSONL,
  exact dispatched prompt, dependency preflight.
- the full frozen seed diff, the source issue, the local task file and the
  setup manifest.
- the observer probe, its output, and the existing-test output.

Persistent isolated checkouts: `.alucard/pilot-460-20260909/`.
Image:
`sha256:25b08ef1db6542136839ecc5b35069b903a642f3d74c3928106f479020866261`.
Seed command used one iteration, 30-minute timeout, zero review cycles,
`--no-build`, and the original gitignored env file.

Do not spend paired-review runs on this seed to claim consolidation. No review
arm, fix or deployment was performed from this assessment.

## Outcome

#464 was corrected and merged on 2026-09-09 without input from this document.
Re-running the same observer probe against merged `main` gives **4 passed,
1 failed**, against 3/2 on the frozen seed:

- The secondary-failure gap is **closed**. The failure is now recorded before
  the fallible step that used to be able to prevent it.
- The swallowed-provider-exception gap is **still present**. The failure is
  recorded and a normal result is returned, so the original exception never
  reaches the caller.

Whether that second one is a defect is a product decision rather than a
measurement. Issue #460 asked for raise-through; the merged behaviour records
and returns. Returning may well be the better choice for the calling surface —
but the issue still says otherwise, and it is still open.
