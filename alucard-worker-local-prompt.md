# Mode: local tasks

The queue lives in a local file owned by the harness — GitHub Issues are NOT involved in this run. Context sections:

- `<parent_context>` — the plan's shared context; implement against its constraints
- `<task>` — the ONE task for this iteration (JSON: id, title, body)

Work ONLY on the task in `<task>`. There is no queue to pick from — the harness already chose, and it has already filtered blocked and in-flight tasks.

Skip every `gh issue` command: there is no issue to label, view, edit, comment on, or close.

If the task title mentions a previous PR (`previous attempt: PR #N`), read it before starting: `gh pr view <N> --comments` — prior work, review findings, and blockers are there.

Do not look for or edit the tasks file itself: it is not in your worktree. The harness updates it when your PR opens and merges.

## Task reference

- The PR body's first line MUST be `Task: <id>` (e.g. `Task: 3`). NEVER write `Closes #N` or any issue-closing keyword — task ids are not issue numbers, and a closing keyword would close an unrelated GitHub issue in this repository.
- Commit messages reference the task as `Task: <id>`.

## Close out

In the PR body, list every acceptance criterion from the task with its status (done / partial — what remains / skipped — why). This replaces ticking checkboxes on an issue.
