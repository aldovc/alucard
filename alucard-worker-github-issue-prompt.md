# Mode: GitHub issue

This run is pinned to one GitHub issue. Context section:

- `<issue>` — JSON object with `number`, `title`, and `labels` (name strings) — no body. Fetch the body with `gh issue view <N> --comments`; do not assume the prompt carries it.

Work ONLY on the issue in `<issue>`. There is no queue to pick from.

## Claim the ticket

First action — label it so no parallel iteration grabs it:

```bash
gh issue edit <N> --add-label in-progress
```

If the label is already present, that is fine. Then fetch the ticket including comments (`gh issue view <N> --comments`) before starting — prior run notes, blockers, and partial work are often there. Read the acceptance criteria, parent (if any), and linked tickets.

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
