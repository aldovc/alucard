---
name: to-issues
description: Break a plan, spec, or PRD into independently-grabbable GitHub issues or a local Alucard tasks file, using tracer-bullet vertical slices. Use when user wants to convert a plan into issues, create implementation tickets, or break down work into issues or tasks.
---

# To Issues

Break a plan into independently-grabbable units of work using vertical slices (tracer bullets), labeled and sized so an autonomous worker loop can pick them up. The slicing and quiz flow (steps 1-4) is identical regardless of where the result is published; step 5 picks the target and step 6 publishes to it.

## Process

### 1. Gather context

Work from whatever is in the conversation. If the user passes an issue number or URL, fetch with `gh issue view <number>` (with comments).

### 2. Explore the codebase (optional)

If you haven't already, explore to understand current state.

### 3. Draft vertical slices

Each issue is a thin vertical slice cutting through **every layer relevant to that slice** end-to-end — NOT a horizontal slice of one layer. Which layers apply depends on the slice (could be schema/API/UI/tests, could be just script/test, could be just config/docs).

Each slice is **AFK** or **HITL**:
- **AFK** — implementable and mergeable without human interaction. Crisp acceptance criteria, no architectural ambiguity, no design judgement.
- **HITL** — needs a human. Architectural decision, design review, ambiguous tradeoff, anything where "just pick" isn't safe.

Prefer AFK. But **if you find yourself stretching to justify an AFK label, it's HITL.** An AFK issue the worker gets stuck on mid-run is worse than an honest HITL you handle when you check back.

<vertical-slice-rules>
- Each slice delivers a narrow but COMPLETE path through every relevant layer
- A completed slice is demoable or verifiable on its own
- Prefer many thin slices over few thick ones
- AFK slices fit in a single agent session — roughly ≤10 file changes or ≤30 minutes of focused work. If bigger, split.
</vertical-slice-rules>

### 4. Quiz the user

Present as a numbered list. For each slice:

- **Title**
- **Type**: AFK or HITL
- **Blocked by**: dependent slice numbers, if any
- **Estimated size**: rough file count or time
- **User stories covered**: if source material has them

Ask:

- Granularity right? (too coarse / too fine)
- Dependencies correct?
- Any slices to merge or split?
- AFK/HITL labels honest, or are AFKs really HITLs in disguise?
- Do AFK slices look small enough to finish in one agent session?

Iterate until approved.

### 5. Choose the publish target

Two targets exist: **GitHub issues** (default) or a **local Alucard tasks file** (`.alucard/tasks.md` in the target repo — the format Alucard's `alucard run --tasks` / auto-detect consumes; see the Alucard README's "Local task source" section for the full grammar).

Decide before publishing, in this order:

- The user named a target explicitly — use it.
- The target repo already has a `.alucard/tasks.md` — prefer appending to it, but confirm with the user rather than silently switching away from GitHub.
- Neither signal is present and both are plausible — ask: "Publish as GitHub issues or a local tasks file (`.alucard/tasks.md`)?"

### 6. Publish

#### GitHub target

Apply the type label so the worker loop can filter:

```bash
gh issue create \
  --title "<title>" \
  --label "<afk|hitl>" \
  --label tracer-bullet \
  --body "<body from template below>"
```

Create in dependency order (blockers first) so you can reference real issue numbers in dependents' "Blocked by" sections.

<issue-template>
## Type

AFK  *(or HITL)*

## Parent

#<parent-issue-number> *(omit if no parent)*

## What to build

Concise description of the end-to-end behavior. Not layer-by-layer implementation.

## Acceptance criteria

These are the definition of done. The worker ticks them as it completes them and will not close the issue if any remain unticked.

- [ ] Criterion 1
- [ ] Criterion 2
- [ ] Criterion 3

## Blocked by

- Blocked by #<issue-number>

Or `None — can start immediately`. The worker harness parses this section verbatim to skip blocked issues, so use the exact `Blocked by #N` format.

## Notes

Relevant files, gotchas, prior art — anything to save the worker exploration time.

</issue-template>

Do NOT close or modify any parent issue.

#### Local target

Write or update `.alucard/tasks.md` in the target repo instead of creating GitHub issues.

- **New file:** create it with the plan summary as the header — the freeform markdown above the first task heading. This block is the parent context, injected verbatim into every worker and reviewer prompt, so put the goal, constraints, and key decisions here instead of repeating them per task.
- **Existing file:** append new tasks after the existing ones. Treat the existing header as canonical unless the user asks to revise it. Number new tasks continuing from the highest existing id — never reuse or renumber an existing id.
- Assign each slice a stable id (sequential integers are simplest: `1`, `2`, ...) and write slices in dependency order — file order is queue order, and a task can only be blocked by an id defined earlier in the file.
- AFK slices become `## [ ] <id>: <title>`. HITL slices become `## [h] <id>: <title>` — never queued, but recorded in the same file so the whole plan lives in one artifact.
- The body is the same content as the GitHub template minus `## Type` and `## Parent` (state and file membership already encode those): `## What to build`, `## Acceptance criteria`, `## Notes`, plus a bare `Blocked by:` line.

<local-task-template>
```
## [ ] <id>: <title>

## What to build

Concise description of the end-to-end behavior. Not layer-by-layer implementation.

## Acceptance criteria

- [ ] Criterion 1
- [ ] Criterion 2
- [ ] Criterion 3

Blocked by: <id>, <id>

## Notes

Relevant files, gotchas, prior art — anything to save the worker exploration time.
```
</local-task-template>

Use `## [h] <id>: <title>` in place of `## [ ] <id>: <title>` for HITL slices. `Blocked by:` takes comma-separated task ids, or the literal `none` — it must start at column 0 exactly as written (`Blocked by:`, no `##` heading around it) for the parser to find it; everything else in the body is freeform and ignored by the parser.

Before finishing, re-check the file against the grammar: no duplicate ids, every `Blocked by:` id refers to a task defined earlier in the file, the header isn't empty, every heading matches `## [ >xh] <id>: <title>` exactly. If the `alucard` CLI is available, confirm with `alucard doctor --tasks <target-repo>/.alucard/tasks.md <target-repo>` (or from inside the target repo, `alucard doctor`, which auto-detects the file) — it reports the same problems with line numbers.

Do NOT create GitHub issues for a plan published locally, and do NOT close or modify any parent issue.
