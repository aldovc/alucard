use alucard::{
    github_queue, github_ticket_blockers, import_github_tasks, import_local_tasks, local_queue,
    local_queue_with_open_issues, migrate, open, parse_tasks, resolve_pinned_issue,
    GithubPullRequest, GithubTask, TaskState,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("alucard-{name}-{}-{unique}", std::process::id()))
}

fn run(command: &mut Command) {
    assert!(command.status().unwrap().success());
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
    let binary = env!("CARGO_BIN_EXE_alucard");
    let contract = PathBuf::from("test/fixtures/contracts/local-tasks.md")
        .canonicalize()
        .unwrap();
    let expected = fs::read_to_string("test/fixtures/contracts/local-queue.json").unwrap();

    let output = Command::new(binary)
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
    let tasks = root.join("tasks.md");
    fs::write(
        &tasks,
        "Shared context\n\n## [ ] first: First task\n\nFirst body\n\n## [ ] second: Second task\n\nSecond body\n",
    )
    .unwrap();
    let prompt = root.join("prompt");
    let docker = bin.join("docker");
    fs::write(
        &docker,
        "#!/bin/sh\ncase \"$1 $2\" in\n  'image inspect') echo stamp ;;\n  'run '*) cat > \"$ALUCARD_PROMPT\" ;;\n  'rm -f') ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\ncase \"$1 $2\" in 'pr list') echo '[]' ;; esac\n",
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

    let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .env("PATH", path)
        .env("ALUCARD_PROMPT", &prompt)
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
    assert!(output.status.success(), "{output:?}");
    let prompt = fs::read_to_string(prompt).unwrap();
    assert!(prompt.contains("First task"));
    assert!(!prompt.contains("Second task"));
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
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ALUCARD_DOCKER_TRACE\"\ncase \"$1 $2\" in 'image inspect') echo stamp ;; esac\n",
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ALUCARD_GH_TRACE\"\ncase \"$1 $2\" in\n  'issue view') echo '{\"number\":42,\"title\":\"Eligible\",\"body\":\"\",\"labels\":[{\"name\":\"ready-for-agent\"}],\"state\":\"OPEN\",\"isPullRequest\":false}' ;;\n  'issue list') echo '[]' ;;\n  'repo view') echo 'owner/repo' ;;\n  'pr list'|'api graphql') if [ -f \"$ALUCARD_RETURN_PR\" ]; then echo '[{\"number\":77,\"title\":\"\",\"body\":\"\",\"headRefName\":\"agent/task\",\"headRefOid\":\"abc\"}]'; else echo '[]'; fi ;;\nesac\n",
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
        Command::new(env!("CARGO_BIN_EXE_alucard"))
            .env("PATH", &path)
            .env("ALUCARD_DOCKER_TRACE", &docker_trace)
            .env("ALUCARD_GH_TRACE", &gh_trace)
            .env("ALUCARD_RETURN_PR", &return_pr)
            .env("ALUCARD_CACHE_DIR", root.join("cache"))
            .args(arguments)
            .output()
            .unwrap()
    };
    let output = run();
    assert!(output.status.success(), "{output:?}");
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
    let output = run();
    assert!(output.status.success(), "{output:?}");
    let output = run();
    assert!(output.status.success(), "{output:?}");
    let docker_trace = fs::read_to_string(docker_trace).unwrap();
    assert_eq!(
        docker_trace
            .lines()
            .filter(|line| line.starts_with("run "))
            .count(),
        2
    );
    let gh_trace = fs::read_to_string(gh_trace).unwrap();
    assert!(gh_trace.contains("issue edit 42 --add-label in-progress"));
    assert!(gh_trace.contains("issue edit 42 --remove-label in-progress"));
    fs::remove_dir_all(root).unwrap();
}
