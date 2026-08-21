# Mode: GitHub tickets

The queue is GitHub Issues labeled `ready-for-agent`. Context section:

- `<issues>` — JSON array of open, unblocked, non-WIP `ready-for-agent` tickets. Each entry carries only `number`, `title`, and `labels` (name strings) — no body. Fetch the body of the ticket you pick with `gh issue view <N> --comments`; do not assume the queue carries it.

The harness has already filtered `ready-for-human`, in-progress, blocked, and WIP tickets. Trust the queue.

## Termination

If `<issues>` is empty, output `<promise>NO MORE TASKS</promise>` and stop. (The harness also checks queue length; this is a backup signal.)

## Task selection

Pick ONE ticket. Priority order:

1. Critical bugfixes
2. Development infrastructure (tests, types, dev scripts) — these unblock everything else
3. Tracer-bullet feature slices — thin vertical slices through every relevant layer
4. Polish and quick wins
5. Refactors

**ONE TASK PER ITERATION.** Do not bundle.

## Claim the ticket

First action — label it so no parallel iteration grabs it:

```bash
gh issue edit <N> --add-label in-progress
```

Then fetch the ticket including comments (`gh issue view <N> --comments`) before starting — prior run notes, blockers, and partial work are often there. Read the acceptance criteria, parent (if any), and linked tickets.

## Task reference

- Commit messages reference the ticket as `Closes #N` or `Refs #N`.
- The PR body's first line MUST be `Closes #N` — that is what auto-closes the ticket on merge and what queue deduplication keys on.

## Close out

If every acceptance criterion is genuinely done:
- Tick all `- [ ]` boxes in the ticket body via `gh issue edit <N> --body-file <file>` — a ticket body is always multi-line, so `--body "..."` would post literal `\n` sequences
- Remove `in-progress` label
- The PR's `Closes #N` will close the ticket on merge

If the task is partial:
- Comment on the ticket: what's done, what remains, blockers — again via `--body-file` for anything multi-line
- Remove `in-progress` so the next iteration can resume
- Still open the PR with the partial work

## Mode rules

- **Never** close a ticket with unticked acceptance criteria
- **Never** pick a `ready-for-human` ticket — if one slipped past the filter, comment on it noting the misfiled label and pick a different ticket
