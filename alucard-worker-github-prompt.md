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
3. Feature work that can merge on its own
4. Polish and quick wins
5. Refactors

**ONE TASK PER ITERATION.** Do not bundle.

## Too large for one iteration

Before claiming, size the ticket against `<turn_budget>`. A ticket that spans several layers (schema, backend, frontend, bot) with a long list of acceptance criteria will not finish in one iteration, and an attempt that exhausts the budget leaves unverified code on a branch nobody scheduled. Do not start it, and do not silently pick something else either: that judgment dies with your log, and a later iteration with nothing else to pick will attempt the same ticket unchanged. Two iterations in a row did exactly that once; the third exhausted its budget on it with nothing committed.

Park it for a human instead:

1. Comment on the ticket (`gh issue comment <N> --body-file <file>`): why it does not fit one iteration, and a proposed split into tickets that each would.
2. Relabel it: `gh issue edit <N> --remove-label ready-for-agent --add-label ready-for-human`. It leaves the queue until a human splits it. The harness reports it at the end of the run.
3. Pick another ticket. If none is left, output `<promise>NO MORE TASKS</promise>` and stop.

Do this for the ticket you were about to pick, not as a triage pass over the whole queue.

## Claim the ticket

First action — label it so no parallel iteration grabs it:

```bash
gh issue edit <N> --add-label in-progress
```

Then fetch the ticket including comments (`gh issue view <N> --comments`) before starting — prior run notes, blockers, and partial work are often there. Read the acceptance criteria, parent (if any), and linked tickets.

## Task reference

- If every acceptance criterion will be done: the PR body's first line MUST be `Closes #N`. Commit messages can use `Closes #N`. That auto-closes the ticket on merge.
- If the work is partial: the PR body's first line MUST be `Refs #N`. Do not write `Closes #N`. The queue treats an open PR that mentions `#N` as occupying the ticket, so the next run will not re-pick it.

## Close out

If every acceptance criterion is genuinely done:
- Tick all `- [ ]` boxes in the ticket body via `gh issue edit <N> --body-file <file>` — a ticket body is always multi-line, so `--body "..."` would post literal `\n` sequences
- Remove `in-progress` label
- The PR's `Closes #N` will close the ticket on merge

If the task is partial:
- Comment on the ticket: what's done, what remains, blockers — again via `--body-file` for anything multi-line
- Remove `in-progress`
- Still open the PR, with `Refs #N` as the first line of the body

## Mode rules

- **Never** close a ticket with unticked acceptance criteria
- **Never** pick a `ready-for-human` ticket — if one slipped past the filter, comment on it noting the misfiled label and pick a different ticket
