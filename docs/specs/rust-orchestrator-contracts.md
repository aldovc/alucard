# Bash compatibility contracts

This is the read-only behaviour matrix for the Rust port.  It records the
observable Bash contract at the start of the rewrite; ticket numbers identify
the Rust implementation responsible for keeping each behaviour.

## Test matrix

| Bash test | Invariant | Rust ticket |
|---|---|---|
| `test_agent_settings.sh` | Role settings, defaults, and environment overrides select the recorded agent arguments. | #131 |
| `test_build_image.sh` | `build` stamps its image and propagates build failures. | #131 |
| `test_ci_checks_probe.sh` | CI availability is probed rather than assumed green. | #136 |
| `test_ci_gate_failure_log.sh` | Failed CI records useful failure output before recovery. | #136 |
| `test_ci_run_list_stale_sha.sh` | Checks for an older head SHA do not pass the CI gate. | #136 |
| `test_concurrent_runs.sh` | Concurrent runs serialize cached-clone Git writes. | #130 |
| `test_container_repo_context.sh` | Containers receive a usable repository context and failures recover safely. | #131 |
| `test_continue_feedback_stall.sh` | `continue` preserves a stalled feedback path for human attention. | #138 |
| `test_credentials.sh` | Provider selection requires only that provider's credentials. | #131 |
| `test_engineering_policy.sh` | Prompt assembly includes the engineering policy in the defined order. | #131 |
| `test_error_scan.sh` | Agent JSONL errors and exit codes classify failures consistently. | #137 |
| `test_event_log.sh` | Run lifecycle events are appended to `events.log` in order. | #140 |
| `test_feedback_stall.sh` | A feedback agent that does not publish cannot be treated as a successful fix. | #137 |
| `test_feedback_toolchain.sh` | Feedback receives the toolchain preflight status. | #137 |
| `test_findings_handoff.sh` | Review findings handed to feedback keep actionable content and human comments. | #137 |
| `test_measurement_gates.sh` | Gate outcomes are represented in the measurement record. | #140 |
| `test_measurement_run_loop.sh` | Worker usage and iteration measurements roll up into a run. | #140 |
| `test_measurements.sh` | Measurement JSON preserves stages, unavailable usage, costs, and diffs. | #140 |
| `test_merge.sh` | Merge requires current-head harness approval and leaves the branch safe on failure. | #139 |
| `test_playwright_cache.sh` | Playwright cache seeding is stamped and safe to reuse. | #131 |
| `test_preflight_manifests.sh` | Dependency manifests produce explicit toolchain preflight status. | #131 |
| `test_pr_binding.sh` | Gates and usage bind to the PR opened by this worker attempt. | #133 |
| `test_provider.sh` | Provider values select the matching CLI and reject invalid combinations. | #131 |
| `test_review_approved_draft.sh` | Only an authenticated current approval unparks a recovery draft. | #137 |
| `test_review_gate.sh` | Review verdicts, blocked findings, and human-comment filtering drive the gate. | #137 |
| `test_review_no_verdict.sh` | A missing verdict is never approval and is surfaced for a human. | #137 |
| `test_review_policy.sh` | Repository review policy is bounded and appended to reviewer input. | #137 |
| `test_tasks_parser.sh` | Task headings and blockers produce the eligible local/GitHub queue and safe write-back. | #127, #128, #141 |
| `test_timeout_recovery.sh` | A timeout preserves worker changes in a recovery path. | #133 |
| `test_transport_pr_retry.sh` | A transport retry adopts the attempt's already-open PR rather than duplicating it. | #133 |
| `test_transport_recovery.sh` | A clean transport failure retains its branch without a spurious recovery PR. | #133 |
| `test_turn_budget.sh` | Exhausted turn budgets are classified and recovered as incomplete work. | #133 |
| `test_usage_extract.sh` | Claude and Codex JSONL usage produces one normalized usage table/comment. | #140 |
| `test_worker_recovery.sh` | Recovery PRs occupy their task and release the issue claim label. | #134 |
| `test_worker_wrapup.sh` | Wrap-up runs before mechanical recovery and its handoff reaches the recovery PR. | #134 |

## Remote protocol

All of the following literal prefixes are defined in `alucard`, so existing
remote state remains readable after cutover.  `{...}` denotes a runtime value,
not part of the literal prefix.

| Remote value | Exact format written | Reader |
|---|---|---|
| Wrapper comment prefix | `**🤖 Alucard` | `review_gate`, `command_continue`, `harness_approved_sha` |
| Blocked findings | `**🤖 Alucard blocked findings** (cycle {N}/{MAX})` | `review_gate` (comment body starts-with filter) |
| Parked recovery comment | `**🤖 Alucard recovery PR parked** — opened as a draft and labeled \`needs-human\` by the harness.` | `review_gate` (authenticated comment) |
| Recovery PR body | `{Refs line, if known}\n\n**🤖 Alucard recovery PR** — the worker {stop} (rc={RC}) before opening a PR. Its {COMMIT_COUNT} commit(s), plus whatever was still uncommitted, are pushed here so the work isn't lost. Nothing on this branch has been through CI or review, and the last commit may be mid-edit.` The body then contains the required next-step text and optional wrap-up handoff. | Human-facing; recovery eligibility is read from the authenticated parked comment or draft/title/label state |
| Recovery issue comment | `**🤖 Alucard recovery** — the worker on this ticket {stop} (rc={RC}) before opening a PR.` | Human/next worker; GitHub PR occupancy is read by `open_pr_covers_ticket` |
| Review approval | `**🤖 Alucard review cycle {N}/{MAX}: APPROVED**` | `harness_approved_sha` treats this as the latest approval; `review_gate` human-facing audit |
| Review approval without CI | `**🤖 Alucard review cycle {N}/{MAX}: APPROVED — no CI ran, needs human**` | Human-facing; wrapper prefix filters it |
| Reviewed head | `Reviewed head: \`{40 lowercase hex SHA}\`` in an approval comment | `harness_approved_sha` |
| Review blocked | `**🤖 Alucard review cycle {N}/{MAX}: BLOCKED — needs human**` | Human-facing; supersedes an earlier approval because `harness_approved_sha` reads the latest review-cycle comment |
| No review verdict | `**🤖 Alucard review cycle {N}/{MAX}: no review posted — needs human** (rc={RC}, class={CLASS})` | Human-facing; supersedes an earlier approval because `harness_approved_sha` reads the latest review-cycle comment |
| Review changes requested | `**🤖 Alucard review cycle {N}/{MAX}: CHANGES_REQUESTED**` | `command_continue` finds the latest such comment; human-facing audit |
| Review cycles exhausted | `**🤖 Alucard gave up after {MAX}/{MAX} review cycles — needs human**` | Human-facing; wrapper prefix filters it |
| Feedback stalled in review loop | `**🤖 Alucard review cycle {N}/{MAX}: feedback agent stopped without publishing — needs human** (rc={RC})` | Human-facing; wrapper prefix filters it |
| Feedback failed after publishing in review loop | `**🤖 Alucard cycle {N}/{MAX}: feedback agent failed after pushing** (rc={RC}, branch advanced={BOOLEAN_OR_UNKNOWN})` | Human-facing; wrapper prefix filters it |
| Feedback stalled by `continue` | `**🤖 Alucard continue: feedback agent stopped without publishing — needs human** (rc={RC})` | Human-facing; wrapper prefix filters it |
| Feedback failed after publishing by `continue` | `**🤖 Alucard continue: feedback agent failed after pushing** (rc={RC}, branch advanced={BOOLEAN_OR_UNKNOWN})` | Human-facing; wrapper prefix filters it |
| Usage comment | `**🤖 Alucard token usage (iteration {N})**` | No semantic reader; excluded as wrapper-authored by the human-comment filters |
| No-CI comment | `**🤖 Alucard CI gate: no CI ran — not green**` | Human-facing; wrapper prefix filters it |
| Merge rebase conflict | `**🤖 Alucard merge: rebase conflict — needs human**` | Human-facing; wrapper prefix filters it |
| Merge check result | `**🤖 Alucard merge: checks passed**` or `**🤖 Alucard merge: checks failed — not merged**` | Human-facing; wrapper prefix filters it |
| Recovery title | `wip: alucard recovery — iter {N} (worker stopped: {class}, rc={RC})` | `review_gate` recovery identification |

The labels are protocol too: GitHub source reads `ready-for-agent`, skips
`in-progress` and `wip`, and a blocked/recovery/review outcome adds
`needs-human`. Recovery PR creation writes the exact `alucard` label;
`command_run` reads it in its worker-PR fallback to find an already-open
worker-created PR when its branch does not match the harness branch.
`command_run` removes `in-progress` after a stopped worker;
`unpark_recovery_pr` removes `needs-human` only after current-head approval.
`ready-for-human` is not selected because the source query requires
`ready-for-agent`.

## CLI contract

Commands: `run [TARGET_REPO]`, `queue [TARGET_REPO]`, `build`, `doctor
[TARGET_REPO]`, `continue <PR_NUM> [TARGET_REPO]`, `merge <PR_NUM>
[TARGET_REPO]`, `version`, and `help`.  A target is a local clone,
`owner/repo`, or GitHub HTTPS URL.  `-C/--repo PATH` is equivalent to the
target positional; its default is `ALUCARD_TARGET_REPO` then `$PWD`.

`run`, `queue`, and `doctor` accept `-C/--repo`, `-t/--timeout-minutes`,
`--env-file`, `--image`, `--logs-root`, `--tasks`, and `--github`; run also
accepts `-n/--iterations`, `--base`, `--max-review-cycles`, `--no-build`, and
`--issue`. `continue` accepts the run settings except iterations, base, task
source, and issue. `merge` accepts `-C`, `-t`, `--env-file`, `--logs-root`,
`--check-command`, and `--dry-run`. `build` accepts `--image`; `queue` and
`doctor` reject `--issue`. `--tasks` conflicts with `--github` and `--issue`.

The legacy positional form `alucard N [M] [TARGET_REPO] [options]` rewrites to
`run --iterations N [--timeout-minutes M]`; a directory as the first word also
means `run DIRECTORY`. `--issue` forces one iteration.

Defaults: iterations 20, timeout 30 minutes, base `main`, max review cycles
10, image `ghcr.io/aldovc/alucard:latest`, env file beside the CLI
(`ALUCARD_ENV_FILE`), logs `${XDG_STATE_HOME:-$HOME/.local/state}/alucard/logs`,
and cache `${XDG_CACHE_HOME:-$HOME/.cache}/alucard`. Task source precedence is
`--issue`/`--github`, `--tasks`/`ALUCARD_TASKS_FILE`,
`.alucard/tasks.md`, then GitHub.

Environment configuration is `ALUCARD_IMAGE`, `ALUCARD_ENV_FILE`,
`ALUCARD_BASE_BRANCH`, `ALUCARD_LOG_ROOT`, `ALUCARD_CACHE_DIR`,
`ALUCARD_TARGET_REPO`, `ALUCARD_TASKS_FILE`, `ALUCARD_CHECK_COMMAND`, global
provider/model settings `ALUCARD_PROVIDER` (default `claude`),
`ALUCARD_CLAUDE_MODEL`,
`ALUCARD_CLAUDE_FALLBACK_MODEL`, `ALUCARD_CODEX_MODEL`, `ALUCARD_EFFORT`,
and per-role `ALUCARD_<ROLE>_{PROVIDER,CLAUDE_MODEL,CLAUDE_FALLBACK_MODEL,EFFORT,CODEX_MODEL}`.
For each role, `ALUCARD_<ROLE>_PROVIDER` overrides `ALUCARD_PROVIDER`; either
must be `claude` or `codex`, and an unset value falls back to `claude`.
Model and effort settings resolve independently per role before falling back to
their global setting and then the built-in default.
Turn/budget settings are `ALUCARD_{WORKER,CI_FIX,REVIEWER,FEEDBACK,WRAPUP}_MAX_TURNS`
and `_MAX_BUDGET`; retry count is `ALUCARD_TRANSPORT_RETRY_ATTEMPTS`.

Exit 0 means the requested command completed. Invalid arguments, unavailable
prerequisites, malformed task files, command/agent failures, and failed gates
exit 1. An agent whose worktree is absent uses 78 (`NO_WORKTREE_RC`) so the
run classifies it as retryable; `timeout` uses 124 and hard-kill handling sees
137. Agent commands otherwise retain their process exit code for classification.

## Queue JSON contract

GitHub output is an array of `{number,title,body,labels}`, where `labels` is
an array of objects with `name`. It includes only open `ready-for-agent`
issues that are neither blocked nor labelled `in-progress`/`wip` and are not
occupied by an open PR. Local output is an array of `{id,title,body}` in file
order, limited to `[ ]` headings whose local and GitHub blockers are satisfied.
The canonical fixture inputs and expected outputs are in
`test/fixtures/contracts/`.
