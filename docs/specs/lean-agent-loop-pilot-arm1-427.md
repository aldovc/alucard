# Arm 1, second pair — the sweep arm approved a PR with the defect still in it

Date: 2026-09-09. Seed: replay of family-brain #427's worker head. Both arms ran
to a verdict. **The sweep arm finished in four fewer cycles and left two of the
four defect sites open.**

Read [the first pair](lean-agent-loop-pilot-arm1-results.md) first; this one
answers a different question and reverses the direction of the result.

## Two things this pair is not

**It did not test the shipped prompt.** The sweep section ships with two worked
examples, and one of them is #427 itself — the class named outright and three of
its sites listed. Left in, the sweep arm would have been reading the answer.
Both examples were replaced with neutral ones for this pair
(`pilot/arm1-sweep-427`, tool rev `5b5e66f3`), so what ran is the instruction
without its illustrations. See the contamination precondition in the runbook.

**The historical run is not the control.** #427's original six-cycle loop ran on
2026-08-28 under a different tool revision, different prompts and different
models. It is what told us the seed contains a repeated defect. Nothing more.

## What ran

One worker head, replayed at no worker cost, forked into two arms that differ in
one file.

| | control | sweep |
|---|---|---|
| PR | [#465](https://github.com/aldovc/family-brain/pull/465) | [#466](https://github.com/aldovc/family-brain/pull/466) |
| Tool rev | `d4b62c0`, tag `pilot/arm1-427-control-tool` | `5b5e66f3`, tag `pilot/arm1-427-sweep-tool` |
| Shared head | `7d35eac8` | `7d35eac8` |
| Final head | `8bf11fcc` | `ad4ac324` |
| Review cycles | **7** | **3** |
| Feedback rounds | 6 | 2 |
| Verdict | APPROVED, cycle 7 | APPROVED, cycle 3 |
| Added above the head | 3 files, +325/−27 | 4 files, +152/−18 |
| Turns | 245 | 88 |
| Cost | **$2.979** | **$0.947** |
| Backend CI at final head | success | success |

Cost counts the feedback agent only; the reviewer runs on codex, which reports
none. Base pinned at `pilot/427-base`, image
`sha256:25b08ef1…`, 30-minute timeout, 10-cycle ceiling, both arms sequential.

## On the raw numbers the sweep arm wins by a mile

Less than a third of the cycles, a third of the cost, a third of the turns, half
the lines, no sprawl — its fourth file is a test module, not an excursion
outside the PR. If the pair were scored here it would be the efficiency win the
arm was designed for.

It is the wrong place to score. The runbook says so twice, and this is the
second pair in a row where the cheap-looking arm was the worse one.

## The defect, written down before either arm reported

Recorded in the run's log directory while the control arm was still on cycle 1,
so that neither arm's behaviour could shape what counted as the answer. One
class — untrusted text reaching model-facing tool output — at four sites, all
inside the one function the diff adds, all leaving through the same error path.
Three distinct sources: an external service's error text, an identifier read
from that service's catalog, and the caller's own input echoed back.

The sites themselves are recorded with the run artifacts rather than here, since
the target repository is private.

## What each arm did with it

The control arm found the class four separate times, one site per cycle, its
fourth finding being that its own earlier fix had been incomplete. Exactly the
habit the instruction exists to break, reproduced under the current tool
revision rather than inferred from August's logs.

The sweep arm raised four findings in cycle 1 — matching what the control arm
took until cycle 6 to accumulate — and then approved on cycle 3. But it read one
of the sites as a type-safety problem, fixed it as one, and left the underlying
interpolation in place. It never raised a third site at all.

So it was broader per cycle and still did not sweep the class. Reporting more
findings at once is not the same behaviour as enumerating one class's sites,
and this pair separates them cleanly.

## Settled independently, not by reading the reviews

An observer-only probe, kept with the run artifacts, sends an instruction-shaped
string through both paths and asserts it never reaches model-facing output. It
is not part of either arm. Run in the pinned image against three heads:

| Head | Result |
|---|---|
| Shared worker head `7d35eac8` | **2 failed** — the defect is real and present |
| control final `8bf11fcc` | **2 passed** |
| sweep final `ad4ac324` | **2 failed** |

The sweep arm's final head is no better than the seed on this class. Its CI is
green and its reviewer approved it.

The control arm's fix is not a token escape either: it moves the untrusted text
out of model-facing output entirely, adds a bounded allowlist with a stated
rationale, and then notices in cycle 5 that a prompt elsewhere still promised
behaviour the code no longer had, and corrects it. That coherence is what the
extra cycles and the extra 173 lines bought.

## Verdict

**A quality loss at lower cost — the mirror image of the first pair.** Pair one
had the sweep arm catching a defect the control missed, at higher cost. Pair two
has it missing defects the control caught, at lower cost. Two pairs, opposite
directions, one stochastic run each. Neither is evidence of a causal prompt
effect, and the two together are not a wash that cancels out; they are two
samples that say the effect is not yet distinguishable from run-to-run noise.

What this pair does support, because it is a difference in kind rather than
degree: **the instruction on its own did not produce sweeping.** The reviewer
was told to enumerate a class before reporting it and did not, on a seed that
contains a textbook instance. The examples are the part of that section that
names classes concretely, and they were what this pair removed.

The cheapest reading is that the worked examples were doing the work. If that is
right, the arm as shipped is closer to two repo-specific checklist items than to
a general instruction — which is a different, cheaper, and more directly
testable thing to build, and it belongs in **Mechanical checks** rather than in
a paragraph of prose.

That is a hypothesis this pair suggests and does not establish. Testing it needs
a third arm — instruction plus *neutral* examples — on a seed that neither the
instruction nor the examples describe.

## Do not

- Merge #465 or #466. Both are pilot artifacts on a pinned base.
- Read the 7→3 cycle drop as a saving. It is a saving only if the work skipped
  did not need doing, and the probe shows it did.
- Read this as a verdict on the paragraph *with* its examples. It was run
  without them, and this pair is the reason the paragraph is gone from
  `alucard`'s reviewer prompt altogether — replaced by two
  mechanical checks in `a90cccf`. Whether the examples alone would have worked
  is still untested.

## Artifacts

Each arm's exact tool revision is tagged in this repository —
`pilot/arm1-427-control-tool` and `pilot/arm1-427-sweep-tool` — so the two
prompts that produced these results stay recoverable after the pilot worktrees
were removed. The control tag is the prompt as of `e8df226`, before the
class-sweep section existed.

Everything that quotes the private target repository is held locally under
`logs/` and `.alucard/`, both gitignored: the ground truth, the seed diff, the
observer probe and its output against all three heads, both run logs, each arm's
measurements, events, raw agent transcripts and dispatched prompts, and the
pairing records.

Both pairs ran on image `sha256:25b08ef1…`, which `alucard build` removed as
superseded when headless Chromium was added later the same day. The digest
stands as provenance; an exact re-run would need that image rebuilt, and the
conclusions here do not depend on one.
