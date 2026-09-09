# Baseline audit for the lean agent loop

Status: step 1 of `lean-agent-loop.md`, complete. Date: 2026-09-08.
No prompt and no target repository was modified. The one harness change is the
stage-measurement instrumentation described under "Instrumentation", which
records but does not alter behaviour.

## What was measured

Alucard PRs were identified by the `alucard/` head-branch prefix. Runs were joined
to PRs through `PR found: #N` lines in `logs/*/events.log`, which also supply the
worker-finish timestamp. Commits are then bucketed three ways, by author and by
that timestamp: `alucard-bot` at or before it is **worker**, `alucard-bot` after it
is **agent feedback**, and anything authored by a human is **human**. The earlier
draft of this audit collapsed the last two and got zodiac badly wrong. Per-role
token, cost, and tool-output volumes come from the run JSONL.

| Repository | Alucard PRs, all time | With run logs | Swept | Read in detail |
| --- | ---: | ---: | ---: | --- |
| family-brain | 68 | 51 | 16 | 5 (#427, #431, #433, #444, #445) |
| zodiac | 51 | 24 | 15 | 5 (#144, #146, #149, #150, #153) |
| home-cluster | 7 | 1 | 3 | 3 (#21, #24, #29) |

The swept column is the most recent contiguous window per repository — family-brain
27-30 August (#400-#445), zodiac 6-7 September (#140-#153, plus #104). Older PRs were
not swept; conclusions below describe current prompt behaviour, not all of history.
The detail column is where task, diff, tests, review findings, and feedback commits
were read line by line.

### Evidence limits

- **home-cluster is not a usable sample.** Seven Alucard PRs exist across its whole
  history, four in the recent listing, one of those closed unmerged, and only #29 has
  run logs. Its `HEAD~20` growth is service manifests and docs written
  outside Alucard. Nothing in this report about reviewer or worker behaviour is
  supported by home-cluster evidence.
- **Costs are partial.** Worker invocations run on the claude provider and report
  `total_cost_usd`; reviewer, feedback, and CI-fix run on codex, which emits no cost
  field. Reviewer and feedback cost is unknown, not zero.
- **Token counts are not comparable across providers or across roles.** Both
  providers accumulate cache reads per turn, so a 100-turn worker and a 10-turn
  reviewer are counted on different scales. Tool-output bytes and invocation counts
  are the comparable figures used below; they are proxies, not billed tokens.
- **Shell commands are only partly classified.** File touches were counted by
  matching path-shaped tokens in commands; anything else is unclassified. No shell
  parser was built.
- **Squash merges hide per-commit history on the base branch.** Commit-level
  attribution came from the PR commit lists, which survive; `git log` on `main`
  shows one squashed commit per PR.
- **The spec's own evidence table reproduces, with two corrections.** Recomputed
  today: family-brain net tests +5,913 / other +5,815 (spec: +5,925 / +5,803);
  zodiac +2,013 / +1,248 (exact match); home-cluster +266 / +4,096 (spec: +355 /
  +4,007) — the home-cluster gap is test-path classification, not a different
  measurement. Zodiac's HEAD is now `f0dea09`, not the `e13ecfd` the spec records:
  the `HEAD~20` window slides, so that table needs pinned SHAs to stay meaningful.

## Finding 1 — the worker writes 89% of Alucard's lines; review revisions write 11%

Across 16 family-brain PRs, the worker's own first push already carries the
test-heavy shape. Test-to-implementation line ratios at worker head: #443 348/218,
#402 542/460, #401 749/584, #403 193/166, #432 265/107, #434 240/176. The reviewer
did not ask for those; they are the worker prompt's `Repeat per criterion` loop
plus the repositories' own conventions.

Attributing every line in the two windows by author and stage:

| Repository | worker lines | agent feedback | human |
| --- | ---: | ---: | ---: |
| family-brain (17 PRs) | 9,994 | +1,523 (+15%) | +122 (+1%) |
| zodiac (15 PRs) | 2,125 | +0 (+0%) | +319 (+15%) |
| both | 12,119 | +1,523 (+13%) | +441 (+4%) |

Percentages in that table are growth relative to worker head. As a share of the
13,642 lines Alucard authored in total, **the worker's first pass is 89% and review
revisions are 11%.** Any policy aimed at line count has to act on the worker to
matter. (The human column is not Alucard's output and is excluded from that share;
it is discussed under zodiac below.)

Per-PR, growth on top of worker head is usually small and occasionally large. In
family-brain every post-worker Alucard commit in this window is a feedback commit —
the messages name the finding:

| PR | cycles | worker → final lines | growth |
| --- | ---: | --- | ---: |
| #443, #403, #402 | 1 | unchanged | +0% |
| #428 | 2 | 398 → 406 | +2% |
| #401 | 5 | 1333 → 1435 | +7% |
| #431 | 3 | 1210 → 1313 | +8% |
| #434 | 2 | 416 → 457 | +9% |
| #429 | 3 | 1317 → 1450 | +10% |
| #416 | 2 | 1017 → 1151 | +13% |
| #432 | 2 | 372 → 425 | +14% |
| #433 | 1 | 5 → 6 | +20% |
| #404 | 2 | 353 → 499 | +41% |
| #400 | 4 | 191 → 277 | +45% |
| #444 | 7 | 905 → 1366 | +50% |
| #427 | 6 | 303 → 549 | +81% |
| #445 | 3 | 67 → 198 | +195% |

Median review-driven growth across these 16 PRs is about 11%; #433's +20% is one
line on a six-line diff, not a signal. The tail is where the cost sits, and it tracks
cycle count, not PR size.

Zodiac shows a different shape: every one of the 15 swept PRs settled on review
cycle 1 — 14 approved, and #150 blocked for a human-only screenshot criterion the
container could not satisfy. **Alucard added zero lines after worker head on all 15.**
Every one of the 319 post-worker lines is authored by `aldovc`, on 8 of the 15 PRs,
almost all of it the next morning: `docs/verification/pr<N>/` READMEs and browser
screenshots the sandbox could not capture, because it has no headless-Chromium
shared libraries and no root to install them. PR bodies label these sections
"Review verification — September 7" and "Review completion — September 7".

That is a finding in its own right, and not one the spec anticipates: zodiac's
review cost is not paid by the reviewer agent at all, it is paid by the maintainer
the next day, and the cause is a missing container dependency rather than anything
in a prompt. Whether the silent reviewer reflects UI work generating fewer real
defects than backend trust-boundary work, or the reviewer being less effective on
TSX, this audit cannot say — but it means zodiac cannot test a reviewer-side change.

## Finding 2 — most reviewer findings are real; a small, identifiable minority are style

36 findings were raised across 13 family-brain PRs (8 High, 25 Medium, 3 Low). Of the
30 with a parseable problem statement, roughly 23 are correctness, security, or
trust-boundary defects — untrusted Home Assistant strings reaching model-facing tool
results, an allowlist that missed the `cover` domain, a down-migration that would fail
after any delete, a pending-confirmation container rename that would strand live rows.
These are exactly the findings the spec says to preserve.

The style minority is small but concretely traceable to prompt text:

- **#427 cycle 2** blocked the merge over `3`, `0`, and `255` appearing as literals
  next to existing named schema bounds. Severity Low. Cost: one full review +
  feedback + CI cycle; the resulting commit was +13/-5 with no test change.
- **#401** raised a Low finding that a function-body import lacked a comment naming
  the circular import it avoids.
- **#427 cycle 3** required the `turn_on` restriction to appear in the tool schema
  description and the `## Home` system prompt. The fix was reasonable, but the test
  it produced asserts exact prose substrings of both the schema description and the
  system prompt, and needed a follow-up commit (`normalize Home prompt guidance
  assertion`) to stop being brittle.

The prompts these came from say so literally. `alucard-worker-prompt.md`: "No logic
block appears in two places. If it does, extract it."; "Every numeric or string
literal that carries domain meaning is a named constant."; "Any `import` inside a
function body has a comment naming the circular-import it avoids."; "Repeat per
criterion". `alucard-reviewer-prompt.md` mirrors them: "Duplicated logic … The fix is
extraction, not tolerance."; "Magic numbers … must be named constants";
"Stringly-typed dispatch … where an enum or polymorphism would make invalid states
unrepresentable." The spec's premise for section 2 is confirmed at the source.

Separately, a family of commits — #427 `preserve parser lint complexity`, #444
`reduce cover validation complexity`, `preserve create routine complexity limit`,
`preserve automation validation complexity` — is caused by ruff `C90` being enabled
in `backend/pyproject.toml`, not by any Alucard prompt. Attribution: repository
convention. Every added validator pushes a function toward the complexity ceiling and
buys a restructuring commit. That is the maintainer's call, not a policy target.

## Finding 3 — the dominant avoidable cost is review-cycle count, not lines

PR #427 is the clearest case. Six review cycles, five feedback rounds. The findings
were not scope creep — each cycle found a genuinely new instance of one class,
untrusted strings reaching model-facing content, in a new place: first the provider
error, then a value echoed back to the caller, then an externally sourced identifier. The
reviewer looked in one place per cycle.

What that cost, for one PR:

| Role | invocations | tool output | tokens (see limits) | wall |
| --- | ---: | ---: | ---: | ---: |
| worker | 1 | 118 KB | 6.09 M | 437 s |
| review | 6 | 1,216 KB | 2.48 M | 543 s |
| feedback | 5 | 2,134 KB | 2.89 M | 479 s |

The review loop ran 2.9× the worker's wall time and read 28× its tool output, to add
246 lines. PR #444 repeated the pattern — six cycles, each finding one more hole in
the same automation-action allowlist, spread across two runs because the second half
ran under `alucard continue`.

Aggregated over the 27–30 August family-brain runs:

| Role | invocations | tool output | per invocation |
| --- | ---: | ---: | ---: |
| worker | 20 | 3.9 MB | 199 KB |
| review | 50 | 11.2 MB | 229 KB |
| feedback | 30 | 12.1 MB | 414 KB |

Twenty worker invocations cost $66.81 (claude, measured). The 80 review and feedback
invocations cost an unknown amount (codex, unreported) and read 6× the worker's tool
output. Wall time splits worker 13,508 s / review 4,265 s / feedback 3,080 s, plus a
CI green-wait between every cycle.

The single most surprising number: **the feedback agent reads more than twice as much
per invocation as the worker** (414 KB vs 199 KB), while fixing one or two findings
in code the worker just wrote.

## Finding 4 — repeated reading is real, concentrated in hub files

For PR #427, the repository's largest hub module (6,239 lines at
the time) appeared 21 times in the worker's tool inputs and 82 times across the 11
review and feedback invocations. The reviewer is already disciplined about it — it
reads named ranges (`sed -n '1380,1540p'`), not whole files — but it re-derives
which ranges to read on every cycle, alongside a fresh `gh pr view`, `gh pr diff`, and
a static conventions document read each time.

Worker orientation is also measurable: bytes of tool output consumed before the first
`Edit`/`Write` were 61%, 70%, 82%, 69%, 68%, 81%, and 37% of each run's total across
seven sampled worker invocations. Some of that is irreducible — reading the issue and
the file about to be changed — but the #427 worker also spent turns on four attempts
to read issue 418 through a transport failure, two `find` sweeps to locate source
files, and five commands establishing whether the repo builds through `Justfile`,
`backend/justfile`, `uv`, or `poetry`.

Toolchain discovery and entry-point location are the parts a navigation record could
actually remove. Re-reading the diff is not.

Sweeping every invocation in both windows — 100 agent invocations over 11
family-brain runs, 32 over 3 zodiac runs — gives the shape of it. Files touched
per invocation, counting each file once however many times a single invocation
reads it:

| Repository | worker | review | feedback |
| --- | ---: | ---: | ---: |
| family-brain | 33.9 files (20 inv) | 11.8 files (50 inv) | 7.4 files (30 inv) |
| zodiac | 25.9 files (14 inv) | 12.9 files (15 inv) | 10.0 files (1 inv) |

Redundancy is then how many *separate* invocations in the same run open the same
file. family-brain averages 1.84, with 83 files opened by four or more invocations
and 19 by eight or more. Zodiac averages 1.41 and has nothing above four. The gap
is review cycles: zodiac approves on cycle 1, family-brain does not.

The worst offenders are all in `alucard-20260828-194928`, the run that produced
#427, #428, and #429: the hub module opened by 23 separate invocations, the
conventions document by 18, its sibling entry-point module by 16. The conventions doc
is the cheapest possible win — it is a static repo document that cannot change
during a run, and 18 agents each spent a read establishing what the previous one
already knew.

## Simplification candidate register

| # | Location / PR | Protects | Unnecessary part | Proposed change | Verification to retain | Class | Confidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | A prompt-guidance assertion in the touched test module (family-brain #427) | Model is not told two optional fields are rejected on some actions | Exact-substring assertions against the schema description *and* the `## Home` system prompt prose | Assert the schema exposes both properties; drop the prompt-prose substring match | `test_rejects_rgb_color_for_non_turn_on_light_actions`, `test_rejects_effect_for_non_turn_on_light_actions` already cover the behaviour | safe to simplify | high |
| 2 | Numeric bound constants in the hub module (family-brain #427) | Schema bounds and runtime validation agreeing | Making a Low-severity literal-extraction a merge blocker | Keep the constants; stop letting this class block merge | existing rgb range tests | policy target, not a code change | high |
| 3 | Deferred-import comment finding (family-brain #401) | Nothing at runtime | Requiring a comment naming the circular import | Drop as a blocking rule | none needed | policy target | high |
| 4 | Two malformed-input rejection tests (family-brain #427) | Bad input rejected before the provider call | Two tests asserting the same message through the same branch | Parameterize into one | same assertions | safe to simplify | medium |
| 5 | Complexity-churn commits (family-brain #427, #444) | ruff `C90` gate | Nothing — repo-chosen lint | No change | — | justified complexity | high |
| 6 | `docs/verification/pr<N>/` READMEs + screenshots (zodiac #143, #145, #146, #147, #150, #153) | Visual acceptance criteria the maintainer set | Nothing agent-side; 12–44 lines plus binaries per PR accumulate in-repo | No change without a maintainer decision on retention | — | requires a product decision | high |
| 7 | family-brain #445 (+195% growth) | Stale README env var, untested JSONB decode path, retired config fields still live in the catalog | Nothing | No change — largest relative growth in the sample was fully justified | all three added tests | justified complexity | high |
| 8 | zodiac #144 (158 test lines, 4 implementation lines) | Message sequence, min-delay completion, rejection fallback, unmount cleanup | Nothing material; the four timer steps could be one loop | Optional parameterization only | all five tests | justified complexity | medium |

## Instrumentation

Section 3's measurement artifact is implemented, since the pilot arms cannot be
compared without it and reconstructing this audit from timestamps was the slowest
part of the work. `logs/alucard-*/measurements.jsonl` carries one `run` record
(harness revision, prompt digest, image ID, per-role provider/model), one `stage`
record per head-moving stage (`worker`, `cifix`, `feedback`), and one `iteration`
record (CI result, verdict, cycles, elapsed, tokens by role and cache category).
Dispatched prompts are archived under `logs/alucard-*/prompts/`. Both are local
only. `README.md` documents the format; `test/test_measurements.sh` covers it.

Five things the audit learned are encoded as behaviour rather than intention:

- **Stages are pinned to the base SHA, not chained.** A feedback stage that reverts
  a worker stage must cancel, not add. `from_prev` is recorded separately for
  attributing one stage's work.
- **`cost` is `null`, never `0`, for a provider that reports none.** Every reviewer
  and feedback figure in this report is missing its cost because codex emits no
  cost field, and a `0` there would have read as free. `cost_complete` needs
  *every* invocation to have reported a cost, and an invocation whose log has no
  parseable usage at all still counts — those are timeouts and transport drops,
  the attempts most likely to be missing a usage record and least safe to drop
  from a total.
- **Records carry `repo_id`, and lookups match on it.** PR numbers are
  repository-local while all three repositories share one logs directory, so
  matching a stored base by PR number alone reads another repository's records.
- **Binary files are counted as files, never as lines.** Zodiac's per-PR screenshots
  would otherwise have arrived as line growth.
- **Changed paths are kept in every record, and renames keep their full path.**
  The bucket heuristics are guesses and were already wrong once: the first version
  classified zodiac's `docs/spec/` requirements as tests. Renames are read from
  git's NUL-delimited output rather than its display notation, because
  a `{unit => integration}` rename path parsed as display text loses the
  prefix and books a moved test as implementation.
- **A diff that could not be taken says so.** An unresolvable ref returns
  `{"unavailable": true, "reason": …}`, never `files:0, added:0`, which would read
  as a stage that genuinely changed nothing.

Two further defects were fixed after review: an ordinary *formal* approval
recorded an empty verdict and zero cycles, because the outcome was written only
on the branch where the reviewer's decision file overrides GitHub; and a
`continue` run re-derived its base from the PR's current `baseRefOid`, which
advances as other PRs merge, so growth measured against it could include changes
the PR never made. Continued runs now recover the base an earlier run pinned,
fall back to the merge base, and label which of the two they used. They also
write a `run` record, which they previously lacked entirely.

A measurement failure is logged and skipped. That guarantee needed hardening: the
first version took the whole run down from inside `ci_gate`, which re-enables
`set -e` immediately before recording a stage, when `pipefail` turned an
unresolvable base ref into a failed pipeline. There is a regression test, and it
has to run in a real child process — bash suppresses `set -e` inside a subshell
that is part of a `||` list, so the obvious form of that test passes on broken code.

Replaying PR #427 through the finished instrumentation reproduces this report's
hand-computed figures exactly: worker head `tests +183 / impl +120` (303 total),
final head `tests +375 / impl +174` (549 total).

## Does the baseline support the proposed changes?

**Section 2, shared engineering policy — supported, but weight it toward the worker.**
The blanket extraction, magic-value, and per-criterion rules exist verbatim in both
the worker and reviewer prompts, and two findings in this window were raised solely
because of them, with a third (the schema/prompt wording on #427) partly so. The
reviewer-side payoff is small: those rules account for about 3 of 30 analysable
findings, a minority of the 11% of lines review revisions contribute at all. The
worker-side half is where the lines are — 89% of everything Alucard wrote — so
replacing "Repeat per criterion" and the two blanket rules in
`alucard-worker-prompt.md` is the part of section 2 that can move a diff. Ship the
reviewer half too, because it is cheap and removes demonstrated merge-blocking
noise, but do not expect it to shrink anything.

**Missing from section 2 — the change this baseline actually argues for.** Neither
prompt asks the reviewer to sweep a finding's whole class before reporting. #427 and
#444 each spent six cycles discovering one instance at a time of a single class, and
cycle count is what correlates with growth and cost. A reviewer instruction to
enumerate every instance of a class it has found — every site where untrusted data
reaches model-facing content, every branch of an allowlist — before writing the
verdict would have collapsed six cycles into roughly two on both PRs. On #427 that is
4 review and 4 feedback invocations, 2.4 MB of tool output, and about 13 minutes of
agent wall time plus four CI waits. This should be added to section 2, and it is a
better first pilot arm than the policy fragment alone.

**Section 3, measurement — supported, and now implemented** (see Instrumentation).
Building this report surfaced exactly the failure modes the spec anticipates: "first
commit" is not the worker head (workers commit incrementally, so the first commit
understates worker output by up to 546 test lines on #431); PR growth spans runs, so
accounting must key on PR and join `continue` runs (#444's second review round lives
in a separate log directory, which is why the `continue` path records against the
PR's own base); and cost is genuinely absent for every codex role. One requirement
the spec did not have, added because reconstructing attribution from timestamps was
the slowest part of this audit and got zodiac wrong on the first pass: each stage
record stamps the actual head SHA at that gate, so attribution is never inferred.

**Section 4, orientation reuse — partially supported, with the emphasis on the wrong
role.** The spec frames the record as worker → later roles. The measurements say the
biggest per-invocation reader is the *feedback* agent (414 KB, vs 199 KB for the
worker that wrote the code), and the most repetitive reader is the reviewer across
cycles of the same PR. Both are later roles, so the mechanism fits — but the record's
contents should be chosen for them: verification commands and toolchain layout
(the #427 worker burned five commands finding the build tool), and entry points into
hub files like that 6,239-line module. The single clearest candidate is
static repo documentation: one conventions file was opened by 18 separate invocations in
one run and cannot change while that run is in flight. Keep it behind its flag and
behind the policy pilot, as the spec says.

**Unbudgeted, and larger than either: zodiac's browser evidence.** Eight of 15
zodiac PRs needed a human the next morning because the container has no
headless-Chromium shared libraries. That is the entire measured review cost in that
repository, it is a missing container dependency rather than a prompt, and no item
in the spec addresses it. It belongs in the roadmap ahead of orientation reuse.

**Section 2's "exclude optional simplifications from findings sent to feedback" —
unsupported by this window.** The leak is real in code: `alucard:1618` passes the
entire review body, `Out of scope (follow-up)` section included, into
`<review_findings>`. But no family-brain review in this window emitted an
out-of-scope section alongside CHANGES_REQUESTED; the only one observed (zodiac #150)
accompanied a BLOCKED verdict, so no feedback agent ever saw it. Cut this item or
demote it to a one-line note, rather than spending prompt budget on it.

**Section 2's CI-fix concern — already handled.** `alucard-ci-fix-prompt.md` already
says "Do not touch code unrelated to the CI failure" and "Edit only the files needed
to fix the failing checks." The real risk is that adding shared simplicity policy to
this role *weakens* an adequate guard. Consider excluding CI-fix from the fragment.

## Recommended pilot task selection

The spec asks for one behaviour change on existing helpers, one test-heavy
application change, and one configuration/docs change, chosen after this audit.
family-brain's device-control domain extensions (the #402/#403/#427 shape) are
the right behaviour-change slot: they are bounded, they exercise the trust boundary
that generates most real findings, and there are four comparable historical runs to
sanity-check against. Zodiac supplies the test-heavy slot, and its silent
reviewer is a feature there: with review-driven growth at zero, any change in test
volume is attributable to the worker policy alone. Drop home-cluster from the paired
pilot — one logged PR cannot support a comparison — and use a configuration/docs task
in zodiac or family-brain for the third slot instead.

Two arms are worth running, in this order: the reviewer class-sweep instruction
first, because it is one paragraph and the measurements point at it hardest, then the
shared policy fragment. Running them together would make an already-noisy three-task
comparison uninterpretable.
