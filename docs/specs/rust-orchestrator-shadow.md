# Rust orchestrator: shadow validation

Phase 4 of `rust-orchestrator.md`, tracked in #142. The wave lead ran it on 2026-10-17. This is the record of the first pass. It found seven bugs, all fixed on `rust-rewrite` at `db3f646`. Several failure paths are still untested; see [Not covered](#not-covered). Accepting #142 and authorizing #143 are Aldo's decisions.

## Setup

- **Rust binary:** built with `cargo build --locked --offline` at the revision named in each row. Revisions marked "PR head" were tested before their squash merge. Each such head is tree-identical to its squash commit on `rust-rewrite`: `c9ac9ac` = `b5cac1d` (#264), `4b3c880` = `de8750f` (#266), `dc45604` = `fa225d8` (#269), `82749a7` = `79686f0` (#271), `085266c` = `db3f646` (#272).
- **State:** isolated `--db-path`, `XDG_STATE_HOME` and `ALUCARD_CACHE_DIR` in a scratch directory. Aldo's installed alucard, ledger and state were not touched. The scratch directory, including the sandbox ledger, no longer exists.
- **Host tools:** `gh` 2.45.0 (Ubuntu 24.04's package) and Git over SSH. The host has no HTTPS credential helper.
- **Image:** `alucard-rust:latest`, ID `acb2e50e41dc`, Claude Code 2.1.289. Every role runs on `claude-opus-5-5`, except CI fix on `claude-sonnet-5-5`.
- **Read-only target:** `aldovc/alucard`, the only real repository Aldo approved for observation.
- **Write-path target:** `aldovc/alucard-smoke`, a private sandbox Aldo named. Bash and Rust never acted on the same task or PR.

## Evidence

Run directories are under `~/.local/state/alucard/rust-rewrite/shadow/<dir>/` on the lead's host. Each holds `console.log` and, for runs that reached an agent, a run directory with `measurements.jsonl`, `events.log`, prompts and agent transcripts. The `run` record in `measurements.jsonl` names the Rust revision (`alucard_rev`) and task source. `merge`, `continue` and the read-only commands write no run record. Their revisions, like every exact command line, come from the lead's session transcript. `PROVENANCE.md` in that directory lists the commands and names the transcript.

## Read-only comparison (`aldovc/alucard`)

Rust at `4e92a19`, then `c9ac9ac` (#264 PR head) for the rerun. The outputs were in the deleted scratch directory. The summary is in the #122 comment of 2026-10-17 01:51 UTC.

| Command | Bash | Rust | Outcome |
|---|---|---|---|
| `queue aldovc/alucard` | `[]`, rc 0 | failed: target passed to `gh` as a directory | Bug #261, fixed in #264 |
| `queue <clone> --github` | `[]`, rc 0 | failed: malformed open-PR GraphQL query | Bug #261, fixed in #264 |
| `queue <clone>` | `[]`, rc 0 | same failure | Bug #261, fixed in #264 |
| `doctor <clone>` | rc 0 | rc 1: `missing: ledger migrations` on a fresh ledger | Explained, see below |

At `c9ac9ac` all three `queue` forms print `[]` with rc 0, matching Bash.

## Write paths (`aldovc/alucard-smoke`)

Local-task runs read a tasks file outside the repository. GitHub runs read issues.

| Path | Rust | Evidence dir | Result | Bugs found |
|---|---|---|---|---|
| Full run, local task | `4e92a19`, `b5cac1d`, `4b3c880` (#266 PR head) | `run1`, `run1b`, `run2` | At `4e92a19` the run failed before dispatch (`--slurp`). At `b5cac1d` PR #2 opened but the CI gate reported `unverified`. At `4b3c880` PR #3 opened with head `76549b9`. The CI gate reported `no CI ran`, which is correct because the repo had no workflows then. Review approved; the PR was labeled `needs-human` because no CI ran (designed behavior from #187/#188). | #261 (`--slurp` needs gh ≥ 2.48; fixed in #264), #265 (host-side git used the container's HTTPS origin; fixed in #266) |
| Merge | `de8750f` | `merge3` | `alucard merge 3` ran the check command on head `76549b9` (exit 0), posted the evidence comment, and merged with `--match-head-commit 76549b9`. The squash commit is `5c0f81d`. It deleted the branch and recorded the ledger merge run. The next `queue` marked the task `[x]`. | none |
| `runs` | `de8750f` | none | Run twice on the sandbox ledger, after the merge and after the first recovery attempts. It listed `id`, `status`, `command` and log path, newest first, for the `run` and `merge` runs. The output survives only in the transcript. The ledger is gone, and `runs` was not compared with Bash. | none |
| Recovery-PR creation after a worker stop | `de8750f`, `dc45604` (#269 PR head), `085266c` (#272 PR head) | `recovery2`, `recovery3`, `recovery4` | A worker committed, backgrounded a long command, and ended its session cleanly with no PR. At `de8750f` Rust requeued the task and pushed nothing. At `dc45604` draft recovery PR #5 opened with `alucard` and `needs-human`, the parked marker and the Bash-protocol body, and the ledger held the task. At `085266c` recovery PR #6 opened and the task heading also read `[>] … (PR #6)`. This is the normal end-of-iteration path. It does not exercise restart-time replay. | #267 (clean exit dropped the work, breaking #200; fixed in #269), #270 (task heading not marked `[>]` after a recovery PR; fixed in #272) |
| Continue | `de8750f`, `82749a7` (#271 PR head) | `continue2`, `continue2b` | At `de8750f` `continue 2` failed at once. At `82749a7` it resumed PR #2, stuck since the run before #265, and went through the CI and review gates. | #268 (`baseRefOid` JSON field missing in gh 2.45; fixed in #271) |
| GitHub issue, `run --issue 7` | `db3f646` | `issue7`, `issue7b` | The first attempt stopped before dispatch because the repo lacked the queue labels (see below). After the labels were created, the harness labeled issue #7 `in-progress` before dispatch, and the worker opened PR #8 with `Closes #7`. The CI gate reported `no CI ran`, the review approved, and the PR was labeled `needs-human`. The worker removed `in-progress` as the shared worker prompt tells it to, the same as Bash. | none |
| GitHub queue selection with CI, no `--issue` | `db3f646` | `queue-ci` | `main` had gained a CI workflow (`cd528a2`: every Markdown file starts with a `# ` title). The command was `alucard --db-path … run <clone> --env-file … --image alucard-rust:latest --no-build -n 1 --timeout-minutes 20 --logs-root …`, with no `--issue`, `--tasks` or `--github`, and `ALUCARD_TASKS_FILE` unset. Rust selected issue #9, the only `ready-for-agent` issue, and the run record shows task source `github`. PR #10 opened at `20ff329`, `ci / check` passed, and review approved that head. | none |
| Human PR comment, `continue 10` | `db3f646` | `feedback` | The owner commented on PR #10 asking for a second line. The feedback agent pushed `d25c5d3`. `ci / check` passed on it and review approved with `Reviewed head: d25c5d3…`. The measurements record one feedback invocation and `ci_result: green`. | none |
| Attempted CI fix, `run --issue 11` | `db3f646` | `cifix` | The issue asked for a file whose content fails CI. The worker followed verify-before-push and wrote `# plain note` instead. CI passed on PR #12 (`b709867`). The measurements show only worker and reviewer invocations. Review returned BLOCKED because the intended failure exercise was not performed. **No CI failure occurred and no CI-fix agent ran.** | none |

Sandbox records:

- PR #3 merge evidence: https://github.com/aldovc/alucard-smoke/pull/3#issuecomment-6228762718
- PR #10 approval of `20ff329`: https://github.com/aldovc/alucard-smoke/pull/10#issuecomment-6229810192
- PR #10 owner comment: https://github.com/aldovc/alucard-smoke/pull/10#issuecomment-6229815259
- PR #10 approval of `d25c5d3`: https://github.com/aldovc/alucard-smoke/pull/10#issuecomment-6229831296
- PR #12 BLOCKED review: https://github.com/aldovc/alucard-smoke/pull/12#issuecomment-6229847731

## Explained differences

- **`doctor` on a fresh ledger exits 1.** It reports `missing: ledger migrations`. Bash has no ledger. `run` migrates on first use (#213), so this blocks nothing. Change it only if Aldo wants `doctor` to treat an absent ledger as fine.
- **Recovery stop wording for a clean exit.** Rust says "ended its session". Bash's `describe_worker_stop` falls through to "failed". Rust's wording is accurate; this is the only text difference in the recovery PR.
- **Missing queue labels.** On a repository without the README's one-time labels, Rust's claim fails before dispatch with `could not label GitHub issue: … 'in-progress' not found`, and nothing runs. In Bash the worker applies the label itself, so the failure would surface mid-run inside the agent. Rust's behavior is the safer one; the README already says to create the labels first.
- **Queue selection moved into the harness.** Rust picks and claims the issue, then gives the worker the pinned-issue prompt (`alucard-worker-github-issue-prompt.md`) whether or not `--issue` was passed. The prompt and the task source therefore cannot tell `run` from `run --issue`; only the command line can.
- **Turn cap with work done in one turn.** With `ALUCARD_WORKER_MAX_TURNS=1`, the worker created the files, committed, pushed and opened the PR within its one allowed turn (`error_max_turns`). Rust found the PR (#4) and ran the gates, as Bash does. Rust `de8750f`, evidence dir `recovery1`.

## What the bugs had in common

Every bug was invisible to the suite because the tests stub `gh` and use local bare-path remotes:

- **Query syntax:** a GraphQL query the stub never parsed.
- **`gh` version:** `gh` flags and fields that exist only in versions newer than 2.45.
- **Remote URLs:** origin rewrites that matter only for `git@github.com:` URLs.
- **Agent behavior:** an agent that ends its session cleanly mid-task.

Before cutover, run the Rust binary against real `gh` and GitHub at least once per command.

## Not covered

- **CI-fix agent on a real CI failure.** Not exercised; see the PR #12 row. Covering it needs a failing head created outside the implementation worker, in an explicitly authorized sandbox fixture. Then observe the CI-fix invocation, the replacement head's checks and an exact-head approval. Workers must keep verifying before they push.
- **Restart-time journal replay.** Not observed against GitHub. The recovery PRs above came from the normal end-of-iteration path. There is no reconciliation dry-run command. Local tests with real Git and SQLite and a stubbed `gh` cover replay (`startup_replays_pending_recovery_operations_idempotently`, `concurrent_startup_replays_one_recovery_pr` and `startup_replay_preserves_pending_operations_before_remote_mutations` in `tests/local_tasks.rs`). They are separate evidence from a real restart.
- **Startup lock scope.** `reconcile_startup` holds the ledger-wide recovery lock across replay and remote PR lookups. A slow network call there blocks every other startup on the same ledger. This was not measured.
- **`runs`.** Only the ad-hoc observation above. It has not been run against a surviving populated ledger or compared with Bash.
- **Postgres and Playwright targets.** Not run; alucard-smoke has neither. The stub-level tests from #236/#239 are the only evidence. Whether they need live runs before cutover is a separate decision.
- **Fork PRs.** Not exercised.
