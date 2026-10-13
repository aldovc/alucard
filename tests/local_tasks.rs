use alucard::{
    import_local_tasks, local_queue, local_queue_with_open_issues, migrate, open, parse_tasks,
    TaskState,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("alucard-{name}-{}-{unique}", std::process::id()))
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
