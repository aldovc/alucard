---
name: to-tickets
description: Break a spec into GitHub tickets or a local Alucard tasks file. Use when user wants to convert a spec into tickets, create implementation tickets, or break down work into tickets or tasks.
disable-model-invocation: true
---

# To Tickets

Break a spec into **tickets**, each small enough for one agent session and complete enough to merge on its own. The drafting and quiz flow (steps 1-4) is identical regardless of where the result is published; step 5 picks the target and step 6 publishes to it.

## Process

### 1. Gather context

Work from whatever is in the conversation. If the user passes a ticket number or URL, fetch with `gh issue view <number>` (with comments).

### 2. Explore the codebase (optional)

If you haven't already, explore to understand current state. If the project has a domain glossary, ticket titles and descriptions should use its vocabulary.

### 3. Draft tickets

Each ticket should be complete and mergeable on its own: whatever layers it needs (schema, API, UI, tests, or just a script), not one layer of a larger change.

Each ticket is **ready-for-agent** or **ready-for-human**:

- **ready-for-agent** — implementable and mergeable without human interaction. Crisp acceptance criteria, no architectural ambiguity, no design judgement.
- **ready-for-human** — needs a human. Architectural decision, design review, ambiguous tradeoff, anything where "just pick" isn't safe.

Prefer ready-for-agent. If you find yourself stretching to justify that label, it's ready-for-human. An agent-labeled ticket the worker gets stuck on mid-run is worse than an honest human ticket you handle when you check back.

- A finished ticket is demoable or verifiable on its own
- Prefer many small tickets over few large ones
- ready-for-agent tickets fit in a single agent session, roughly ≤10 file changes or ≤30 minutes of focused work. If bigger, split.

Give each ticket its **blockers**: the other tickets that must complete before it can start. A ticket with no blockers can start immediately.

### 4. Quiz the user

Present as a numbered list. For each ticket:

- **Title**
- **Type**: ready-for-agent or ready-for-human
- **Blocked by**: dependent ticket numbers, if any
- **What it delivers**: the end-to-end behaviour this ticket makes work

Ask:

- Granularity right? (too coarse / too fine)
- Dependencies correct?
- Any tickets to merge or split?
- Labels honest, or are ready-for-agent tickets really ready-for-human in disguise?
- Do ready-for-agent tickets look small enough to finish in one agent session?

Iterate until approved.

### 5. Choose the publish target

Two targets exist: **GitHub tickets** (default) or a **local Alucard tasks file** (`.alucard/tasks.md` in the target repo — the format Alucard's `alucard run --tasks` / auto-detect consumes).

Decide before publishing, in this order:

- The user named a target explicitly — use it.
- The target repo already has a `.alucard/tasks.md` — prefer appending to it, but confirm with the user rather than silently switching away from GitHub.
- Neither signal is present and both are plausible — ask: "Publish as GitHub tickets or a local tasks file (`.alucard/tasks.md`)?"

### 6. Publish

#### GitHub target

Apply the type label so the worker loop can filter. If `ready-for-agent` or `ready-for-human` does not exist yet, create it once (`gh label create ready-for-agent --color 0E8A16 --description "Ready for an agent to pick up"` / `gh label create ready-for-human --color FBCA04 --description "Needs a human; Alucard will not pick this up"`), then retry.

```bash
gh issue create \
  --title "<title>" \
  --label "<ready-for-agent|ready-for-human>" \
  --body-file <path>
```

Write the body to a file in the OS temp directory first. Create in dependency order (blockers first) so you can reference real ticket numbers in dependents' "Blocked by" sections.

<issue-template>
## Parent

#<parent-ticket-number> *(omit if no parent)*

## What to build

Concise description of the end-to-end behavior. Not layer-by-layer implementation.

## Acceptance criteria

These are the definition of done. The worker ticks them as it completes them and will not close the ticket if any remain unticked.

- [ ] Criterion 1
- [ ] Criterion 2
- [ ] Criterion 3

## Blocked by

Blocked by #<ticket-number>

Or `None — can start immediately`. The worker harness parses blockers as the literal `Blocked by #N` on one line, or as `#N` tokens under this heading. Do not write a bare `- #N` list and assume that is enough unless the `#N` sits in this section.

## Notes

Relevant files, gotchas, prior art — anything to save the worker exploration time.

</issue-template>

Do NOT close or modify any parent ticket beyond adding sub-issue links.

#### Local target

Write or update `.alucard/tasks.md` in the target repo instead of creating GitHub tickets.

- **New file:** create it with the spec summary as the header — the freeform markdown above the first task heading. This block is the parent context, injected verbatim into every worker and reviewer prompt, so put the goal, constraints, and key decisions here instead of repeating them per task.
- **Existing file:** append new tasks after the existing ones. Treat the existing header as canonical unless the user asks to revise it. Number new tasks continuing from the highest existing id — never reuse or renumber an existing id.
- Assign each slice a stable id (sequential integers are simplest: `1`, `2`, ...) and write slices in dependency order — file order is queue order, and a task can only be blocked by an id defined earlier in the file.
- ready-for-agent slices become `## [ ] <id>: <title>`. ready-for-human slices become `## [h] <id>: <title>` — never queued, but recorded in the same file so the whole spec lives in one artifact.
- The body is the same content as the GitHub template minus `## Parent` (state and file membership already encode type): `## What to build`, `## Acceptance criteria`, `## Notes`, plus a bare `Blocked by:` line.

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

Use `## [h] <id>: <title>` in place of `## [ ] <id>: <title>` for ready-for-human slices. `Blocked by:` takes comma-separated task ids, or the literal `none` — it must start at column 0 exactly as written (`Blocked by:`, no `##` heading around it) for the parser to find it; everything else in the body is freeform and ignored by the parser.

Before finishing, re-check the file against the grammar: no duplicate ids, every `Blocked by:` id refers to a task defined earlier in the file, the header isn't empty, every heading matches `## [ >xh] <id>: <title>` exactly. If the `alucard` CLI is available, confirm with `alucard doctor --tasks <target-repo>/.alucard/tasks.md <target-repo>` (or from inside the target repo, `alucard doctor`, which auto-detects the file) — it reports the same problems with line numbers.

Do NOT create GitHub tickets for a spec published locally, and do NOT close or modify any parent ticket.
