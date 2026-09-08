# Lean implementation and efficient context in Alucard

Status: draft for implementation. Date: 2026-09-08.

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

| Repository and observed HEAD | Test lines | Other lines |
| --- | ---: | ---: |
| family-brain, `93435e2` | +5,925 | +5,803 |
| zodiac, `e13ecfd` | +2,013 | +1,248 |
| home-cluster, `d84e788` | +355 | +4,007 |

Growth alone does not prove waste or attribute it to Alucard. For example,
family-brain's cover-control tests protect a meaningful distinction between
curtains and garage doors, while home-cluster's recent Audiobookshelf growth is
mostly manifests and documentation. The audit must inspect requirements and code.

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

## 1. Establish a small evidence baseline

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

## 2. Align engineering policy across roles

Add one short `alucard-engineering-policy.md` fragment, assembled inside the
trusted instructions for worker, reviewer, feedback, and CI-fix invocations.
Keep permissions, role responsibilities, verification and output contracts in
their role prompts. Remove conflicting policy copies instead of appending another
checklist. Include the fragment in `doctor`'s required files.

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

### Worker and CI-fix behavior

The worker checks existing implementation and coverage before adding either.
For testable changes, retain red/green verification where useful, but replace
"repeat per criterion" with coverage of changed behavior and distinct failure
modes. An existing test that already covers a criterion counts.

CI-fix stays focused on the actual failure and its cause. Shared simplicity
guidance must not invite unrelated refactoring or changes to passing tests.

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

### Reviewer and feedback behavior

A merge-blocking request for more structure or tests must name the concrete
failure, violated contract, or material maintenance problem and the smallest
adequate remedy. Blanket rules about two copies, inline literals, or possible
polymorphism must no longer mandate changes.

The reviewer may identify unnecessary additions, but should not create repeated
cycles around equivalent design preferences. Optional simplifications are clearly
non-blocking and excluded from actionable findings sent to feedback. Feedback
does not implement optional suggestions automatically or expand the scope after
resolving a finding. Human instructions and existing blocker handling still apply.

## 3. Measure changes through the loop

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

## 4. Experiment with bounded orientation reuse

Implement context reuse after the policy pilot, only if baseline logs show enough
repeated exploration to justify it. Keep it independently switchable and off by
default during the policy comparison; use one experiment setting, not multiple
intensity levels. Missing or invalid orientation falls back to ordinary reading.

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

## Delivery order and acceptance

1. Baseline audit and stage/usage measurement, retaining current behavior.
2. Shared policy and aligned role prompts; run the first pilot.
3. Bounded orientation reuse if repeated-reading evidence warrants it; run the
   second pilot with policy held constant.
4. Record findings and refine or remove changes that do not help.

Implementation acceptance requires both task sources and existing providers to
receive the shared policy once, with correct role contracts and escaped untrusted
context. Existing recovery, CI, verdict, and blocker behavior must remain intact.
Small meaningful fixture checks cover prompt assembly, measurement accounting,
and, if implemented, stale/malformed/cross-task orientation rejection and fallback.
Avoid tests that assert every sentence of prompt prose.

Use the repository's existing Bash tests and ShellCheck commands from CI. Static
checks establish harness behavior; model behavior requires the pilot below.

## Pilot runs and iteration

Use two small rounds spanning family-brain, zodiac, and home-cluster. Select one
bounded task per repository: one behavior change involving existing helpers, one
test-heavy application change, and one configuration/docs change. Select actual
tasks after the baseline audit; avoid inventing features just to benchmark them.

For the first round, compare the current prompts with the revised policy on each
selected task, using separate checkouts of the same starting commit. Pin model,
effort, limits, image, task content, and relevant repository instructions. Use
isolated task queues and distinct branches so one arm cannot claim, close, or
consume the other's task. Historical runs provide context but are not equivalent
to a paired baseline. Keep validation PRs unmerged and avoid deployments or live
infrastructure changes as part of the experiment.

If the second round tests orientation reuse, compare policy-only with
policy-plus-orientation using the same paired setup. If reuse is not justified,
use that round to validate a refined policy or repeat a noisy comparison. Keep
all failures and retries in the results. Three paired tasks are an exploratory
pilot, not enough to promise a percentage improvement or generalize to all work.

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
