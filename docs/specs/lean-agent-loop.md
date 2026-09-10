# Lean implementation and efficient context in Alucard

Status: steps 1–3 complete. Date: 2026-09-08, revised 2026-09-10
against the shipped policy fragment (#73), the mechanical checks, and the
Size-the-remedy rule (#78).

Step 1 landed in PR #69: `lean-agent-loop-baseline.md` beside this file, and
the measurement artifact section 3 asks for. **Read the baseline report before
acting on anything below** — it retired two items this spec used to contain,
added one it did not, and changed which role the policy work should weight.
Sections revised against it are marked *(revised)*; sections that later shipped
are marked *(complete)* or *(implemented)*.

## Problem and outcome

Alucard's generated code and tests have grown substantially in family-brain,
zodiac, and home-cluster. Its worker requests minimal implementation, but its
worker, reviewer, and feedback prompts also impose blanket extraction, constant,
layering, and test expectations. These can expand a change without establishing
that the added structure or coverage protects a useful behavior. Each fresh agent
may then repeat exploration of that growing codebase.

The outcome is an agent loop that meets the task's requirements with less
unnecessary implementation, test infrastructure, and repeated reading. Correctness,
acceptance completion, and maintainability determine success. Line counts and
tokens are diagnostic measurements, not optimization targets or merge gates.

This spec covers changes to Alucard and subsequent validation runs. It does not
schedule work, modify the three target repositories, or launch runs by itself.

## Evidence and influences

A local comparison of `HEAD~20` with `HEAD` on 2026-09-08 found these net changes.
The window is 20 first-parent commits per repository, not a shared time period.
Tests were classified by path; other files include code, configuration, and docs.

Both endpoints are pinned. `HEAD~20` slides as commits land, so a table naming
only the newer SHA stops describing the same window the day after it is written
— zodiac's HEAD had already moved off `e13ecfd` by the time the baseline audit
recomputed this.

| Repository | Window (`base..head`) | Test lines | Other lines |
| --- | --- | ---: | ---: |
| family-brain | `93435e2~20..93435e2` | +5,925 | +5,803 |
| zodiac | `e13ecfd~20..e13ecfd` | +2,013 | +1,248 |
| home-cluster | `d84e788~20..d84e788` | +355 | +4,007 |

The baseline audit reproduced these within classification noise, and treats the
home-cluster test figure as +266 rather than +355 — the difference is which
paths count as tests, not a different measurement.

Growth alone does not prove waste or attribute it to Alucard. For example,
some of family-brain's tests protect a safety-relevant distinction between two
device kinds that behave alike but must not be confused, while home-cluster's
recent growth is mostly service manifests and documentation. The audit must inspect requirements and code.

[Ponytail's rules](https://github.com/DietrichGebert/ponytail/blob/main/skills/ponytail/SKILL.md)
inform the preference for existing capabilities and present needs, with explicit
protection for required behavior and safety. Its one-line preference, aggressive
modes, and minimal-test prescription are not adopted.

[Spotify's Portal article](https://engineering.atspotify.com/2026/9/portal-by-spotify-cut-my-claude-code-token-usage-by-90)
motivates selective context delivery. Its reported bulk-read saving concerns the
main model's context, not demonstrated total Alucard workflow savings. Delegated
reading is a possible later experiment; cheaper code generation is deferred.

## Scope and invariants

Keep the existing Bash orchestrator, per-role containers, providers, task sources,
PR workflow, recovery, CI gate, and review verdicts. Preserve repository-specific
contracts and acceptance criteria. Preserve trust-boundary validation, secrets
handling, cleanup and data-loss prevention, accessibility, and verification of
consequential behavior.

Do not add a new review agent, mandatory large-file read blocker, generic routing
platform, persistent repository index, line-count quota, or automatic cleanup of
existing code. Do not weaken tests to obtain a smaller diff. No Portal dependency
or new model integration is required for this iteration.

## 1. Establish a small evidence baseline *(complete — see the baseline report)*

Inspect approximately five recent Alucard PRs per target repository. Include large
additions, unexpectedly large fixes, and straightforward changes for comparison.
Read the original task, relevant source and tests, initial worker output where
available, review findings, and feedback revisions. A squash merge or missing log
must be reported as unavailable evidence, not reconstructed as fact.

For each candidate simplification, record the location and PR, requirement or
failure protected, unnecessary portion, proposed replacement or deletion,
verification to retain, and confidence. Classify it as safe to simplify, requiring
a product decision, or justified complexity. Attribute its origin to task scope,
worker output, reviewer demands, or repository conventions only when evidenced.

Use existing logs to sample repeated file reads and large tool results by role.
Unknown shell commands remain unclassified; do not build a shell parser to infer
all reads. Output bytes or characters are proxies, not billed token counts.
Record whether the baseline supports the proposed prompt changes before coding.

## 2. Align engineering policy across roles *(complete — #73, #78)*

Shipped as `alucard-engineering-policy.md`, assembled inside the trusted
instructions for worker, reviewer, and feedback invocations. Keep permissions,
role responsibilities, verification and output contracts in their role prompts.
Conflicting policy copies were removed rather than appending another checklist.
The fragment is in `doctor`'s required files.

**Not CI-fix.** `alucard-ci-fix-prompt.md` already says "Edit only the files
needed to fix the failing checks" and "Do not touch code unrelated to the CI
failure", which is a tighter guard than this fragment provides. Adding shared
simplicity guidance there can only loosen it.

**Weight the worker half.** The baseline attributes 89% of Alucard's lines to
the worker's first pass and 11% to review revisions, so the worker-side edits
below are the part of this section that can change a diff. The reviewer-side
edits are worth making — they remove a demonstrated class of merge-blocking
noise — but should not be expected to shrink anything.

The shared policy must express these decisions:

- Understand the affected flow and relevant callers before selecting a solution.
- Reuse existing helpers and repository patterns. Prefer standard-library,
  platform, and installed-dependency capabilities when they meet the requirements.
- Add abstractions, dependencies, configuration, and extensibility for a current
  need. Similar-looking code does not automatically require extraction; extract
  when it represents a shared rule or reduces a concrete maintenance burden.
- Follow established repository architecture. Do not invent new layers, enums,
  wrappers, or constants solely to satisfy a generic checklist.
- Meet every requested acceptance criterion. Stop when the change is complete
  and adequately verified; speculative improvements are outside the task.

### Worker and CI-fix behavior *(revised)*

The worker checks existing implementation and coverage before adding either.
For testable changes, retain red/green verification where useful, but replace
"repeat per criterion" with coverage of changed behavior and distinct failure
modes. An existing test that already covers a criterion counts.

CI-fix keeps its own prompt unchanged and does not receive the shared fragment,
for the reason given above.

### Tests

Extend existing tests when practical. Parameterize equivalent input variations
when that improves clarity. Test observable behavior and contracts; inspect
internal interactions when those interactions are themselves consequential.

Avoid testing framework guarantees, duplicating the same guarantee at multiple
layers without a separate purpose, or creating elaborate mocks for trivial
wiring. Every added test should detect a distinct plausible failure or provide
necessary integration evidence. This is a judgment rule, not a required written
justification per test, test-count cap, or ban on unit tests and fixtures.

Retain coverage for materially distinct entry points, permissions, races, cleanup,
data loss, and other meaningful boundary conditions. A larger test diff can be
correct when the behavior warrants it.

### Reviewer and feedback behavior *(revised)*

A merge-blocking request for more structure or tests must name the concrete
failure, violated contract, or material maintenance problem and the smallest
adequate remedy. Blanket rules about two copies, inline literals, or possible
polymorphism must no longer mandate changes.

The reviewer may identify unnecessary additions, but should not create repeated
cycles around equivalent design preferences. Feedback does not implement optional
suggestions automatically or expand the scope after resolving a finding. Human
instructions and existing blocker handling still apply.

**Cut: excluding optional simplifications from findings sent to feedback.** The
leak is real in code — `alucard:1618` passes the whole review body, `Out of
scope (follow-up)` section included, into `<review_findings>` — but no
family-brain review in the audited window emitted such a section alongside
CHANGES_REQUESTED, and the one that did (zodiac #150) accompanied a BLOCKED
verdict, so no feedback agent ever saw it. There is nothing here to fix yet.

### Two recurring classes, as mechanical checks *(complete — after two pilots)*

This is not in the original spec and is the change the baseline argues for
hardest. Cycle count, not line count, is the dominant avoidable cost: PR #427
spent six review cycles and PR #444 six more, each cycle reporting one instance
of a single issue class and then handing back for a fix. On #427 the class was
untrusted strings reaching model-facing content — found first in the provider
error, then a value echoed back to the caller, then an externally sourced identifier. On
#444 it was holes in one automation-action allowlist. Every finding was real;
they simply arrived one at a time.

This shipped first as a paragraph of prose telling the reviewer to look for
every other instance of a class before writing the finding up. It is no longer
in the prompt. Two entries in **Mechanical checks** replace it, one per class
above: enumerate every value the diff interpolates into model-facing output and
name each one's source, and enumerate every shape a new guard's input can take
and check the guard inspects each. Both say to report their sites as a single
finding.

Why the swap, in one line: the second pilot pair ran the instruction with its
worked examples removed — they had to be, because one of them *is* #427 — and
the reviewer holding it approved a head that still failed an independent probe
for this very class, in three cycles against the control's seven. The
instruction without concrete classes did not produce sweeping. The examples
were the part
naming classes, so the classes are now the artefact and the prose is gone. See
[the second pair](lean-agent-loop-pilot-arm1-427.md); the hypothesis that
examples were the whole effect is recorded there and is not established.

**Both checks are bounded by the existing scope rule, not an exception to it.**
`alucard-reviewer-prompt.md` already defines a finding as in scope when its fix
lands in a file the PR touches or directly breaks, and already tells the
reviewer not to hunt further afield in late cycles. Enumerating a class's sites
means looking harder inside that boundary, never widening it — an instance in a
pre-existing file the diff merely reads through stays an out-of-scope
follow-up. This is the obvious way the change could backfire: a reviewer that
reads "find every instance" as licence to audit the surrounding system would
trade six cycles for one enormous unactionable finding. Neither pilot arm did
that — the second pair's sweep arm stayed inside the PR and its extra file was
a test module — but the checks are narrower than the prose was, so the risk is
smaller rather than gone.

Cost of not doing it, on #427 alone: four review and four feedback invocations,
2.4 MB of tool output, and about 13 minutes of agent wall time, plus four CI
waits. Replaying that head under the current tool reproduced the habit exactly —
the control arm found the class in four separate cycles, its own incomplete fix
included.

Two checks are cheaper to judge than the paragraph was. Each names a class, so
each can be tested on its own against a seed known to contain it, without a
paired run and without a seed that has to supply repetition by luck.

### Size the remedy *(complete — #78)*

A finding can be real, in scope, and still larger than one agent can land in a
single pass. The loop had no way to discover that except by spending a cycle on
it and getting nothing back, then handing the next cycle the same unchanged
commit. One logged run spent four cycles that way.

The reviewer now sizes the smallest adequate fix before writing a finding. It
belongs in the findings list only if one agent could land it in one pass: edits
within files the PR already touches or directly breaks, no new module or layer,
and no test infrastructure the repository does not already have. Anything larger
is recorded under a heading of exactly `## Too large for this loop` for a human
to turn into its own task, and when it is the only thing blocking merge the
verdict is BLOCKED — which already means "nothing in this loop can act on it".
The prompt also tells the reviewer to confirm a named pattern actually exists
before sending an agent to follow it.

**This could not be prompt-only.** The feedback agent is handed the review body
verbatim, so a new section would have arrived looking like a finding and been
acted on — the exact failure being fixed. `strip_nonactionable_sections` keeps
both non-actionable sections (`Out of scope (follow-up)` and
`Too large for this loop`) off the handoff while leaving them on the PR for the
operator. The filter is the harness-side half of the rule; without it the prompt
change would make the loop worse.

Filing oversized findings back to the queue as their own issues is deferred:
Alucard has no `gh issue create` today, and reviewer-side sizing should be
enough until oversized findings keep arriving after #78.

## 3. Measure changes through the loop *(implemented, PR #69)*

Built as `logs/alucard-*/measurements.jsonl`; `README.md` documents the record
shapes and `test/test_measurements.sh`, `test_measurement_gates.sh` and
`test_measurement_run_loop.sh` cover them. The requirements below stand as
written — each one that follows is there because building the baseline by hand
hit the failure it describes. The section is kept for the record.

Use existing JSONL logs, usage extraction, event timing, and pinned base SHA.
Add a small local measurement artifact, with an explicit format version, recording
repository, task/PR, Alucard revision and prompt digest, image identity, role/model
settings, starting SHA, initial worker head, and heads after modifying gate steps.
Archive the dispatched role prompts locally so comparisons are reproducible;
do not include credentials or publish full prompt/log contents to GitHub.

Record added/deleted lines and changed paths for each stage, and base-to-final
net change, separating tests, implementation, config/docs, and generated/lock
files. Keep paths available to correct heuristic classifications. Treat moves,
binary files, partial runs, retries, and merges explicitly; do not sum successive
diffs and call that final growth. If the base changes during a run, flag the
comparison and distinguish integration changes from agent-authored changes.

Report acceptance results, CI/review outcomes, review cycles, elapsed time, and
tokens by role and cache category. Missing usage or cost is unknown, not zero;
partial cost totals must be labeled. All attempts count toward total effort.
Measurement failures are logged but do not fail or alter an otherwise valid run.

This instrumentation needs no dashboard or per-file token accounting. Sampled
tool-read analysis can remain an offline audit until it demonstrates value.

## 4. Experiment with bounded orientation reuse *(revised)*

Implement context reuse after the policy pilot, only if baseline logs show enough
repeated exploration to justify it. Keep it independently switchable and off by
default during the policy comparison; use one experiment setting, not multiple
intensity levels. Missing or invalid orientation falls back to ordinary reading.

The baseline shows the repeated reading is real but concentrated differently
than this section assumes. Per invocation, the heaviest reader is the *feedback*
agent (414 KB, against 199 KB for the worker that wrote the code), and the most
repetitive is the reviewer across cycles of one PR. Both are later roles, so the
mechanism below still fits — but the record's contents should be chosen for them
rather than for a generic successor:

- Verification commands and toolchain layout. One audited worker spent five
  commands establishing whether the repo builds through `Justfile`,
  `backend/justfile`, `uv`, or `poetry`.
- Entry points into hub files. One 6,239-line module appeared in 82
  tool calls across the eleven review and feedback invocations for one PR.
- Static repository documentation. One conventions file was opened by 18 separate
  invocations in a single run and cannot change while that run is in flight.
  This is the cheapest item on the list.

Re-reading the diff is not a target: the reviewer needs the current diff every
cycle and already reads it in narrow ranges rather than whole files.

Have the worker produce an optional, compact navigation record in a separate
output mount. It identifies relevant source entry points, reusable helpers,
existing tests, verification commands, and unresolved navigation questions. It
must not contain review verdicts, claims that code is safe, or replacement task
instructions. Use at most 12 entries and 8 KiB; discard oversized output rather
than silently truncating it. These are initial experiment bounds, not proven
optimal values.

The harness captures the record before disposing the worker clone and stores it
under that run's logs. Stamp it with repository/task identity and the actual
worker commit; do not trust an agent-supplied SHA. Validate every referenced path
as a regular repository file within the checkout, and bind each entry to its
referenced files' Git blob IDs. Reject entries for missing, external, dirty, or
uncommitted content. Attach commands to their relevant manifest or task-runner
files and treat commands as untrusted suggestions, never automatic execution.

Before a later role receives the record, compare file identities with that role's
checkout. Drop changed entries; do not refresh them with another model call in
this first version. Surviving entries are navigation hints, not assertions that
their callers or surrounding behavior remain unchanged. Later agents still read
task requirements, actual code, changed callers, and the full diff as their role
requires. The reviewer must not use the worker's record as correctness evidence.

Keep this record local to one run and task. A subsequent `continue` or rerun may
operate without it; cross-run discovery and persistence are deferred. Separate
metadata output must not relax the reviewer's read-only checkout or permitted
output contract. Retry attempts cannot inherit another attempt's stale record.

## Delivery order and acceptance *(revised 2026-09-10)*

1. ~~Baseline audit and stage/usage measurement~~ — done, PR #69.
2. ~~Reviewer class-sweep instruction; run the first pilot on it alone~~ — run,
   two pairs, and the instruction is withdrawn in favour of two mechanical
   checks. See [pair one](lean-agent-loop-pilot-arm1-results.md) and
   [pair two](lean-agent-loop-pilot-arm1-427.md).
3. ~~Shared policy and aligned role prompts, worker-weighted~~ — done, #73.
   Size-the-remedy rule and harness-side handoff filter landed as #78.
   **Do not run a second pilot on the policy fragment.** The two pairs already
   showed that a one-run-per-arm comparison cannot attribute a prompt change to
   an outcome; another pair would spend the same budget to relearn that. The
   policy and sizing rules are verified at the harness level only — the block
   reaches the right roles, carries the right content, and is stripped from the
   feedback handoff where it must be — by mutating the change and confirming the
   test fails. Whether an agent behaves differently for having read them is
   untested, and that is an accepted limit rather than a gap waiting for another
   pilot.
4. Bounded orientation reuse if repeated-reading evidence warrants it. Parked:
   the baseline's 59%-share-zero-files figure measured reuse *between different
   tasks*. Within one PR's review cycles the agents share nearly everything —
   one cycle redid 65–70% of the previous cycle's investigation. If unparked,
   scope it to same-PR cycles; do not re-run the cross-iteration measurement.
5. Record findings and refine or remove changes that do not help.

Steps 2 and 3 were one step in the original order. They are split because they
are separately attributable and act on different costs — the reviewer-side
change on review cycles, the policy fragment on worker-authored lines —
and running them together would make an already-noisy three-task comparison
uninterpretable. Step 4 is the *least* supported of the three: see the standing
item below, which the baseline ranks above it, and the parking note on step 4.

### Standing item, outside this spec's sections

Zodiac needs headless-Chromium shared libraries in the container image. Eight of
the fifteen audited zodiac PRs needed a human the next morning purely to capture
browser evidence the sandbox could not produce, and that is the entire measured
review cost in that repository. It is a Dockerfile change, not a prompt
experiment, and the baseline ranks it above section 4.

Implementation acceptance requires both task sources and existing providers to
receive the shared policy once, with correct role contracts and escaped untrusted
context. Existing recovery, CI, verdict, and blocker behavior must remain intact.
Small meaningful fixture checks cover prompt assembly, measurement accounting,
and, if implemented, stale/malformed/cross-task orientation rejection and fallback.
Avoid tests that assert every sentence of prompt prose.

Use the repository's existing Bash tests and ShellCheck commands from CI. Static
checks establish harness behavior; model behavior requires the pilot below.

## Pilot runs and iteration *(revised)*

Use small rounds spanning family-brain and zodiac. Select one bounded task per
arm per repository: one behavior change involving existing helpers, one
test-heavy application change, and one configuration/docs change. Avoid
inventing features just to benchmark them.

**Drop home-cluster from the paired pilot.** Seven Alucard PRs exist across its
whole history and one has run logs; that cannot support a comparison. Use a
configuration/docs task in zodiac or family-brain for the third slot instead.

**Which repository can test which arm.** Family-brain is the only one that can
test the class-sweep instruction: its reviewer raised 36 findings over 13 PRs
and ran six cycles twice. Zodiac's reviewer settled all fifteen audited PRs on
cycle 1 and contributed zero post-worker lines, which makes it useless for a
reviewer-side arm and ideal for a worker-side one — with review-driven growth at
zero, any change in test volume is attributable to the worker policy alone.

Family-brain's device-control domain extensions (the #402/#403/#427
shape) are the right behaviour-change slot: bounded, they exercise the trust
boundary that generates most real findings, and four comparable historical runs
exist to sanity-check against.

For each round, compare the current prompts with the revised ones on each
selected task, using separate checkouts of the same starting commit, and change
exactly one arm at a time. Pin model, effort, limits, image, task content, and
relevant repository instructions. Use isolated task queues and distinct branches
so one arm cannot claim, close, or consume the other's task. Historical runs
provide context but are not equivalent to a paired baseline. Keep validation PRs
unmerged and avoid deployments or live infrastructure changes as part of the
experiment.

Keep all failures and retries in the results. Three paired tasks are an
exploratory pilot, not enough to promise a percentage improvement or generalize
to all work.

**What the first round is measuring.** The class-sweep arm should move review
cycles per PR, and through them tool output, invocation count, and wall time —
not line count. Judge it on cycles and on whether the findings it consolidates
are the same findings the serial version eventually found. A round where the
diff is unchanged and the cycle count halves is a success.

**What it actually measured.** Two pairs, opposite directions, one stochastic
run each. Pair one: the sweep arm caught a defect the control missed and spent
an extra cycle and 62 lines fixing it — quality up, cost up. Pair two: the sweep
arm approved in three cycles against seven, for a third of the cost, on a head
that still failed an independent probe for the class it was supposed to sweep —
cost down, quality down. Cycle count moved in both, in opposite directions. The
prediction below held in neither, and the arm was withdrawn rather than tuned.

Watch the counterweight: sweeping makes each cycle do more searching, so
per-invocation reviewer cost should be expected to rise. The arm only wins if
cycles fall faster than per-cycle cost climbs, which is a comparison of totals,
not of either figure alone. `measurements.jsonl` records both — reviewer
invocations and their tool output per iteration — so this is checkable rather
than assumed. If totals come out flat, the honest reading is that the
instruction reorganised the work without reducing it.

**First live pair, 2026-09-08 (family-brain #446).** It produced a shape this
spec did not anticipate: the sweep arm found an invariant defect the control arm
approved, and paid an extra cycle and 62 lines to fix it. Better coverage, no
efficiency saving — independently verified, in that the added regression fails
on the seed head and passes with the fix. Two consequences for judging. A
one-cycle approval must never be scored as a win before checking what it missed:
on the raw numbers the control arm looked strictly better and was the worse
outcome. And a quality gain must never be reported as a savings claim. That pair
also could not test consolidation at all, because the sweep arm raised one
finding at one site; a multi-site task is needed before the instruction's actual
mechanism has been measured. Full write-up in
`lean-agent-loop-pilot-arm1-results.md`.

For each pair, inspect acceptance completion and retained failure coverage first,
then unnecessary abstractions/tests/docs, reviewer-induced growth, repeated reads,
review cycles, total tokens/cost where known, and wall time. The maintainer's
review supplies qualitative judgment alongside runnable acceptance checks.

Retain a policy change when it removes demonstrated unnecessary work without
losing required behavior or useful coverage. Keep orientation reuse disabled if
staleness, missed context, or generation overhead outweighs saved exploration.
Any missed requirement or consequential regression triggers diagnosis and
revision before rollout. Equal LOC is acceptable; fewer lines alone is not a win.

Save a short results report beside this spec, linking tasks, run artifacts, and
PRs, and recording what to keep, revise, or drop. Existing-code cleanup candidates
remain follow-up work, separate from these harness changes.

## Deferred model-routing experiment

If the pilots still show substantial broad-reading cost, trial a read-only helper
for bounded discovery questions. Preserve direct source access for edits,
debugging, security, and architectural judgment. Account for helper tokens,
latency, failures, cache behavior, and correction work in total run metrics.
Do not delegate code/test generation until restraint on unnecessary additions is
validated. Any such extension needs evidence from these pilots and a spec update.
