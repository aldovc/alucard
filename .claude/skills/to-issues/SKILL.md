---
name: to-issues
description: Break a plan, spec, or PRD into independently-grabbable GitHub issues using tracer-bullet vertical slices. Use when user wants to convert a plan into issues, create implementation tickets, or break down work into issues.
---

# To Issues

Break a plan into independently-grabbable GitHub issues using vertical slices (tracer bullets), labeled and sized so an autonomous worker loop can pick them up.

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

Prefer AFK. But **if you find yourself stretching to justify an AFK label, it's HITL.** An AFK issue the worker gets stuck on at 3am is worse than an honest HITL you handle in the morning.

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

### 5. Create the GitHub issues

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
