# Mode: GitHub issues

The queue is GitHub Issues. Context section:

- `<issues>` — JSON array of open, unblocked, non-WIP AFK issues. Each entry carries only `number`, `title`, and `labels` (name strings) — no body. Fetch the body of the issue you pick with `gh issue view <N> --comments`; do not assume the queue carries it.

The harness has already filtered HITL, in-progress, blocked, and WIP issues. Trust the queue.

## Termination

If `<issues>` is empty, output `<promise>NO MORE TASKS</promise>` and stop. (The harness also checks queue length; this is a backup signal.)

## Task selection

Pick ONE issue. Priority order:

1. Critical bugfixes
2. Development infrastructure (tests, types, dev scripts) — these unblock everything else
3. Tracer-bullet feature slices — thin vertical slices through every relevant layer
4. Polish and quick wins
5. Refactors

**ONE TASK PER ITERATION.** Do not bundle.

## Claim the issue

First action — label it so no parallel iteration grabs it:

```bash
gh issue edit <N> --add-label in-progress
```

Then fetch the issue including comments (`gh issue view <N> --comments`) before starting — prior run notes, blockers, and partial work are often there. Read the acceptance criteria, parent (if any), and linked issues.

## Task reference

- Commit messages reference the issue as `Closes #N` or `Refs #N`.
- The PR body's first line MUST be `Closes #N` — that is what auto-closes the issue on merge and what queue deduplication keys on.

## Close out

If every acceptance criterion is genuinely done:
- Tick all `- [ ]` boxes in the issue body via `gh issue edit <N> --body-file <file>` — an issue body is always multi-line, so `--body "..."` would post literal `\n` sequences
- Remove `in-progress` label
- The PR's `Closes #N` will close the issue on merge

If the task is partial:
- Comment on the issue: what's done, what remains, blockers — again via `--body-file` for anything multi-line
- Remove `in-progress` so the next iteration can resume
- Still open the PR with the partial work

## Mode rules

- **Never** close an issue with unticked acceptance criteria
- **Never** pick a HITL issue — if one slipped past the filter, comment on it noting the misfiled label and pick a different issue
