# Rust orchestrator: shadow validation

Phase 4 of `rust-orchestrator.md`, tracked in #142. Run on 2026-10-17 by the wave lead. All seven bugs found are fixed on `rust-rewrite` at `db3f646`.

## Setup

- **Rust binary:** built from `rust-rewrite`. The head under test is named in each section.
- **State:** isolated `--db-path`, `XDG_STATE_HOME` and `ALUCARD_CACHE_DIR` in a scratch directory. Aldo's installed alucard, ledger and state were not touched.
- **Host tools:** `gh` 2.45.0 (Ubuntu 24.04's package) and Git over SSH. The host has no HTTPS credential helper.
- **Image:** `alucard-rust:latest`, Claude Code 2.1.289. Every role runs on `claude-opus-5-5`, except CI fix on `claude-sonnet-5-5`.
- **Read-only target:** `aldovc/alucard`, the only real repository Aldo approved for observation.
- **Write-path target:** `aldovc/alucard-smoke`, a private sandbox Aldo named, with a local tasks file outside the repository. Bash and Rust never acted on the same task or PR.

## Read-only comparison (`aldovc/alucard`)

| Command | Bash | Rust | Outcome |
|---|---|---|---|
| `queue aldovc/alucard` | `[]`, rc 0 | failed: target passed to `gh` as a directory | Bug #261, fixed in #264 |
| `queue <clone> --github` | `[]`, rc 0 | failed: malformed open-PR GraphQL query | Bug #261, fixed in #264 |
| `queue <clone>` | `[]`, rc 0 | same failure | Bug #261, fixed in #264 |
| `doctor <clone>` | rc 0 | rc 1: `missing: ledger migrations` on a fresh ledger | Explained, see below |

After #264 all three `queue` forms print `[]` with rc 0, matching Bash.

## Write paths (`aldovc/alucard-smoke`)

| Path | Result | Bugs found |
|---|---|---|
| Full run, local task, PR opened, CI gate, review gate | PR #3 opened. CI gate: `no CI ran` (correct, the repo has no workflows). Review approved; labeled `needs-human` because no CI ran (designed behavior from #187/#188). | #261 (`--slurp` needs gh ≥ 2.48; fixed in #264), #265 (host-side git used the container's HTTPS origin; fixed in #266) |
| Merge | `alucard merge 3` ran the check, posted evidence, merged with `--match-head-commit` (`5c0f81d`), deleted the branch, and recorded the ledger merge run. The next `queue` marked the task `[x]`. | none |
| Recovery | A worker committed, backgrounded a long command, and ended its session cleanly with no PR. Before #269, Rust requeued the task and pushed nothing. After it, draft recovery PR #5 opened with `alucard` and `needs-human`, the parked marker and the Bash-protocol body; the ledger holds the task. After #272, the task heading also reads `[>] … (PR #6)`. | #267 (clean exit dropped the work, breaking #200; fixed in #269), #270 (task heading not marked `[>]` after a recovery PR; fixed in #272) |
| Continue | `alucard continue 2` resumed a PR stuck since the run before #265, then went through the CI and review gates. | #268 (`baseRefOid` JSON field missing in gh 2.45; fixed in #271) |

## Explained differences

- **`doctor` on a fresh ledger exits 1.** It reports `missing: ledger migrations`. Bash has no ledger. `run` migrates on first use (#213), so this blocks nothing. Change it only if Aldo wants `doctor` to treat an absent ledger as fine.
- **Recovery stop wording for a clean exit.** Rust says "ended its session". Bash's `describe_worker_stop` falls through to "failed". Rust's wording is accurate; this is the only text difference in the recovery PR.
- **Turn cap with work done in one turn.** With `ALUCARD_WORKER_MAX_TURNS=1`, the worker created the files, committed, pushed and opened the PR within its one allowed turn (`error_max_turns`). Rust found the PR and ran the gates, as Bash does.

## What the bugs had in common

Every bug was invisible to the suite because the tests stub `gh` and use local bare-path remotes:

- **Query syntax:** a GraphQL query the stub never parsed.
- **`gh` version:** `gh` flags and fields that exist only in versions newer than 2.45.
- **Remote URLs:** origin rewrites that matter only for `git@github.com:` URLs.
- **Agent behavior:** an agent that ends its session cleanly mid-task.

Before cutover, run the Rust binary against real `gh` and GitHub at least once per command.

## Not covered

- **Startup reconciliation:** not observed on a real repository; there is no stand-alone dry-run command.
- **Postgres and Playwright targets:** not run (alucard-smoke has neither). The stub-level tests from #236/#239 are the only evidence.
- **Fork PRs, CI-fix agent, feedback agent on a real CI failure:** not exercised (alucard-smoke has no CI).
- **GitHub task source in `run`:** only `queue` was run against a real issue list. `run --issue` against a real repository was not run.
