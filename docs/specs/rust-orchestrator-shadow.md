# Rust orchestrator: shadow validation

Phase 4 of `rust-orchestrator.md`, tracked in #142. The wave lead ran a first pass on 2026-10-17 and a second on 2026-10-18. The first pass found seven bugs, all fixed on `rust-rewrite` at `db3f646`. The second pass covered CI fix and restart replay on that revision and found one more bug, #274, fixed in #275 (`eed9ab9`). Some paths are still untested; see [Not covered](#not-covered). Accepting #142 and authorizing #143 are Aldo's decisions.

## Setup

- **Rust binary:** built with `cargo build --locked --offline` at the revision named in each row. Revisions marked "PR head" were tested before their squash merge. Each such head is tree-identical to its squash commit on `rust-rewrite`: `c9ac9ac` = `b5cac1d` (#264), `4b3c880` = `de8750f` (#266), `dc45604` = `fa225d8` (#269), `82749a7` = `79686f0` (#271), `085266c` = `db3f646` (#272).
- **State:** isolated `--db-path`, `XDG_STATE_HOME` and `ALUCARD_CACHE_DIR`. Aldo's installed alucard, ledger and state were not touched. The first pass used a scratch directory, and that directory, including its sandbox ledger, no longer exists. The second pass keeps its ledger, state, sandbox clone and tasks file in `~/.local/state/alucard/rust-rewrite/shadow/r2/`.
- **Host tools:** `gh` 2.45.0 (Ubuntu 24.04's package) and Git over SSH. The host has no HTTPS credential helper.
- **Image:** `alucard-rust:latest`, ID `acb2e50e41dc`, Claude Code 2.1.289. Every role runs on `claude-opus-5-5`, except CI fix on `claude-sonnet-5-5`.
- **Read-only target:** `aldovc/alucard`, the only real repository Aldo approved for observation.
- **Write-path target:** `aldovc/alucard-smoke`, a private sandbox Aldo named. Bash and Rust never acted on the same task or PR.

## Evidence

Run directories are under `~/.local/state/alucard/rust-rewrite/shadow/<dir>/` on the lead's host. Each holds `console.log` and, for runs that reached an agent, a run directory with `measurements.jsonl`, `events.log`, prompts and agent transcripts. The `run` record in `measurements.jsonl` names the Rust revision (`alucard_rev`) and task source. `merge`, `continue` and the read-only commands write no run record. For the first pass, their revisions and every exact command line come from the lead's session transcript. Each second-pass directory has a `command.txt`. `PROVENANCE.md` in that directory lists the commands and names the transcript.

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
| Attempted CI fix, `run --issue 11` (first pass) | `db3f646` | `cifix` | The issue asked for a file whose content fails CI. The worker followed verify-before-push and wrote `# plain note` instead. CI passed on PR #12 (`b709867`). The measurements show only worker and reviewer invocations. Review returned BLOCKED because the intended failure exercise was not performed. **No CI failure occurred and no CI-fix agent ran.** | none |
| CI fix on a real CI failure, `continue 12` (second pass) | `db3f646` | `r2-cifix` | Authorized fixture outside the worker. The lead pushed `eebcb87` to PR #12's branch with the title removed, and `ci / check` failed. The lead also edited issue #11 to name `# plain note` as the end state. `continue 12` handed the failure to the CI-fix agent, which pushed `05a49ac`, and CI passed. Review cycle 1 requested changes: the fix had added `# Plain note` above the bare line instead of restoring the single line. The feedback agent pushed `4b213c2`, CI passed, and review cycle 2 approved with `Reviewed head: 4b213c2…`. The measurements record cifix and feedback stages and `ci_result: green`. | none |
| Restart-time journal replay (second pass) | `db3f646` | `r2-replay-kill`, `r2-replay-restart1`, `r2-replay-restart2` | Authorized fixture: a host-only `gh` wrapper killed Rust with SIGKILL at `gh pr create` during a local-task recovery. That left the branch pushed at `71cb063`, `push` done, `create_recovery_pr` pending, no PR, and the task claim held by a dead owner. A restart with the real `gh` replayed the journal and created draft recovery PR #13 once, with `alucard`, `needs-human` and the parked marker. All five operations finished, the recovery row was recorded, the run became `interrupted` and the task stayed `in_flight`. Nothing was dispatched, and `queue` returned `[]`. A second restart created nothing. However, the task heading stayed `[ ]` and the ledger task lost its `pr_number`. | #274 (restart replay skips the #270 heading write; fixed in #275) |
| Restart replay after the #274 fix | `175841c` (#275 PR head, tree-identical to `eed9ab9`) | `r3-replay-kill`, `r3-replay-restart1`, `r3-replay-restart2` | Same fixture on a fresh ledger. The restart created draft recovery PR #14 once, with labels and marker. The heading reads `[>] replay-heading: … (PR #14)` and the ledger task has `pr_number = 14`. A second restart and `queue` changed nothing. | none |
| `runs` (second pass) | `db3f646` | `r2-runs.txt` | On the second-pass ledger it listed the replay run as `interrupted` and the continue run as `completed`, with their log paths. Not compared with Bash. | none |

Sandbox records:

- PR #3 merge evidence: https://github.com/aldovc/alucard-smoke/pull/3#issuecomment-6228762718
- PR #10 approval of `20ff329`: https://github.com/aldovc/alucard-smoke/pull/10#issuecomment-6229810192
- PR #10 owner comment: https://github.com/aldovc/alucard-smoke/pull/10#issuecomment-6229815259
- PR #10 approval of `d25c5d3`: https://github.com/aldovc/alucard-smoke/pull/10#issuecomment-6229831296
- PR #12 BLOCKED review: https://github.com/aldovc/alucard-smoke/pull/12#issuecomment-6229847731
- PR #12 failing fixture run: https://github.com/aldovc/alucard-smoke/actions/runs/39114353252
- PR #12 approval of `4b213c2`: https://github.com/aldovc/alucard-smoke/pull/12#issuecomment-6248110615
- Replayed recovery PRs: https://github.com/aldovc/alucard-smoke/pull/13 (before #275), https://github.com/aldovc/alucard-smoke/pull/14 (after)

## Explained differences

- **`doctor` on a fresh ledger exits 1.** It reports `missing: ledger migrations`. Bash has no ledger. `run` migrates on first use (#213), so this blocks nothing. Change it only if Aldo wants `doctor` to treat an absent ledger as fine.
- **Recovery stop wording for a clean exit.** Rust says "ended its session". Bash's `describe_worker_stop` falls through to "failed". Rust's wording is accurate; this is the only text difference in the recovery PR.
- **Missing queue labels.** On a repository without the README's one-time labels, Rust's claim fails before dispatch with `could not label GitHub issue: … 'in-progress' not found`, and nothing runs. In Bash the worker applies the label itself, so the failure would surface mid-run inside the agent. Rust's behavior is the safer one; the README already says to create the labels first.
- **Queue selection moved into the harness.** Rust picks and claims the issue, then gives the worker the pinned-issue prompt (`alucard-worker-github-issue-prompt.md`) whether or not `--issue` was passed. The prompt and the task source therefore cannot tell `run` from `run --issue`; only the command line can.
- **Turn cap with work done in one turn.** With `ALUCARD_WORKER_MAX_TURNS=1`, the worker created the files, committed, pushed and opened the PR within its one allowed turn (`error_max_turns`). Rust found the PR (#4) and ran the gates, as Bash does. Rust `de8750f`, evidence dir `recovery1`.
- **`needs-human` stays after a later approval.** On PR #12 the first-pass BLOCKED review added `needs-human`, and the second-pass approval left it in place. Bash and Rust both remove that label on approval only for recovery PRs.
- **Restart output.** The replay restart printed nothing about the recovery PR it created. Bash has no startup replay to compare.

## What the bugs had in common

Every bug was invisible to the suite because the tests stub `gh` and use local bare-path remotes:

- **Query syntax:** a GraphQL query the stub never parsed.
- **`gh` version:** `gh` flags and fields that exist only in versions newer than 2.45.
- **Remote URLs:** origin rewrites that matter only for `git@github.com:` URLs.
- **Agent behavior:** an agent that ends its session cleanly mid-task.

Before cutover, run the Rust binary against real `gh` and GitHub at least once per command.

## Not covered

- **Startup lock scope.** `reconcile_startup` holds the ledger-wide recovery lock across replay and remote PR lookups. A slow network call there blocks every other startup on the same ledger. This was not measured.
- **Replay of other journaled operations.** The restart test interrupted only `create_recovery_pr`. Push, label and marker replay rely on the local tests (`startup_replays_pending_recovery_operations_idempotently`, `concurrent_startup_replays_one_recovery_pr`, `startup_replay_preserves_pending_operations_before_remote_mutations` in `tests/local_tasks.rs`, which use real Git and SQLite with a stubbed `gh`). There is no reconciliation dry-run command.
- **`runs` against Bash.** Rust `runs` was observed on both ledgers but not compared with Bash.
- **Postgres and Playwright targets.** Not run; alucard-smoke has neither. The stub-level tests from #236/#239 are the only evidence. Whether they need live runs before cutover is a separate decision.
- **Fork PRs.** Not exercised.
