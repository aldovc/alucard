use alucard::{
    github_queue, github_ticket_blockers, import_github_tasks, import_local_tasks, local_queue,
    local_queue_with_open_issues, migrate, open, parse_tasks, resolve_pinned_issue,
    write_task_headings, GithubPullRequest, GithubTask, TaskHeadingUpdate, TaskState,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temporary_path(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("alucard-{name}-{}-{unique}", std::process::id()))
}

#[test]
fn task_heading_write_preserves_non_heading_bytes() {
    let path = temporary_path("task-heading-write");
    let original =
        "# Plan\r\n\r\n## [>] one: First (PR #12)\r\n\r\nBody  \r\n\r\n## [ ] two: Second\r\n";
    fs::write(&path, original).unwrap();
    write_task_headings(
        &path,
        original,
        &[TaskHeadingUpdate {
            id: "one".to_string(),
            state: TaskState::Pending,
            pr_number: None,
            previous_pr_number: Some(12),
        }],
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "# Plan\r\n\r\n## [ ] one: First (previous attempt: PR #12)\r\n\r\nBody  \r\n\r\n## [ ] two: Second\r\n"
    );
    fs::remove_file(path).unwrap();
}

#[test]
fn task_heading_write_rejects_changed_file() {
    let path = temporary_path("task-heading-conflict");
    let original = "## [>] one: First (PR #12)\n";
    fs::write(&path, "## [h] one: Human edit\n").unwrap();
    let error = write_task_headings(
        &path,
        original,
        &[TaskHeadingUpdate {
            id: "one".to_string(),
            state: TaskState::Done,
            pr_number: Some(12),
            previous_pr_number: None,
        }],
    )
    .unwrap_err();
    assert!(error.contains("changed during reconciliation"));
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "## [h] one: Human edit\n"
    );
    fs::remove_file(path).unwrap();
}

fn run(command: &mut Command) {
    assert!(command.status().unwrap().success());
}

fn wait_for_file(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn git_repository(root: &Path) {
    run(Command::new("git").args(["init", "-q", "-b", "main", root.to_str().unwrap()]));
    fs::write(root.join("README.md"), "test\n").unwrap();
    run(Command::new("git").args(["-C", root.to_str().unwrap(), "add", "README.md"]));
    run(Command::new("git").args([
        "-C",
        root.to_str().unwrap(),
        "-c",
        "user.email=test@example.com",
        "-c",
        "user.name=Test",
        "commit",
        "-qm",
        "initial",
    ]));
}

fn run_transport_case(root: &Path, scenario: &str, retries: usize) -> std::process::Output {
    let repository = root.join("repository");
    let remote = root.join("remote.git");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    run(Command::new("git").args(["init", "--bare", "-q", remote.to_str().unwrap()]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "remote",
        "add",
        "origin",
        remote.to_str().unwrap(),
    ]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "push",
        "-q",
        "origin",
        "main",
    ]));
    let foreign_head = if scenario == "foreign" {
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "switch",
            "-q",
            "-c",
            "foreign",
        ]));
        fs::write(repository.join("foreign.txt"), "foreign\n").unwrap();
        run(Command::new("git").args(["-C", repository.to_str().unwrap(), "add", "."]));
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "-c",
            "user.email=test@example.com",
            "-c",
            "user.name=Test",
            "commit",
            "-qm",
            "foreign",
        ]));
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "push",
            "-q",
            "origin",
            "foreign",
        ]));
        let head = Command::new("git")
            .args(["-C", repository.to_str().unwrap(), "rev-parse", "HEAD"])
            .output()
            .unwrap();
        run(Command::new("git").args(["-C", repository.to_str().unwrap(), "switch", "-q", "main"]));
        String::from_utf8(head.stdout).unwrap().trim().to_string()
    } else {
        String::new()
    };
    let trace = root.join("trace");
    let docker = bin.join("docker");
    fs::write(
        &docker,
        r#"#!/bin/sh
case "$1 $2" in
  'image inspect') echo stamp ;;
  'run '*)
    for argument in "$@"; do case "$argument" in *:/work:rw) worktree=${argument%:/work:rw} ;; esac; done
    printf '%s|%s\n' "$(git -C "$worktree" branch --show-current)" "$worktree" >> "$ALUCARD_TRACE"
    case "$ALUCARD_SCENARIO" in
      billing) printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"error":"billing_error"}' ;;
      *) printf '%s\n' '{"type":"result","subtype":"success","is_error":true,"result":"API Error: Connection closed"}' ;;
    esac
    [ "$ALUCARD_SCENARIO" = foreign ] && exit 0
    exit 1 ;;
  'rm -f') ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$ALUCARD_GH_TRACE"
case "$1 $2" in
  'pr list')
    case "$*" in
      *--head*)
        if [ "$ALUCARD_SCENARIO" = pr ]; then
          printf '%s\n' '[{"number":59,"headRefName":"attempt","headRefOid":"attempt"}]'
        else echo '[]'; fi ;;
      *--label*)
        if [ "$ALUCARD_SCENARIO" = foreign ]; then
          printf '[{"number":60,"headRefName":"foreign","headRefOid":"%s"}]\n' "$ALUCARD_FOREIGN_HEAD"
        else echo '[]'; fi ;;
    esac ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let tasks = root.join("tasks.md");
    fs::write(&tasks, "## [ ] 1: Transport task\n\nBody\n").unwrap();
    let environment = root.join("agent.env");
    fs::write(
        &environment,
        "ALUCARD_PROVIDER=claude\nANTHROPIC_API_KEY=test\n",
    )
    .unwrap();
    Command::new(env!("CARGO_BIN_EXE_alucard"))
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .env("ALUCARD_TRACE", &trace)
        .env("ALUCARD_GH_TRACE", root.join("gh-trace"))
        .env("ALUCARD_SCENARIO", scenario)
        .env("ALUCARD_FOREIGN_HEAD", foreign_head)
        .env("ALUCARD_CACHE_DIR", root.join("cache"))
        .env("ALUCARD_TRANSPORT_RETRY_ATTEMPTS", retries.to_string())
        .args([
            "--db-path",
            root.join("ledger.db").to_str().unwrap(),
            "run",
            "--iterations",
            "1",
            "--tasks",
            tasks.to_str().unwrap(),
            "--env-file",
            environment.to_str().unwrap(),
            "--logs-root",
            root.join("logs").to_str().unwrap(),
            "--no-build",
            repository.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

#[test]
fn recovery_publication_failures_are_retained_and_parked_for_humans() {
    for scenario in ["push", "pr", "marker", "label", "alucard_label", "success"] {
        let root = temporary_path(&format!("recovery-{scenario}"));
        let repository = root.join("repository");
        let remote = root.join("remote.git");
        let bin = root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir(&repository).unwrap();
        git_repository(&repository);
        run(Command::new("git").args(["init", "--bare", "-q", remote.to_str().unwrap()]));
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            remote.to_str().unwrap(),
        ]));
        run(Command::new("git").args([
            "-C",
            repository.to_str().unwrap(),
            "push",
            "-q",
            "origin",
            "main",
        ]));
        let git = bin.join("git");
        fs::write(
            &git,
            "#!/bin/sh\nif [ \"$1\" = push ] && [ \"$ALUCARD_RECOVERY_SCENARIO\" = push ]; then echo 'push denied' >&2; exit 1; fi\nexec \"$ALUCARD_REAL_GIT\" \"$@\"\n",
        )
        .unwrap();
        fs::set_permissions(&git, fs::Permissions::from_mode(0o755)).unwrap();
        let docker = bin.join("docker");
        fs::write(
            &docker,
            r#"#!/bin/sh
case "$1 $2" in
  'image inspect') echo stamp ;;
  'run '*)
    for argument in "$@"; do case "$argument" in *:/work:rw) worktree=${argument%:/work:rw} ;; esac; done
    printf 'unfinished\n' > "$worktree/recovery.txt"
    git -C "$worktree" add recovery.txt
    git -C "$worktree" -c user.email=test@example.com -c user.name=Test commit -qm recovery
    printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"failed"}'
    exit 1 ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
        let gh = bin.join("gh");
        fs::write(
            &gh,
            r#"#!/bin/sh
printf '%s\n' "$*" >> "$ALUCARD_RECOVERY_TRACE"
case "$1 $2" in
  'pr list')
    if [ -f "$ALUCARD_RECOVERY_CREATED" ]; then echo '[{"number":91,"headRefName":"recovery","headRefOid":"abc"}]'; else echo '[]'; fi ;;
  'pr create')
    if [ "$ALUCARD_RECOVERY_SCENARIO" = pr ]; then echo 'create denied' >&2; exit 1; fi
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --body ]; then printf '%s' "$2" > "$ALUCARD_RECOVERY_BODY"; break; fi
      shift
    done
    : > "$ALUCARD_RECOVERY_CREATED" ;;
  'pr comment')
    if [ "$ALUCARD_RECOVERY_SCENARIO" = marker ]; then echo 'marker denied' >&2; exit 1; fi
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --body ]; then printf '%s' "$2" > "$ALUCARD_RECOVERY_MARKER"; break; fi
      shift
    done ;;
  'pr edit')
    for argument in "$@"; do
      if [ "$ALUCARD_RECOVERY_SCENARIO" = label ] && [ "$argument" = needs-human ]; then echo 'label denied' >&2; exit 1; fi
      if [ "$ALUCARD_RECOVERY_SCENARIO" = alucard_label ] && [ "$argument" = alucard ]; then echo 'alucard label denied' >&2; exit 1; fi
    done ;;
esac
"#,
        )
        .unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        let tasks = root.join("tasks.md");
        fs::write(&tasks, "## [ ] 1: Recovery task\n\nBody\n").unwrap();
        let environment = root.join("agent.env");
        fs::write(
            &environment,
            "ALUCARD_PROVIDER=claude\nANTHROPIC_API_KEY=test\n",
        )
        .unwrap();
        let database = root.join("ledger.db");
        let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("ALUCARD_REAL_GIT", "/usr/bin/git")
            .env("ALUCARD_RECOVERY_SCENARIO", scenario)
            .env("ALUCARD_RECOVERY_CREATED", root.join("recovery-created"))
            .env("ALUCARD_RECOVERY_BODY", root.join("recovery-body"))
            .env("ALUCARD_RECOVERY_MARKER", root.join("recovery-marker"))
            .env("ALUCARD_RECOVERY_TRACE", root.join("recovery-trace"))
            .env("ALUCARD_CACHE_DIR", root.join("cache"))
            .args([
                "--db-path",
                database.to_str().unwrap(),
                "run",
                "--iterations",
                "1",
                "--tasks",
                tasks.to_str().unwrap(),
                "--env-file",
                environment.to_str().unwrap(),
                "--logs-root",
                root.join("logs").to_str().unwrap(),
                "--no-build",
                repository.to_str().unwrap(),
            ])
            .output()
            .unwrap();
        assert!(output.status.success(), "{scenario}: {output:?}");
        let connection = open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT status FROM tasks", [], |row| row
                    .get::<_, String>(0))
                .unwrap(),
            if scenario == "success" {
                "in_flight"
            } else {
                "human"
            },
            "{scenario}"
        );
        if scenario == "marker" {
            let trace = fs::read_to_string(root.join("recovery-trace")).unwrap();
            assert!(trace.contains("pr comment"), "{scenario}: {trace}");
            assert!(trace.contains("pr edit"), "{scenario}: {trace}");
        }
        if scenario == "label" {
            let trace = fs::read_to_string(root.join("recovery-trace")).unwrap();
            assert!(
                trace.contains("pr edit 91 --add-label needs-human"),
                "{trace}"
            );
            assert!(!trace.contains("pr comment"), "{trace}");
            assert!(!root.join("recovery-marker").exists(), "{trace}");
            let phase: String = connection
                .query_row(
                    "SELECT phase FROM operations WHERE target = 'recovery-pr:91' AND operation_type = 'needs-human-label'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(phase, "failed");
        }
        if scenario == "alucard_label" {
            let phase: String = connection
                .query_row(
                    "SELECT phase FROM operations WHERE target = 'recovery-pr:91' AND operation_type = 'alucard-label'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(phase, "failed");
        }
        if scenario == "success" {
            let body = fs::read_to_string(root.join("recovery-body")).unwrap();
            assert!(body.starts_with("Task: 1\n\n"), "{body}");
            assert!(body.contains("This is not a finished change."), "{body}");
            assert!(
                body.contains("the harness flips the task to `[x]` when this PR merges."),
                "{body}"
            );
            assert!(
                body.contains("task flips back to `[ ]` at the next queue build"),
                "{body}"
            );
            assert!(
                body.contains("It does not resume implementation."),
                "{body}"
            );
            assert_eq!(
                fs::read_to_string(root.join("recovery-marker")).unwrap(),
                "**🤖 Alucard recovery PR parked** — opened as a draft and labeled `needs-human` by the harness. When the review gate approves this PR it marks it ready, removes the label, turns the `Refs` line into `Closes`, and replaces the stub title with the ticket's."
            );
            let trace = fs::read_to_string(root.join("recovery-trace")).unwrap();
            assert!(
                trace.find("pr edit 91 --add-label needs-human")
                    < trace.find("pr comment 91 --body"),
                "{trace}"
            );
        }
        let run_id: String = connection
            .query_row("SELECT id FROM runs", [], |row| row.get(0))
            .unwrap();
        let recovery: (Option<String>, Option<String>) = connection
            .query_row(
                "SELECT checkout_path, failure_reason FROM recovery WHERE original_run_id = ?1",
                [&run_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        drop(connection);
        if matches!(scenario, "push" | "pr") {
            assert!(recovery.0.is_some(), "{scenario}");
            assert!(recovery.1.is_some(), "{scenario}");
            let shown = Command::new(env!("CARGO_BIN_EXE_alucard"))
                .args([
                    "--db-path",
                    database.to_str().unwrap(),
                    "runs",
                    "show",
                    &run_id,
                ])
                .output()
                .unwrap();
            let shown = String::from_utf8(shown.stdout).unwrap();
            assert!(shown.contains("recovery checkout:"), "{shown}");
            assert!(shown.contains("recovery failure:"), "{shown}");
            assert!(shown.contains(recovery.0.as_deref().unwrap()), "{shown}");
            assert!(shown.contains(recovery.1.as_deref().unwrap()), "{shown}");
        }
        fs::remove_dir_all(root).unwrap();
    }
}

fn run_github_recovery_case(root: &Path, scenario: &str) {
    let repository = root.join("repository");
    let remote = root.join("remote.git");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    run(Command::new("git").args(["init", "--bare", "-q", remote.to_str().unwrap()]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "remote",
        "add",
        "origin",
        remote.to_str().unwrap(),
    ]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "push",
        "-q",
        "origin",
        "main",
    ]));
    let docker = bin.join("docker");
    fs::write(
        &docker,
        r#"#!/bin/sh
case "$1 $2" in
  'image inspect') echo stamp ;;
  'run '*)
    printf x >> "$ALUCARD_GITHUB_RECOVERY_DOCKER_TRACE"
    for argument in "$@"; do case "$argument" in *:/work:rw) worktree=${argument%:/work:rw} ;; esac; done
    case "$ALUCARD_GITHUB_RECOVERY_SCENARIO" in
      timeout)
        printf 'unfinished\n' > "$worktree/recovery.txt"
        exit 124 ;;
      *)
        printf 'unfinished\n' > "$worktree/recovery.txt"
        git -C "$worktree" add recovery.txt
        git -C "$worktree" -c user.email=test@example.com -c user.name=Test commit -qm recovery
        if [ "$ALUCARD_GITHUB_RECOVERY_SCENARIO" = turn_budget ]; then
          printf '%s\n' '{"type":"result","subtype":"error_max_turns","is_error":true,"result":"limit reached"}'
        else
          printf '%s\n' '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"failed"}'
        fi
        exit 1 ;;
    esac ;;
  'rm -f') ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        r#"#!/bin/sh
printf '%s\n' "$*" >> "$ALUCARD_GITHUB_RECOVERY_GH_TRACE"
case "$1 $2" in
  'issue list')
    case "$*" in
      *--label*) echo '[{"number":134,"title":"Recovery task","body":"","labels":[{"name":"ready-for-agent"}]}]' ;;
      *) echo '[{"number":134}]' ;;
    esac ;;
  'repo view') echo 'owner/repo' ;;
  'api graphql') echo '[]' ;;
  'pr list')
    if [ -f "$ALUCARD_GITHUB_RECOVERY_CREATED" ]; then echo '[{"number":91,"headRefName":"recovery","headRefOid":"abc"}]'; else echo '[]'; fi ;;
  'pr create')
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --body) printf '%s' "$2" > "$ALUCARD_GITHUB_RECOVERY_BODY"; shift 2 ;;
        --title) printf '%s' "$2" > "$ALUCARD_GITHUB_RECOVERY_TITLE"; shift 2 ;;
        *) shift ;;
      esac
    done
    : > "$ALUCARD_GITHUB_RECOVERY_CREATED" ;;
  'pr comment')
    while [ "$#" -gt 0 ]; do
      if [ "$1" = --body ]; then printf '%s' "$2" > "$ALUCARD_GITHUB_RECOVERY_MARKER"; break; fi
      shift
    done ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let environment = root.join("agent.env");
    fs::write(
        &environment,
        "ALUCARD_PROVIDER=claude\nANTHROPIC_API_KEY=test\n",
    )
    .unwrap();
    let database = root.join("ledger.db");
    let logs = root.join("logs");
    let arguments = [
        "--db-path",
        database.to_str().unwrap(),
        "run",
        "--github",
        "--iterations",
        "1",
        "--env-file",
        environment.to_str().unwrap(),
        "--logs-root",
        logs.to_str().unwrap(),
        "--no-build",
        repository.to_str().unwrap(),
    ];
    let invoke = || {
        Command::new(env!("CARGO_BIN_EXE_alucard"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("ALUCARD_GITHUB_RECOVERY_SCENARIO", scenario)
            .env(
                "ALUCARD_GITHUB_RECOVERY_DOCKER_TRACE",
                root.join("docker-trace"),
            )
            .env("ALUCARD_GITHUB_RECOVERY_GH_TRACE", root.join("gh-trace"))
            .env("ALUCARD_GITHUB_RECOVERY_CREATED", root.join("created"))
            .env("ALUCARD_GITHUB_RECOVERY_BODY", root.join("body"))
            .env("ALUCARD_GITHUB_RECOVERY_TITLE", root.join("title"))
            .env("ALUCARD_GITHUB_RECOVERY_MARKER", root.join("marker"))
            .env("ALUCARD_CACHE_DIR", root.join("cache"))
            .args(arguments)
            .output()
            .unwrap()
    };
    let output = invoke();
    assert!(output.status.success(), "{scenario}: {output:?}");
    let connection = open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT status FROM tasks", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "in_flight",
        "{scenario}"
    );
    assert_eq!(
        connection
            .query_row("SELECT pr_number FROM tasks", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        91,
        "{scenario}"
    );
    drop(connection);
    let output = invoke();
    assert!(output.status.success(), "{scenario}: {output:?}");
    assert_eq!(
        fs::read_to_string(root.join("docker-trace")).unwrap().len(),
        1,
        "{scenario}"
    );
}

#[test]
fn test_worker_recovery() {
    let root = temporary_path("github-worker-recovery");
    fs::create_dir(&root).unwrap();
    run_github_recovery_case(&root, "worker");
    assert!(fs::read_to_string(root.join("body"))
        .unwrap()
        .starts_with("Refs #134\n\n"));
    assert_eq!(
        fs::read_to_string(root.join("marker")).unwrap(),
        "**🤖 Alucard recovery PR parked** — opened as a draft and labeled `needs-human` by the harness. When the review gate approves this PR it marks it ready, removes the label, turns the `Refs` line into `Closes`, and replaces the stub title with the ticket's."
    );
    let trace = fs::read_to_string(root.join("gh-trace")).unwrap();
    assert!(
        trace.contains("pr edit 91 --add-label needs-human"),
        "{trace}"
    );
    assert!(
        trace.contains("issue edit 134 --remove-label in-progress"),
        "{trace}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_timeout_recovery() {
    let root = temporary_path("github-timeout-recovery");
    fs::create_dir(&root).unwrap();
    run_github_recovery_case(&root, "timeout");
    assert_eq!(
        fs::read_to_string(root.join("title")).unwrap(),
        "wip: alucard recovery — iter 1 (worker stopped: timeout, rc=124)"
    );
    let commits = Command::new("git")
        .args([
            "--git-dir",
            root.join("remote.git").to_str().unwrap(),
            "log",
            "--all",
            "--format=%s",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8(commits.stdout)
        .unwrap()
        .contains("wip: alucard recovery — uncommitted worker state"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_turn_budget() {
    let root = temporary_path("github-turn-budget-recovery");
    fs::create_dir(&root).unwrap();
    run_github_recovery_case(&root, "turn_budget");
    assert_eq!(
        fs::read_to_string(root.join("title")).unwrap(),
        "wip: alucard recovery — iter 1 (worker stopped: exhausted, rc=1)"
    );
    assert!(fs::read_to_string(root.join("body"))
        .unwrap()
        .contains("ran out of turns or hit its time limit"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn parser_preserves_task_structure_and_queue_eligibility() {
    let contents = fs::read_to_string("test/fixtures/tasks-basic.md").unwrap();
    let parsed = parse_tasks(&contents).unwrap();

    assert!(parsed
        .parent_context
        .contains("Shared context for the parser tests"));
    assert_eq!(
        parsed
            .tasks
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3", "4", "5", "6", "7"]
    );
    assert_eq!(
        parsed
            .tasks
            .iter()
            .map(|task| task.state)
            .collect::<Vec<_>>(),
        [
            TaskState::Pending,
            TaskState::Done,
            TaskState::Pending,
            TaskState::Pending,
            TaskState::InFlight,
            TaskState::Human,
            TaskState::Pending
        ]
    );
    assert_eq!(parsed.tasks[4].title, "In flight");
    assert_eq!(parsed.tasks[4].pr_number, Some(123));
    assert!(parsed.tasks[0]
        .body
        .contains("What to build: the first eligible thing."));
    assert_eq!(parsed.tasks[0].line, 6);
    assert_eq!(
        local_queue(&parsed.tasks)
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "4"]
    );
}

#[test]
fn parser_rejects_malformed_headings() {
    let contents = fs::read_to_string("test/fixtures/tasks-malformed.md").unwrap();
    let errors = parse_tasks(&contents).unwrap_err().join("\n");

    assert!(errors.contains("## [?] 3: Bad state token"));
    assert!(errors.contains("## [ ] 4 missing the colon separator"));
}

#[test]
fn parser_rejects_whitespace_prefixed_malformed_headings() {
    let errors = parse_tasks("## [ ] ready: Ready\n\n##\t[?] invalid\n\n##  [ ] id: Invalid\n")
        .unwrap_err()
        .join("\n");

    assert!(errors.contains("##\t[?] invalid"));
    assert!(errors.contains("##  [ ] id: Invalid"));
}

#[test]
fn parser_only_strips_unsigned_pr_suffixes() {
    let parsed = parse_tasks(
        "## [ ] previous: Retried (previous attempt: PR #7)\n\n## [ ] signed: Signed (PR #-7)\n\n## [ ] plus: Plus (previous attempt: PR #+7)\n",
    )
    .unwrap();

    assert_eq!(parsed.tasks[0].title, "Retried");
    assert_eq!(parsed.tasks[0].previous_pr_number, Some(7));
    assert_eq!(parsed.tasks[1].title, "Signed (PR #-7)");
    assert_eq!(parsed.tasks[1].pr_number, None);
    assert_eq!(parsed.tasks[2].title, "Plus (previous attempt: PR #+7)");
    assert_eq!(parsed.tasks[2].previous_pr_number, None);
}

#[test]
fn issue_blockers_use_the_open_issue_set() {
    let parsed =
        parse_tasks(&fs::read_to_string("test/fixtures/tasks-issue-blockers.md").unwrap()).unwrap();
    assert_eq!(
        local_queue_with_open_issues(&parsed.tasks, Some(&[100]))
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["2", "3", "4"]
    );
    assert_eq!(
        local_queue(&parsed.tasks)
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["4"]
    );
}

#[test]
fn inline_issue_blockers_use_the_open_issue_set() {
    let parsed = parse_tasks("## [ ] 1: Blocked\n\nThis task is Blocked by #100.\n").unwrap();

    assert_eq!(parsed.tasks[0].issue_blockers, [100]);
    assert!(local_queue_with_open_issues(&parsed.tasks, Some(&[100])).is_empty());
}

#[test]
fn parser_ignores_non_numeric_inline_issue_blockers() {
    let parsed = parse_tasks("## [ ] 1: Ready\n\nBlocked by #someone and #.\n").unwrap();

    assert!(parsed.tasks[0].issue_blockers.is_empty());
}

#[test]
fn parser_normalizes_crlf_task_content() {
    let parsed =
        parse_tasks("# Context\r\n\r\n## [ ] 1: Ready\r\n\r\nBlocked by: none\r\n").unwrap();

    assert_eq!(parsed.tasks[0].title, "Ready");
    assert_eq!(parsed.tasks[0].body, "Blocked by: none");
    assert_eq!(
        local_queue(&parsed.tasks)
            .iter()
            .map(|task| task.id.as_str())
            .collect::<Vec<_>>(),
        ["1"]
    );
}

#[test]
fn queue_cli_matches_local_contract_and_reimport_is_unchanged() {
    let database = temporary_path("queue").join("ledger.db");
    let gh_root = temporary_path("queue-gh");
    fs::create_dir_all(&gh_root).unwrap();
    let gh = gh_root.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncase \"$1 $2\" in\n  'pr list') printf '%s\\n' '[]' ;;\n  *) echo 'gh stub failed' >&2; exit 1 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_alucard");
    let contract = PathBuf::from("test/fixtures/contracts/local-tasks.md")
        .canonicalize()
        .unwrap();
    let expected = fs::read_to_string("test/fixtures/contracts/local-queue.json").unwrap();

    let output = Command::new(binary)
        .env("PATH", &gh_root)
        .args([
            "--db-path",
            database.to_str().unwrap(),
            "queue",
            "--tasks",
            contract.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::from_str::<serde_json::Value>(&expected).unwrap()
    );

    let parsed = parse_tasks(&fs::read_to_string(&contract).unwrap()).unwrap();
    migrate(&database).unwrap();
    let mut connection = open(&database).unwrap();
    let repository = temporary_path("repository").display().to_string();
    import_local_tasks(&mut connection, &repository, &contract, &parsed.tasks).unwrap();
    let changed_before = connection.total_changes();
    import_local_tasks(&mut connection, &repository, &contract, &parsed.tasks).unwrap();
    assert_eq!(connection.total_changes(), changed_before);
    drop(connection);
    fs::remove_dir_all(database.parent().unwrap()).unwrap();
    fs::remove_dir_all(gh_root).unwrap();
}

#[test]
fn reimport_preserves_claimed_task_status() {
    let database = temporary_path("claimed-reimport").join("ledger.db");
    migrate(&database).unwrap();
    let mut connection = open(&database).unwrap();
    let repository = "repo";
    let path = PathBuf::from("tasks.md");
    let tasks = parse_tasks("## [ ] task: Local task\n").unwrap().tasks;
    let issues = vec![serde_json::from_str::<GithubTask>(
        r#"{"number":42,"title":"GitHub task","body":"","labels":[]}"#,
    )
    .unwrap()];

    import_local_tasks(&mut connection, repository, &path, &tasks).unwrap();
    import_github_tasks(&mut connection, repository, &issues).unwrap();
    connection
        .execute("UPDATE tasks SET status = 'claimed'", [])
        .unwrap();
    import_local_tasks(&mut connection, repository, &path, &tasks).unwrap();
    import_github_tasks(&mut connection, repository, &issues).unwrap();

    let statuses: Vec<String> = connection
        .prepare("SELECT status FROM tasks ORDER BY source")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(statuses, ["claimed", "claimed"]);
    drop(connection);
    fs::remove_dir_all(database.parent().unwrap()).unwrap();
}

#[test]
fn queue_resolves_explicit_environment_and_detected_task_sources() {
    let root = temporary_path("task-source");
    let task_directory = root.join(".alucard");
    fs::create_dir_all(&task_directory).unwrap();
    let detected = task_directory.join("tasks.md");
    fs::copy("test/fixtures/contracts/local-tasks.md", &detected).unwrap();
    let binary = env!("CARGO_BIN_EXE_alucard");

    let detected_run = Command::new(binary)
        .args([
            "--db-path",
            root.join("detected.db").to_str().unwrap(),
            "queue",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(detected_run.status.success());

    let environment_run = Command::new(binary)
        .env("ALUCARD_TASKS_FILE", &detected)
        .args([
            "--db-path",
            root.join("environment.db").to_str().unwrap(),
            "queue",
        ])
        .output()
        .unwrap();
    assert!(environment_run.status.success());

    let no_local_source = root.join("no-local-source");
    fs::create_dir_all(&no_local_source).unwrap();
    let github_default = Command::new(binary)
        .env_remove("ALUCARD_TASKS_FILE")
        .args(["queue", no_local_source.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!github_default.status.success());

    let github_overrides_environment = Command::new(binary)
        .env("ALUCARD_TASKS_FILE", &detected)
        .args(["queue", "--github"])
        .output()
        .unwrap();
    assert!(!github_overrides_environment.status.success());

    let conflict = Command::new(binary)
        .args(["queue", "--tasks", detected.to_str().unwrap(), "--github"])
        .output()
        .unwrap();
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("--tasks and --github"));
}

#[test]
fn queue_does_not_fetch_issues_for_locally_blocked_tasks() {
    let root = temporary_path("locally-blocked-queue");
    fs::create_dir_all(&root).unwrap();
    let tasks = root.join("tasks.md");
    fs::write(
        &tasks,
        "## [>] waiting: Waiting\n\n## [ ] blocked: Blocked\n\nBlocked by: waiting, #100\n",
    )
    .unwrap();
    let gh = root.join("gh");
    let invoked = root.join("gh-invoked");
    fs::write(
        &gh,
        format!("#!/bin/sh\n: > '{}'\nexit 1\n", invoked.display()),
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_alucard");

    let output = Command::new(binary)
        .env("PATH", &root)
        .args([
            "--db-path",
            root.join("ledger.db").to_str().unwrap(),
            "queue",
            "--tasks",
            tasks.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::from_str::<serde_json::Value>("[]").unwrap()
    );
    assert!(!invoked.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn queue_excludes_reset_task_with_matching_open_pr() {
    let root = temporary_path("reset-task-open-pr");
    fs::create_dir_all(&root).unwrap();
    let tasks = root.join("tasks.md");
    fs::write(&tasks, "## [ ] task: Reset task\n").unwrap();
    let gh = root.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncase \"$1 $2\" in\n  'pr list') printf '%s\\n' '[{\"number\":12,\"title\":\"Open work\",\"body\":\"Task: task\"}]' ;;\n  *) exit 1 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_alucard");

    let output = Command::new(binary)
        .env("PATH", &root)
        .args([
            "--db-path",
            root.join("ledger.db").to_str().unwrap(),
            "queue",
            "--tasks",
            tasks.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap(),
        serde_json::json!([])
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn github_queue_matches_contract_and_orders_high_priority_first() {
    let mut issues: Vec<GithubTask> = serde_json::from_str(
        &fs::read_to_string("test/fixtures/contracts/github-issues.json").unwrap(),
    )
    .unwrap();
    issues.push(serde_json::from_str(r#"{"number":9,"title":"High","body":"","labels":[{"name":"ready-for-agent"},{"name":"priority:high"}]}"#).unwrap());
    issues.push(serde_json::from_str(r#"{"number":8,"title":"Human","body":"","labels":[{"name":"ready-for-agent"},{"name":"ready-for-human"}]}"#).unwrap());
    let open_prs: Vec<GithubPullRequest> = serde_json::from_str(
        &fs::read_to_string("test/fixtures/contracts/github-open-prs.json").unwrap(),
    )
    .unwrap();

    assert_eq!(github_ticket_blockers("## Blocked by\n\n- #12\n"), [12]);
    assert_eq!(
        github_queue(issues, &[99], &open_prs)
            .into_iter()
            .map(|issue| issue.number)
            .collect::<Vec<_>>(),
        [9, 10]
    );
}

#[test]
fn github_queue_cli_reports_gh_failures_and_rejects_pins() {
    let root = temporary_path("github-queue");
    fs::create_dir_all(&root).unwrap();
    let gh = root.join("gh");
    fs::write(
        &gh,
        r##"#!/bin/sh
if [ "${ALUCARD_GH_FAIL:-}" = 1 ]; then echo 'gh stub failed' >&2; exit 7; fi
case "$1 $2" in
  "issue list") printf '%s\n' '[{"number":10,"title":"Eligible","body":"","labels":[{"name":"ready-for-agent"}]}]' ;;
  "repo view") printf '%s\n' 'owner/repo' ;;
  "api graphql") printf '%s\n' "${ALUCARD_OPEN_PRS:-[]}" ;;
  "issue view") printf '%s\n' "${ALUCARD_ISSUE_JSON}" ;;
  "pr view") exit "${ALUCARD_PR_VIEW_EXIT:-1}" ;;
  *) echo 'gh stub failed' >&2; exit 7 ;;
esac
"##,
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let binary = env!("CARGO_BIN_EXE_alucard");

    let queued = Command::new(binary)
        .env("PATH", &root)
        .args([
            "--db-path",
            root.join("queue.db").to_str().unwrap(),
            "queue",
            "--github",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(queued.status.success(), "{:?}", queued);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&queued.stdout).unwrap(),
        serde_json::json!([{"number":10,"title":"Eligible","body":"","labels":[{"name":"ready-for-agent"}]}])
    );
    let connection = open(&root.join("queue.db")).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT external_id FROM tasks WHERE source = 'github' AND source_path = ''",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "10"
    );

    let failed = Command::new(binary)
        .env("PATH", &root)
        .env("ALUCARD_GH_FAIL", "1")
        .args(["queue", "--github", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(String::from_utf8_lossy(&failed.stderr).contains("gh stub failed"));

    let pinned = Command::new(binary)
        .env("PATH", &root)
        .args(["queue", "--issue", "42", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!pinned.status.success());
    assert!(
        String::from_utf8_lossy(&pinned.stderr).contains("--issue is only valid with alucard run")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_issue_resolution_rejects_ineligible_issues() {
    let issue = |body: &str| GithubTask {
        number: 42,
        title: "Pinned".to_string(),
        body: body.to_string(),
        labels: vec![],
        state: Some("OPEN".to_string()),
    };

    assert_eq!(
        resolve_pinned_issue(issue(""), true, &[], &[]),
        Err("issue #42 is a pull request; use alucard continue 42".to_string())
    );
    assert_eq!(
        resolve_pinned_issue(issue("Blocked by #10"), false, &[10], &[]),
        Err("issue #42 is blocked by #10".to_string())
    );
    let open_prs = vec![serde_json::from_str(
        r#"{"number":5,"title":"","body":"Refs #42","closingIssuesReferences":{"nodes":[]}}"#,
    )
    .unwrap()];
    assert_eq!(
        resolve_pinned_issue(issue(""), false, &[], &open_prs),
        Err("issue #42 is occupied by PR #5; use alucard continue 5".to_string())
    );
}

#[test]
fn run_dispatches_one_local_task_and_returns_it_to_pending_without_a_pr() {
    let root = temporary_path("run-local");
    let repository = root.join("repository");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "remote",
        "add",
        "origin",
        "git@github.com:example/repo.git",
    ]));
    let tasks = root.join("tasks.md");
    fs::write(
        &tasks,
        "Shared context\n\n## [ ] first: First task\n\nFirst body\n\n## [ ] second: Second task\n\nSecond body\n",
    )
    .unwrap();
    let prompts = root.join("prompts");
    fs::create_dir(&prompts).unwrap();
    let docker_args = root.join("docker-args");
    fs::create_dir(&docker_args).unwrap();
    let selection = root.join("selection");
    fs::create_dir(&selection).unwrap();
    let docker = bin.join("docker");
    fs::write(
        &docker,
        "#!/bin/sh\ncase \"$1 $2\" in\n  'image inspect') echo stamp ;;\n  'run '*)\n    printf '%s\\n' \"$@\" > \"$ALUCARD_DOCKER_ARGS/$$\"\n    cat > \"$ALUCARD_PROMPTS/$$\" ;;\n  'rm -f') ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncase \"$1 $2\" in\n  'issue list')\n    if mkdir \"$ALUCARD_SELECTION/lock\" 2>/dev/null; then : > \"$ALUCARD_SELECTION/first\"; fi\n    : > \"$ALUCARD_SELECTION/$$\"\n    while [ \"$(find \"$ALUCARD_SELECTION\" -maxdepth 1 -type f ! -name first | wc -l)\" -lt 2 ]; do sleep 0.01; done\n    echo '[]' ;;\n  'pr list') echo '[]' ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let environment = root.join("agent.env");
    fs::write(
        &environment,
        "ALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let database = root.join("ledger.db");
    let command = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_alucard"));
        command
            .env("PATH", &path)
            .env("ALUCARD_PROMPTS", &prompts)
            .env("ALUCARD_DOCKER_ARGS", &docker_args)
            .env("ALUCARD_SELECTION", &selection)
            .env("ALUCARD_CACHE_DIR", root.join("cache"))
            .args([
                "--db-path",
                database.to_str().unwrap(),
                "run",
                "--iterations",
                "1",
                "--tasks",
                tasks.to_str().unwrap(),
                "--env-file",
                environment.to_str().unwrap(),
                "--logs-root",
                root.join("logs").to_str().unwrap(),
                "--no-build",
                repository.to_str().unwrap(),
            ]);
        command
    };
    let mut first = command().spawn().unwrap();
    wait_for_file(&selection.join("first"));
    let mut second = command().spawn().unwrap();
    let first = first.wait().unwrap();
    assert!(first.success(), "{first:?}");
    let second = second.wait().unwrap();
    assert!(second.success(), "{second:?}");
    let prompts = fs::read_dir(prompts)
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(prompts.len(), 2);
    assert!(prompts.iter().any(|prompt| prompt.contains("First task")));
    assert!(prompts.iter().any(|prompt| prompt.contains("Second task")));
    let docker_arguments = fs::read_dir(docker_args)
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    assert!(docker_arguments
        .iter()
        .all(|arguments| arguments.contains("GH_REPO=example/repo")));
    for entry in fs::read_dir(root.join("logs")).unwrap() {
        let clone = entry.unwrap().path().join("repository");
        let origin = Command::new("git")
            .args(["remote", "get-url", "origin"])
            .current_dir(clone)
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&origin.stdout).trim(),
            "https://github.com/example/repo.git"
        );
    }
    let connection = open(&database).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM claims WHERE state = 'active'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE status = 'pending'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_run_removes_the_label_without_a_pr_and_skips_a_second_dispatch() {
    let root = temporary_path("run-pinned");
    let repository = root.join("repository");
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir(&repository).unwrap();
    git_repository(&repository);
    let docker_trace = root.join("docker-trace");
    let gh_trace = root.join("gh-trace");
    let docker = bin.join("docker");
    fs::write(
        &docker,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ALUCARD_DOCKER_TRACE\"\ncase \"$1 $2\" in\n  'image inspect') echo stamp ;;\n  'run '*)\n    if mkdir \"$ALUCARD_DOCKER_HOLD_LOCK\" 2>/dev/null; then\n      : > \"$ALUCARD_DOCKER_STARTED\"\n      while [ ! -f \"$ALUCARD_DOCKER_RELEASE\" ]; do sleep 0.01; done\n    fi ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ALUCARD_GH_TRACE\"\ncase \"$1 $2\" in\n  'issue view') echo '{\"number\":42,\"title\":\"Eligible\",\"body\":\"\",\"labels\":[{\"name\":\"ready-for-agent\"}],\"state\":\"OPEN\"}' ;;\n  'issue list') echo '[]' ;;\n  'repo view') echo 'owner/repo' ;;\n  'api graphql')\n    case \"$*\" in\n      *issueOrPullRequest*)\n        if [ -f \"$ALUCARD_TYPE_LOOKUP_FAIL\" ]; then echo 'lookup failed' >&2; exit 1; elif [ -f \"$ALUCARD_PINNED_PR\" ]; then echo PullRequest; else echo Issue; fi ;;\n      *) if [ -f \"$ALUCARD_RETURN_PR\" ]; then echo '[{\"number\":77,\"title\":\"\",\"body\":\"\",\"headRefName\":\"agent/task\",\"headRefOid\":\"abc\"}]'; else echo '[]'; fi ;;\n    esac ;;\n  'pr list') if [ -f \"$ALUCARD_RETURN_PR\" ]; then echo '[{\"number\":77,\"title\":\"\",\"body\":\"\",\"headRefName\":\"agent/task\",\"headRefOid\":\"abc\"}]'; else echo '[]'; fi ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let environment = root.join("agent.env");
    fs::write(
        &environment,
        "ALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
    )
    .unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let database = root.join("ledger.db");
    let logs = root.join("logs");
    let return_pr = root.join("return-pr");
    let pinned_pr = root.join("pinned-pr");
    let type_lookup_fail = root.join("type-lookup-fail");
    let docker_started = root.join("docker-started");
    let docker_release = root.join("docker-release");
    let docker_hold_lock = root.join("docker-hold-lock");
    let arguments = [
        "--db-path",
        database.to_str().unwrap(),
        "run",
        "--github",
        "--issue",
        "42",
        "--env-file",
        environment.to_str().unwrap(),
        "--logs-root",
        logs.to_str().unwrap(),
        "--no-build",
        repository.to_str().unwrap(),
    ];
    let run = || {
        let mut command = Command::new(env!("CARGO_BIN_EXE_alucard"));
        command
            .env("PATH", &path)
            .env("ALUCARD_DOCKER_TRACE", &docker_trace)
            .env("ALUCARD_GH_TRACE", &gh_trace)
            .env("ALUCARD_RETURN_PR", &return_pr)
            .env("ALUCARD_PINNED_PR", &pinned_pr)
            .env("ALUCARD_TYPE_LOOKUP_FAIL", &type_lookup_fail)
            .env("ALUCARD_DOCKER_STARTED", &docker_started)
            .env("ALUCARD_DOCKER_RELEASE", &docker_release)
            .env("ALUCARD_DOCKER_HOLD_LOCK", &docker_hold_lock)
            .env("ALUCARD_CACHE_DIR", root.join("cache"))
            .args(arguments);
        command
    };
    let mut first = run().spawn().unwrap();
    wait_for_file(&docker_started);
    let second = run().output().unwrap();
    assert!(second.status.success(), "{second:?}");
    fs::write(&docker_release, "").unwrap();
    let first = first.wait().unwrap();
    assert!(first.success(), "{first:?}");
    let connection = open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT status FROM tasks", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "pending"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM claims WHERE state = 'active'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(connection);
    fs::write(&return_pr, "").unwrap();
    let output = run().output().unwrap();
    assert!(output.status.success(), "{output:?}");
    let output = run().output().unwrap();
    assert!(output.status.success(), "{output:?}");
    fs::write(&pinned_pr, "").unwrap();
    let output = run().output().unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("issue #42 is a pull request; use alucard continue 42"));
    let docker_trace_contents = fs::read_to_string(&docker_trace).unwrap();
    assert_eq!(
        docker_trace_contents
            .lines()
            .filter(|line| line.starts_with("run "))
            .count(),
        2
    );
    let gh_trace_contents = fs::read_to_string(&gh_trace).unwrap();
    assert!(gh_trace_contents.contains("issue view 42 --json number,title,body,labels,state"));
    assert!(gh_trace_contents.contains("issueOrPullRequest"));
    assert!(gh_trace_contents.contains("issue edit 42 --add-label in-progress"));
    assert!(gh_trace_contents.contains("issue edit 42 --remove-label in-progress"));
    fs::remove_file(&pinned_pr).unwrap();
    fs::write(&type_lookup_fail, "").unwrap();
    let output = run().output().unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("could not check whether GitHub issue is a pull request: lookup failed"));
    let docker_trace_contents = fs::read_to_string(&docker_trace).unwrap();
    assert_eq!(
        docker_trace_contents
            .lines()
            .filter(|line| line.starts_with("run "))
            .count(),
        2
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_transport_pr_retry() {
    let retry_root = temporary_path("transport-retry");
    let retry = run_transport_case(&retry_root, "transport", 1);
    assert!(retry.status.success(), "{retry:?}");
    let attempts = fs::read_to_string(retry_root.join("trace")).unwrap();
    let attempts = attempts.lines().collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert_ne!(attempts[0], attempts[1]);
    assert_ne!(
        attempts[0].split_once('|').unwrap().0,
        attempts[1].split_once('|').unwrap().0
    );
    assert_ne!(
        attempts[0].split_once('|').unwrap().1,
        attempts[1].split_once('|').unwrap().1
    );
    fs::remove_dir_all(&retry_root).unwrap();

    let pr_root = temporary_path("transport-pr");
    let pr = run_transport_case(&pr_root, "pr", 1);
    assert!(pr.status.success(), "{pr:?}");
    assert_eq!(
        fs::read_to_string(pr_root.join("trace"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let connection = open(&pr_root.join("ledger.db")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pull_requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row("SELECT number FROM pull_requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        59
    );
    drop(connection);
    fs::remove_dir_all(pr_root).unwrap();
}

#[test]
fn test_pr_binding() {
    let root = temporary_path("foreign-pr");
    let output = run_transport_case(&root, "foreign", 0);
    assert!(output.status.success(), "{output:?}");
    let connection = open(&root.join("ledger.db")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT status FROM tasks", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "pending"
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pull_requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn test_transport_recovery() {
    let billing_root = temporary_path("transport-billing");
    let billing = run_transport_case(&billing_root, "billing", 2);
    assert!(!billing.status.success(), "{billing:?}");
    assert_eq!(
        fs::read_to_string(billing_root.join("trace"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    fs::remove_dir_all(&billing_root).unwrap();

    let clean_root = temporary_path("transport-clean");
    let clean = run_transport_case(&clean_root, "transport", 0);
    assert!(clean.status.success(), "{clean:?}");
    let attempt = fs::read_to_string(clean_root.join("trace")).unwrap();
    let (_, clone) = attempt.trim().split_once('|').unwrap();
    assert!(Path::new(clone).join(".git").is_dir());
    let connection = open(&clean_root.join("ledger.db")).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pull_requests", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(connection);
    assert!(!fs::read_to_string(clean_root.join("gh-trace"))
        .unwrap_or_default()
        .contains("pr create"));
    fs::remove_dir_all(clean_root).unwrap();
}
