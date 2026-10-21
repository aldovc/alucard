# Bash compatibility contracts

This is the read-only behaviour matrix for the Rust port.  It records the
observable Bash contract at the start of the rewrite; ticket numbers identify
the Rust implementation responsible for keeping each behaviour.  #144 removed
the Bash script and the tests below other than `test_merge.sh` and
`test_continue_feedback_stall.sh`, which now run only against the binary; the
last column names the Rust tests (by test target or `lib`/`main` unit module)
that now cover each invariant.

## Test matrix

| Bash test | Invariant | Rust ticket | Rust coverage |
|---|---|---|---|
| `test_agent_settings.sh` | Role settings, defaults, and environment overrides select the recorded agent arguments. | #131 | `agent_settings::role_settings_and_docker_arguments_match_bash_contract`, `agent_settings::blank_settings_fall_back_like_bash_parameter_expansion` |
| `test_build_image.sh` | `build` stamps its image and propagates build failures. | #131 | `lib::build_cleanup_matches_bash_contract`, `agent_settings::prompt_and_image_stamp_match_bash_inputs` |
| `test_ci_checks_probe.sh` | CI availability is probed rather than assumed green. | #136 | `main::ci_checks_denial_uses_run_list_for_later_gates`, `main::ci_registration_requires_the_pr_to_converge_on_origin` |
| `test_ci_gate_failure_log.sh` | Failed CI records useful failure output before recovery. | #136 | `main::ci_fix_prompt_uses_the_current_head_failed_run_log`, `main::ci_fix_failed_run_log_is_limited_to_400_lines` |
| `test_ci_run_list_stale_sha.sh` | Checks for an older head SHA do not pass the CI gate. | #136 | `main::ci_run_list_uses_only_the_current_head`, `main::ci_result_is_invalidated_when_the_head_moves_during_polling` |
| `test_concurrent_runs.sh` | Concurrent runs serialize cached-clone Git writes. | #130 | `database::concurrent_preparation_keeps_working_clones_usable`, `database::prepared_runs_have_isolated_clones_branches_and_directories` |
| `test_container_repo_context.sh` | Containers receive a usable repository context and failures recover safely. | #131 | `test/test_entrypoint.sh`, `main::github_repo_slug_accepts_only_github_origins`, `agent_settings::classifier_preserves_wedge_and_no_worktree_outcomes` |
| `test_continue_feedback_stall.sh` | `continue` preserves a stalled feedback path for human attention. | #138 | `continue_feedback_stall` (runs the shell test against the binary) |
| `test_credentials.sh` | Provider selection requires only that provider's credentials. | #131 | `lib::credentials_are_required_only_for_configured_providers`, `local_tasks::run_loads_env_file_for_host_commands_and_rejects_missing_credentials_early` |
| `test_engineering_policy.sh` | Prompt assembly includes the engineering policy in the defined order. | #131 | `main::engineering_policy_reaches_role_prompts_ahead_of_the_worker_mode` |
| `test_error_scan.sh` | Agent JSONL errors and exit codes classify failures consistently. | #137 | `agent_settings::provider_result_classification_matches_bash_contract`, `agent_settings::classifier_preserves_wedge_and_no_worktree_outcomes` |
| `test_event_log.sh` | Run lifecycle events are appended to `events.log` in order. | #140 | `measurements::events_prompts_and_usage_summary_are_local_artifacts`, `database::runs_commands_list_recent_runs_and_show_claims_and_pull_requests` |
| `test_feedback_stall.sh` | A feedback agent that does not publish cannot be treated as a successful fix. | #137 | `review_gate::run_reconciles_reviewer_verdicts_and_injects_review_policy` (feedback-stall cases) |
| `test_feedback_toolchain.sh` | Feedback receives the toolchain preflight status. | #137 | `review_gate::run_reconciles_reviewer_verdicts_and_injects_review_policy` (feedback-toolchain cases), `main::feedback_prompt_layout` |
| `test_findings_handoff.sh` | Review findings handed to feedback keep actionable content and human comments. | #137 | `main::feedback_findings_strip_only_nonactionable_top_level_sections`, `local_tasks::continue_passes_selected_human_comments_to_feedback` |
| `test_measurement_gates.sh` | Gate outcomes are represented in the measurement record. | #140 | `review_gate::run_reconciles_reviewer_verdicts_and_injects_review_policy`, `local_tasks::continue_records_the_selected_measurement_base_source` |
| `test_measurement_run_loop.sh` | Worker usage and iteration measurements roll up into a run. | #140 | `lib::run_measurement_has_the_complete_bash_compatible_schema`, `measurements::transport_retry_usage_includes_every_attempt_in_table_and_comment` |
| `test_measurements.sh` | Measurement JSON preserves stages, unavailable usage, costs, and diffs. | #140 | `measurements::stage_measurement_buckets_test_paths_and_renames`, `measurements::stage_measurement_reports_when_the_tracked_base_advances` |
| `test_merge.sh` | Merge requires current-head harness approval and leaves the branch safe on failure. | #139 | `merge` (runs the shell test against the binary) |
| `test_playwright_cache.sh` | Playwright cache seeding is stamped and safe to reuse. | #131 | `agent_settings::run_and_continue_roles_mount_only_validated_playwright_caches`, `review_gate::unseeded_playwright_cache_is_not_mounted_for_agent_launches` |
| `test_preflight_manifests.sh` | Dependency manifests produce explicit toolchain preflight status. | #131 | `agent_settings::toolchain_manifests_and_status_match_bash_contract` |
| `test_pr_binding.sh` | Gates and usage bind to the PR opened by this worker attempt. | #133 | `local_tasks::test_pr_binding` |
| `test_provider.sh` | Provider values select the matching CLI and reject invalid combinations. | #131 | `agent_settings::role_settings_and_docker_arguments_match_bash_contract`, `version::doctor_rejects_invalid_provider_settings` |
| `test_review_approved_draft.sh` | Only an authenticated current approval unparks a recovery draft. | #137 | `review_gate::run_reconciles_reviewer_verdicts_and_injects_review_policy` (recovery cases) |
| `test_review_gate.sh` | Review verdicts, blocked findings, and human-comment filtering drive the gate. | #137 | `review_gate::run_reconciles_reviewer_verdicts_and_injects_review_policy`, `main::reviewer_decision_file_falls_back_and_blocked_overrides_formal_review` |
| `test_review_no_verdict.sh` | A missing verdict is never approval and is surfaced for a human. | #137 | `review_gate::run_reconciles_reviewer_verdicts_and_injects_review_policy` (review-no-verdict), `main::review_audit_text_keeps_the_human_handoff_protocol` |
| `test_review_policy.sh` | Repository review policy is bounded and appended to reviewer input. | #137 | `main::reviewer_prompt_includes_local_task_and_bounded_review_policy` |
| `test_tasks_parser.sh` | Task headings and blockers produce the eligible local/GitHub queue and safe write-back. | #127, #128, #141 | `local_tasks::parser_preserves_task_structure_and_queue_eligibility`, `local_tasks::issue_blockers_use_the_open_issue_set`, `local_tasks::task_heading_write_preserves_non_heading_bytes` |
| `test_timeout_recovery.sh` | A timeout preserves worker changes in a recovery path. | #133 | `local_tasks::test_timeout_recovery`, `local_tasks::wrapup_timeout_still_creates_a_recovery_pr` |
| `test_transport_pr_retry.sh` | A transport retry adopts the attempt's already-open PR rather than duplicating it. | #133 | `local_tasks::test_transport_pr_retry` |
| `test_transport_recovery.sh` | A clean transport failure retains its branch without a spurious recovery PR. | #133 | `local_tasks::test_transport_recovery` |
| `test_turn_budget.sh` | Exhausted turn budgets are classified and recovered as incomplete work. | #133 | `local_tasks::test_turn_budget` |
| `test_usage_extract.sh` | Claude and Codex JSONL usage produces one normalized usage table/comment. | #140 | `measurements::extracts_claude_and_codex_usage`, `measurements::formats_usage_table_and_iteration_comment` |
| `test_worker_recovery.sh` | Recovery PRs occupy their task and release the issue claim label. | #134 | `local_tasks::test_worker_recovery`, `main::recovery_github_artifacts_match_the_bash_protocol` |
| `test_worker_wrapup.sh` | Wrap-up runs before mechanical recovery and its handoff reaches the recovery PR. | #134 | `local_tasks::wrapup_changes_are_committed_to_the_recovery_branch`, `main::wrapup_prompt_layout` |

## Remote protocol

All of the following literal prefixes were defined in the Bash `alucard` script and are still written and read by the Rust CLI, so existing
remote state remains readable after cutover.  `{...}` denotes a runtime value,
not part of the literal prefix.

| Remote value | Exact format written | Reader |
|---|---|---|---|
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
and per-role `ALUCARD_<ROLE>_{PROVIDER,CLAUDE_MODEL,CLAUDE_FALLBACK_MODEL,EFFORT,CODEX_MODEL}`, where
`<ROLE>` is only `WORKER`, `CI_FIX`, `REVIEWER`, or `FEEDBACK`.
For each role, `ALUCARD_<ROLE>_PROVIDER` overrides `ALUCARD_PROVIDER`; either
must be `claude` or `codex`, and an unset value falls back to `claude`.
Model and effort settings resolve independently per role before falling back to
their global setting and then the built-in default. The wrap-up agent inherits
the worker provider and model settings; it has no role-specific provider or
model variables.
Credentials are `GITHUB_TOKEN`, `ANTHROPIC_API_KEY`, and `OPENAI_API_KEY`.
`GITHUB_TOKEN` is always required. `ANTHROPIC_API_KEY` is required only when
at least one configured role uses Claude, and `OPENAI_API_KEY` only when at
least one configured role uses Codex.
Turn/budget settings are `ALUCARD_{WORKER,CI_FIX,REVIEWER,FEEDBACK,WRAPUP}_MAX_TURNS`
and `_MAX_BUDGET`; retry count is `ALUCARD_TRANSPORT_RETRY_ATTEMPTS`.

`run` and `continue` exit 0 after completion, an empty queue, max iterations,
or a gate/recovery outcome that leaves work for a human. A provider
`billing_error` aborts `run` with 2. Invalid arguments, unavailable
prerequisites, malformed task files, and setup errors exit 1. SIGINT and
SIGTERM exit 130 and 143. An agent whose worktree is absent uses 78
(`NO_WORKTREE_RC`) so the run classifies it as retryable; `timeout` uses 124
and hard-kill handling sees 137. Agent commands otherwise retain their process
exit code for classification.

## Queue JSON contract

GitHub output is an array of `{number,title,body,labels}`, where `labels` is
an array of objects with `name`. It includes only open `ready-for-agent`
issues that are neither blocked nor labelled `in-progress`/`wip` and are not
occupied by an open PR. Local output is an array of `{id,title,body}` in file
order, limited to `[ ]` headings whose local and GitHub blockers are satisfied.
The canonical fixture inputs and expected outputs are in
`test/fixtures/contracts/`.
