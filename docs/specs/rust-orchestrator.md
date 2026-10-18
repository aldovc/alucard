# Alucard orchestrator rewrite: Rust + SQLite

Low-dependency architecture.

Status: Proposed. Date: 2026-10-13. Tracking: #122.
Repository: aldovc/alucard.
Implementation: incremental replacement of the existing Bash orchestrator.

## 1. Executive summary

Replace Alucard's approximately 5,050-line Bash orchestration layer with a small,
statically linked Rust executable backed by SQLite.

The rewrite is primarily a reliability and maintainability project, not a
redesign of the agent system.

The Rust orchestrator will own task dispatch, transactional claims, run
lifecycle, recovery state, process management, and measurements. Existing agent
behavior, Docker isolation, engineering policy, prompts, and GitHub integration
should remain compatible wherever possible.

Architectural principle:

- GitHub remains authoritative for issues, pull requests, reviews, and remote
  repository state.
- SQLite becomes authoritative for local orchestration state, including claims,
  leases, execution attempts, and recovery tracking.
- Markdown remains the human-editable authoring interface for local tasks.
- Docker continues to execute agents in isolated containers.
- The host orchestrator coordinates the workflow without executing coding agents
  directly.

The primary improvement should be eliminating ambiguous ownership and improving
recovery after partial failures.

## 2. Goals

1. Replace the Bash orchestration engine with a maintainable Rust executable.
2. Make claims atomic, preventing concurrent local runs from dispatching the same
   task.
3. Eliminate reliance on GitHub labels and loosely structured PR bodies as the
   primary source of orchestration state.
4. Make every execution attempt attributable to a known task and run.
5. Preserve unfinished work and recover safely from crashes.
6. Maintain compatibility with existing GitHub issues, PRs, local task files, and
   agent configuration.
7. Preserve existing CI, review, feedback, and merge guarantees.
8. Improve observability without introducing external infrastructure.
9. Minimize runtime dependencies, dependency count, and host attack surface.

## 3. Non-goals

- Rewriting engineering policy or general agent instructions.
- Replacing Claude Code or Codex.
- Rebuilding the container environment.
- Replacing the Docker isolation model.
- Introducing distributed coordination.
- Adding a service daemon, scheduler, or web dashboard.
- Adding a generalized provider framework.
- Automatically invoking PR merges without an operator command.
- Replacing local Markdown task authoring with a database-only interface.
- Rewriting unrelated application or repository tooling.

The existing `alucard merge` command remains supported. It may merge after its
verification procedure, but only following an explicit operator invocation.

## 4. Existing behavior that must be preserved

The port must preserve the following contracts.

| Subsystem | Required behavior |
| --- | --- |
| GitHub queue | `ready-for-agent` eligibility, blockers, existing PR occupancy, explicit issue pinning |
| Local tasks | Parent context, file ordering, task IDs, dependencies, `[ ]`, `[>]`, `[x]`, `[h]` |
| Worker | One task per iteration, isolated checkout, branch and PR creation |
| Providers | Claude/Codex selection, per-role models, budgets, timeouts |
| CI | Polling, bounded repair attempts, missing-CI handling |
| Review | Verdict protocol, blocked findings, feedback cycles, current-head approval |
| Recovery | Wrap-up agent, unpublished changes, recovery PRs, authenticated markers |
| Continue | Resume feedback, CI, and review on an existing open PR |
| Merge | Approval validation, rebase, host-side checks, lease-protected push, explicit squash merge |
| Isolation | Existing Docker security flags, resource limits, temporary directories, credential model |
| Toolchain | Dependency preflight, Playwright cache, optional PostgreSQL |
| Measurements | Usage, costs, diff measurements, baseline SHA, archived prompts |
| CLI | Existing commands, flags, environment variables, default paths |

Existing PRs must remain manageable after the Rust implementation replaces Bash.

## 5. Architecture

```
                   alucard (Rust CLI)
                           |
          +----------------+----------------+
          |                |                |
     Task sources     Execution engine   Observability
          |                |                |
     GitHub issues     Claim manager     Run history
     Markdown files    Run supervisor    Events
          |            Recovery FSM      Measurements
          |                |                |
          +----------------+----------------+
                           |
                      SQLite ledger
                           |
          +----------------+----------------+
          |                                 |
     Host integrations                 Docker runtime
          |                                 |
       git / gh                       Worker agent
       GitHub API                     CI-fix agent
       File locks                     Reviewer agent
       Shared clones                  Feedback agent
       Merge checks                   Wrap-up agent
```

### Component boundaries

**Task source.** Fetches and normalizes GitHub issues or local Markdown tasks. It
does not own execution state.

**Queue engine.** Resolves dependencies, open-PR occupancy, priority, and
eligibility.

**Claim manager.** Uses SQLite transactions to allocate exactly one local owner
per task.

**Execution engine.** Manages agent attempts, process timeouts, subprocess
output, Docker resources, and work directories.

**Recovery engine.** Reconciles crashed runs with local resources and GitHub
state.

**GitHub adapter.** Uses `gh` for structured queries and remote side effects.
Remote failures must be distinguishable from empty results.

**Measurement engine.** Records run and attempt metadata while preserving the
existing JSONL reporting format.

Avoid creating a generic plugin architecture. A small collection of Rust modules
and narrowly defined interfaces is sufficient.

## 6. Technology decisions

| Concern | Decision |
| --- | --- |
| Language | Rust, stable toolchain |
| Database | `rusqlite` with bundled SQLite |
| Database access | Synchronous, parameterized SQL |
| Migrations | Versioned SQL with a small custom migration runner |
| CLI | `clap` |
| Serialization | `serde` and `serde_json` |
| Identifiers | UUIDs for runs, attempts, and leases |
| Logging | `tracing`, with a minimal subscriber |
| GitHub | Shell out to `gh` |
| Git | Shell out to `git` |
| Docker | Shell out to `docker` |
| Async runtime | None initially |
| File locking | Preserve OS-level advisory locking |
| ORM | None |
| Background service | None |

### Why rusqlite

Prefer `rusqlite` over `sqlx` for v1.

Alucard does not require asynchronous database access, connection pooling, or a
database abstraction layer. Using `rusqlite` keeps dependency count and
architectural complexity lower.

### Why external CLI integrations

Continue using `gh`, `git`, and `docker` rather than adding API clients
immediately.

Benefits:

- Preserves the existing authentication model.
- Avoids duplicate implementations of Git and Docker operations.
- Keeps the Rust dependency graph smaller.
- Minimizes behavior changes during migration.

Every subprocess should use explicit arguments, controlled working directories,
captured exit status, and bounded execution where appropriate.

Avoid shell command construction for dynamic values. Retain a shell only where
user-supplied command execution is an explicit existing feature, such as
`merge --check-command`.

### Static binary definition

The intended output is a statically linked Linux executable, targeting supported
architectures such as x86-64 and ARM64.

Static linking must be verified in release CI, not merely assumed from Rust or
the bundled SQLite feature.

External tools (`git`, `gh`, `docker`, and the shell used by merge checks) remain
host prerequisites.

## 7. SQLite ledger

### Location

Default: `$XDG_STATE_HOME/alucard/alucard.db`

Fallback: `~/.local/state/alucard/alucard.db`

Add a `--db-path` override and an `ALUCARD_DB_PATH` environment variable.

### Database requirements

- WAL journaling.
- Foreign-key enforcement.
- Busy timeout for concurrent writers.
- Configurable connection-level pragmas.
- Restricted local file permissions.
- Versioned schema migrations.
- No stored API keys, provider credentials, or other unnecessary secrets.

WAL and foreign-key settings should be established at connection initialization.
They should not rely solely on migration SQL.

### Initial schema

Migration `0001_init.sql` should contain the entities needed to recover safely,
without introducing a general-purpose data model.

#### repositories

Normalized repository identity, including GitHub owner/repo, and optional
cache-path metadata.

#### runs

| Field | Description |
| --- | --- |
| id | UUID primary key |
| repository_id | Repository foreign key |
| command | `run`, `continue`, `merge` |
| status | `running`, `completed`, `interrupted`, `failed` |
| started_at | UTC timestamp |
| heartbeat_at | Last active heartbeat |
| ended_at | UTC timestamp, nullable |
| base_sha | Baseline commit when applicable |
| image | Docker image reference |
| logs_path | Run artifact location |

#### tasks

| Field | Description |
| --- | --- |
| id | Internal task UUID |
| repository_id | Repository foreign key |
| source | `github` or `local` |
| source_path | Local Markdown file identity; empty for GitHub |
| external_id | GitHub issue number or local task ID |
| title | Cached title |
| status | `pending`, `claimed`, `in_flight`, `done`, `human` |
| blockers_json | Normalized dependency references |
| pr_number | Current PR, nullable |
| previous_pr_number | Previous failed/closed PR, nullable |
| updated_at | UTC timestamp |

Enforce a unique constraint across
`(repository_id, source, source_path, external_id)`.

A task ID such as `gh:123` is not globally unique. It can collide with another
repository's issue 123.

#### claims

| Field | Description |
| --- | --- |
| id | Unique lease identifier |
| repository_id | Repository |
| resource_kind | `task` or `pr` |
| resource_key | Task UUID or PR number |
| run_id | Owning run |
| state | `active` or `released` |
| acquired_at | UTC timestamp |
| heartbeat_at | Most recent heartbeat |

Use a partial unique index on repository, resource kind, and resource key where
state is active.

This supports both task claims and exclusive continue/merge operations on a PR.

#### pull_requests

Stores task linkage, PR number, branch, head SHA, normal/recovery classification,
current remote state, gate state, and reviewer-approved SHA.

The PR's actual open/closed/merged state must always be verified against GitHub.

#### recovery

Stores task, original run, recovery branch, recovery PR number, recovery reason,
marker status, unpark status, and creation timestamp.

#### operations

A minimal durable journal of important remote side effects.

Examples:

- Push recovery branch.
- Create recovery PR.
- Post recovery marker.
- Apply or remove a GitHub label.
- Mark recovery PR ready.
- Update PR attribution.

Each operation has an ID, target, operation type, phase, attempt count, and last
error.

Its purpose is replay and reconciliation, not a general job queue.

### Why an operations journal is necessary

SQLite transactions cannot atomically commit a GitHub API request.

For example, Alucard can successfully create a recovery PR and crash before
recording its number in SQLite. Without reconciliation metadata, a restart may
create a duplicate PR.

Recording the intended operation before performing it, and verifying its external
result before retrying, makes the workflow recoverable across that boundary.

## 8. Task state model

Avoid treating a PR being opened as a task being completed.

### Local execution states

```
PENDING
   |
   | atomic claim
   v
CLAIMED
   |
   +---- no useful work ----------> PENDING
   |
   +---- normal PR created -------> IN_FLIGHT
   |
   +---- recovery PR created -----> IN_FLIGHT
   |
   +---- human-only decision -----> HUMAN

IN_FLIGHT
   |
   +---- PR merged ---------------> DONE
   |
   +---- PR closed unmerged ------> PENDING
   |
   +---- PR remains open ---------> IN_FLIGHT
```

Keep review and recovery conditions separate from the task lifecycle.

For example, an `IN_FLIGHT` task can have a PR whose gate state is `approved`,
`changes_requested`, `needs_human`, or `recovery_parked`.

A review approval does not mean the underlying task is complete.

For GitHub issues, the issue's actual open/closed state takes precedence when
determining whether the external task is finished.

## 9. Claim protocol

### Main architectural change

The orchestrator must select and claim a task before invoking the worker.

Currently, GitHub-mode workers select a ticket from a supplied queue and apply
the in-progress label themselves. That cannot provide transactional ownership
because SQLite does not know the selected task until the worker announces it.

### New dispatch sequence

1. Build the eligible queue.
2. Choose one task.
3. Begin an immediate SQLite transaction.
4. Recheck task eligibility and active claims.
5. Create a unique active claim for this run.
6. Commit the transaction.
7. Apply the GitHub visibility label, where applicable.
8. Dispatch exactly the claimed task to the worker.
9. Update the ledger when a PR is discovered or a recovery path begins.

The worker receives a pinned task rather than choosing freely from a queue.

This requires a narrow amendment to the GitHub-mode task-selection instructions.
It does not require rewriting the shared engineering policy or role prompts.

### GitHub prioritization

The current GitHub worker prioritizes critical fixes, infrastructure,
independently mergeable features, and other work.

Moving selection into Rust is an intentional behavior change.

For v1, define deterministic priority from explicit GitHub labels, followed by a
stable issue ordering. Preserve `--issue` for explicit operator selection.

Local Markdown mode continues using file order.

### Claim guarantees

- A task has at most one active local claim.
- A pinned issue must also respect active claims.
- No claim is held inside a long-running SQL transaction.
- Claims have heartbeats and a unique lease identity.
- The owning run must release or transition its claim explicitly.
- A claim older than a threshold is suspicious, not automatically safe to
  release.

### Stale claim handling

Do not release claims solely because they are old.

First establish whether the owner is still active, whether the Docker workload
remains active, and whether a branch or PR has already been created.

If safety cannot be established, mark the resource for reconciliation or human
intervention rather than risk duplicate execution.

### Filesystem locks

SQLite protects logical task ownership.

Filesystem locks continue protecting shared Git operations, including cache
cloning, fetching, and operations that use shared Git state.

Do not hold a filesystem lock for an entire worker or CI/review cycle.

Local-task-file rewrites require their own serialization and conflict checks.

## 10. Worker execution

An iteration proceeds as follows:

1. Resolve the selected task and acquire its lease.
2. Fetch the configured base branch.
3. Record the exact base SHA.
4. Create a unique run directory and isolated local clone.
5. Create a branch containing the run identity and task identity.
6. Run dependency preflight as required.
7. Invoke the existing worker container.
8. Capture structured output, exit status, usage, and diagnostics.
9. Discover and validate the resulting PR.
10. Transition to CI/review, recovery, or claim release.

Preserve the existing practice of using a self-contained Git clone rather than a
Git worktree whose `.git` file references a host-only path.

### Retry behavior

Preserve transport retries with fresh clones, including the existing configurable
retry budget.

Before retrying, check whether the previous attempt actually created a PR.

If a worker lost its connection after successfully publishing, adopt the
already-created PR instead of starting another attempt.

Every retry gets a distinct attempt ID and workspace.

Provider billing failures remain terminal for the current run.

## 11. Recovery guarantees

Recovery must account for local work, remote branches, PRs, and database records
independently.

### Worker failed with useful changes and no PR

1. Inspect commits and uncommitted changes.
2. Invoke wrap-up when the existing policy permits it.
3. Commit remaining changes mechanically when necessary.
4. Push the recovery branch.
5. Create a draft recovery PR.
6. Record its remote identity and task linkage.
7. Apply the authenticated recovery marker and needs-human label.
8. Release the active task claim, keeping the task associated with the recovery
   PR.

Use `Refs #N`, not `Closes #N`, for unfinished GitHub work.

### Worker failed without useful changes

Release the claim after checking for an already-published PR.

Preserve the existing special case for terminal transport failures with no worker
commits: a branch may be retained without creating a recovery PR.

### Remote publication failure

Do not discard a checkout containing unpreserved work.

Record its path and failure reason. Report it as needing human attention.

Cleanup is best effort, but unpublished useful work must not be deleted merely
because cleanup ran.

### Crash recovery

On startup:

1. Find unfinished runs and active leases.
2. Inspect owned Docker containers and retained work directories.
3. Reconcile recorded branches and PRs with GitHub.
4. Recover interrupted remote operations.
5. Reconstruct missing local links when sufficient evidence exists.
6. Release only safely abandoned claims.
7. Mark unresolved ambiguity explicitly.

Use deterministic branch naming and operation identifiers to make remote
discovery reliable.

### Existing PR compatibility

Continue recognizing the current Alucard recovery markers, blocked-findings
comments, and reviewed-head markers.

New internal SQLite state must not make existing Bash-created PRs impossible to
resume.

## 12. CI, review, and continue

Preserve the existing gate order and bounded retry behavior.

### CI

- Up to three check attempts per gate.
- At most two CI-fix executions between attempts.
- Current-head checks only.
- The existing polling timeout and fallback behavior.
- Missing CI is not treated as successful CI.

### Review

- `APPROVED`
- `CHANGES_REQUESTED`
- `BLOCKED`
- Missing or invalid verdict
- Review-cycle exhaustion
- Feedback stopped without publishing

Continue preserving blocked findings across cycles and runs.

An approval must identify the commit that the harness reviewer actually
reviewed.

A recovery PR is unparked only when the current head meets the existing approval
and CI requirements.

### `alucard continue <PR>`

Continue must work for ordinary PRs as well as recovery PRs.

It should acquire an exclusive PR lease, load relevant human and Alucard
comments, execute feedback when appropriate, and rerun CI and review.

Two local processes must not mutate the same PR concurrently.

## 13. Manual merge workflow

Keep:

```
alucard merge <PR_NUM> [TARGET_REPO]
```

The merge workflow must:

1. Require explicit operator invocation.
2. Verify that the latest harness approval applies to the original current PR
   head.
3. Reject unsupported fork-based branch operations as today.
4. Fetch the PR branch and current base.
5. Rebase if necessary.
6. Stop on rebase conflicts without pushing.
7. Run the explicitly configured host-side check command.
8. Stop if checks fail.
9. Push a rebased branch using force-with-lease.
10. Squash-merge with head-SHA protection.
11. Record the outcome and perform best-effort branch cleanup.

Preserve `--dry-run`.

The check command must continue coming from trusted operator configuration, not
PR-controlled content.

No autonomous background merge behavior is introduced.

## 14. Local Markdown tasks

### Ownership model

Markdown owns task authoring; SQLite owns execution coordination.

Markdown remains responsible for:

- Task definitions and acceptance criteria.
- Parent context.
- Task ordering.
- Dependency declarations.
- Human-authored state changes.

SQLite owns:

- Claims and active execution.
- Run attribution.
- PR and recovery linkage.
- Local reconciliation metadata.

### Synchronization

Use bidirectional, conflict-aware synchronization.

On queue construction:

1. Parse and validate the Markdown file.
2. Import new tasks and authored changes.
3. Reconcile execution state with GitHub.
4. Apply safe state transitions to the ledger.
5. Reflect compatible status changes to Markdown when necessary.

Preserve the current heading states:

- `[ ]`: pending
- `[>]`: PR in flight
- `[x]`: done
- `[h]`: human-only

Preserve PR suffixes and previous-attempt references.

### Conflict rules

- An open PR prevents accidental re-dispatch, even if the Markdown heading was
  reset.
- A human change to `[h]` prevents future dispatch.
- A human change to task text or priority should survive reconciliation.
- A merged PR may transition the task to `[x]`.
- A closed, unmerged PR may restore `[ ]`, retaining previous-attempt
  information.
- A GitHub API failure must not cause guessed state transitions.
- A conflicting human edit must not be silently overwritten.

Use file locks, content checks, temporary files, and atomic rename when writing
Markdown.

Only change the required heading lines; preserve unrelated authored content and
formatting.

This behavior is particularly important because local tasks were deliberately
designed to be a simple, editable planning interface.

## 15. CLI compatibility

Keep the existing command structure:

```
alucard run [TARGET_REPO] [options]
alucard queue [TARGET_REPO] [options]
alucard build [options]
alucard doctor [TARGET_REPO] [options]
alucard continue <PR_NUM> [TARGET_REPO] [options]
alucard merge <PR_NUM> [TARGET_REPO] [options]
alucard version
```

Add:

```
alucard db migrate
alucard db status
alucard runs
alucard runs show <RUN_ID>
```

Preserve existing positional repository arguments and `--repo` compatibility.

Preserve existing flags including `--issue`, `--tasks`, `--github`, `--base`,
`--iterations`, `--timeout-minutes`, `--max-review-cycles`, `--image`,
`--no-build`, `--logs-root`, `--check-command`, and `--dry-run` where applicable.

`queue` should retain its existing JSON output contract unless an explicit
format/version flag is introduced.

`doctor` should validate SQLite health and migrations in addition to existing
environment, credentials, toolchain, and task-file checks.

## 16. Observability

SQLite provides the structured lifecycle index; logs retain detailed execution
evidence.

### Structured state

Track:

- Run start and end.
- Task acquisition and release.
- Agent invocation and retries.
- Docker exit and timeout classification.
- PR discovery or creation.
- CI and review outcomes.
- Recovery state transitions.
- Human intervention reasons.

### Compatibility

Preserve:

- `events.log`
- Agent JSONL streams.
- `measurements.jsonl`
- Toolchain preflight logs.
- Archived prompts.
- End-of-run usage summaries.

Do not discard existing measurement distinctions, including unavailable usage,
provider-reported cost, baseline SHA, and diff drift.

SQLite should initially index summaries and file paths rather than duplicate
entire agent transcripts.

Avoid storing secrets in structured logs, SQL fields, or subprocess error
messages.

## 17. Testing and behavioral parity

The Bash implementation and its test suite provide the initial behavioral
baseline.

However, Bash tests that source shell functions cannot automatically test the
Rust implementation. Their assertions must be transferred into black-box contract
tests or equivalent Rust tests.

### Required categories

**Database.**

- Migration application and idempotency.
- Schema compatibility.
- Foreign-key enforcement.
- Unique active claims.
- Concurrent transaction contention.
- Recovery after interrupted transactions.

**Concurrency.**

- Multiple runs selecting the same eligible task.
- Different tasks in one repository running concurrently.
- Concurrent continue on the same PR.
- Unique run directories and branches.
- Shared-clone lock contention.
- Safe cleanup of independently owned workspaces.

**Crash and recovery.** Inject failures at each remote publication boundary:

- Before and after branch push.
- Before and after PR creation.
- Before and after recording the PR in SQLite.
- Before and after marker publication.
- During CI, feedback, and review.
- During recovery unparking.

Assert that repeated reconciliation does not create duplicate PRs, dispatch
occupied tasks, or discard useful work.

**Compatibility.** Port the existing behavior tested by the repository's task
parser, concurrency, worker recovery, timeout recovery, PR binding, review gate,
feedback stall, measurements, and merge tests.

**Safety.** Verify that automatic execution cannot push to the configured base
branch or invoke merging. Verify that an explicit merge operation still enforces
the reviewed-head and verification requirements.

## 18. Migration and rollout

### Phase 0: baseline and contract freeze

- Capture existing CLI behavior and representative fixtures.
- Identify the invariants in current Bash tests.
- Document current remote marker formats.
- Freeze schema v1 and state transitions.
- Record baseline measurements for representative workloads.

Exit criterion: a reviewed behavior matrix and migration design exist.

### Phase 1: Rust foundation

Implement:

- CLI parsing and configuration.
- Database initialization and migrations.
- Run records.
- Task imports.
- Atomic claims and PR leases.
- Queue and doctor.
- Process execution abstraction.

No coding agents are dispatched.

Exit criterion: concurrent claim tests pass and existing queue semantics are
represented correctly.

### Phase 2: worker and recovery

Port:

- Docker invocation.
- Agent/provider settings.
- Work directory lifecycle.
- Retry classification.
- PR binding.
- Wrap-up and recovery PR creation.
- Operations journal and crash recovery.

Exit criterion: worker/recovery contract tests pass, including crash injection.

### Phase 3: gates and merge

Port:

- CI polling and repair.
- Review and feedback cycles.
- `continue`.
- Authenticated recovery unparking.
- Explicit merge.
- Measurements and usage reporting.

Exit criterion: existing gate and merge safety contracts pass.

### Phase 4: shadow validation

Run the Rust implementation against recorded fixtures and read-only observations
from real repositories.

Do not allow Bash and Rust to independently perform the same live task or PR
workflow.

For write-path validation, use isolated test repositories and separate task
fixtures.

Compare outcomes, PR attribution, side effects, measurements, and human handoffs.

Exit criterion: no unexplained behavioral differences or safety regressions
remain.

### Phase 5: controlled cutover

- Ship a release binary and updated installer.
- Bootstrap SQLite state from existing open PRs, recovery markers, and local task
  files.
- Preserve existing logs and credentials.
- Switch default execution to Rust.
- Retain Bash as an explicitly selected fallback for one release.
- Prevent mixed-version concurrent operation against the same repository during
  transition.

Exit criterion: normal workflows complete through the Rust binary without
fallback.

### Phase 6: cleanup

- Remove the Bash orchestration implementation.
- Retain container entrypoint scripts where still needed.
- Remove legacy-only compatibility code after the transition period.
- Consolidate documentation and CI.
- Keep supported existing PR marker formats readable.

## 19. Retention policy

Default behavior:

- Preserve run records and recovery records indefinitely.
- Preserve audit-critical task/PR relationships.
- Do not delete unresolved recovery state.
- Keep existing filesystem logs under their configured location.
- Do not introduce automatic log deletion in v1.

Retention and pruning can be added later as explicit maintenance commands.

## 20. Acceptance criteria

The rewrite is complete when:

1. Existing GitHub and local task workflows remain compatible.
2. No two cooperating local runs can claim the same task simultaneously.
3. Concurrent runs cannot accidentally share or delete each other's working
   directories.
4. A crash after remote PR publication does not create duplicate recovery PRs.
5. Recovery does not discard unpublished useful work.
6. Existing open Bash-created PRs remain resumable.
7. Review approval remains bound to the reviewed head SHA.
8. Missing CI does not silently qualify a PR as merge-ready.
9. Automatic runs never merge or push to the base branch.
10. Explicit `alucard merge` preserves the existing safety requirements.
11. SQLite state can be reconciled against GitHub after interruption.
12. Run and usage measurements remain comparable with Bash output.
13. No additional persistent service or heavyweight framework is required.
14. The release provides a verified statically linked Rust executable for
    supported Linux architectures.

## 21. Decisions for v1

| Question | Decision |
| --- | --- |
| SQLite library | `rusqlite` with bundled SQLite |
| Async | Synchronous first; no Tokio unless demonstrated necessary |
| GitHub integration | `gh` CLI |
| Docker integration | `docker` CLI |
| Git integration | `git` CLI |
| Claim authority | SQLite active leases |
| Task selection | Host orchestrator before dispatch |
| Shared Git safety | Short-lived filesystem locks |
| Local tasks | Markdown authoring with bidirectional state reconciliation |
| Remote consistency | Journaled operations and idempotent reconciliation |
| Recovery | Preserve existing markers and current-head approval |
| Merge | Explicit operator command only |
| Measurements | Existing JSONL retained; SQLite metadata added |
| Retention | No automatic deletion in v1 |
| Rollout | Behavior-driven phases, read-only shadowing, isolated write tests |
| Provider support | Existing Claude/Codex behavior; broader abstraction deferred |

## 22. Guiding principle

Alucard already has a functional agent execution system. Its most important
weaknesses are not the agents themselves, but the orchestration boundaries around
them: task ownership, remote side effects, local filesystem state, and recovery
after interruption.

The Rust implementation should make those boundaries explicit.

Success means Alucard remains recognizable to its existing users, while becoming
easier to reason about, safer to run concurrently, and more resilient when
something fails halfway through an operation.

The goal is not to reproduce 5,000 lines of Bash in Rust. It is to express the
same verified behavior with fewer implicit assumptions and a smaller, testable
orchestration core.
