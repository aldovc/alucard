mod test_support;

use alucard::{
    abandoned_task_claims, acquire_pr, acquire_task, begin_operation, begin_operation_with_payload,
    defer_operation, ensure_repository_identity, finish_operation, heartbeat, import_github_tasks,
    import_local_tasks, lock_recovery_operations, migrate, open, parse_tasks, pending_operations,
    pending_recovery_operations, prepare_run, record_task_pr, record_task_pr_gate_state,
    record_task_pr_review, release, release_abandoned_task_claim, repository_id,
    repository_ledger_id, repository_source_path, run_command, start_continue_run, start_merge_run,
    start_retry_run, start_task_retry_run, start_task_run, status, task_claim_attempts, task_id,
    transition_task, AbandonedTaskClaim, ClaimError, GithubTask, RunOptions, TaskStatus,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn database_path() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir()
        .join(format!("alucard-test-{}-{unique}", std::process::id()))
        .join("ledger.db")
}

fn prepare_claims(path: &Path, task_ids: &[&str]) {
    migrate(path).unwrap();
    let connection = open(path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES ('repo', 'owner', 'name', 'now')",
            [],
        )
        .unwrap();
    for task_id in task_ids {
        connection
            .execute(
                "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
                 VALUES (?1, 'repo', 'github', ?1, 'task', 'pending', '[]', 'now')",
                [task_id],
            )
            .unwrap();
    }
}

fn insert_run(path: &Path, run_id: &str) {
    open(path)
        .unwrap()
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES (?1, 'repo', 'run', 'running', 'now', 'now')",
            [run_id],
        )
        .unwrap();
}

#[test]
fn continue_run_adopts_an_unrecorded_recovery_pull_request() {
    let directory = database_path().parent().unwrap().to_path_buf();
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("ledger.db");
    let repository = directory.join("repository");
    let logs = directory.join("logs");
    fs::create_dir(&repository).unwrap();

    let (continued, _) = start_continue_run(
        &path,
        &repository,
        &logs,
        "image:test",
        42,
        "alucard/recovery",
        "head",
        "recovery title",
        true,
    )
    .unwrap();
    let connection = open(&path).unwrap();
    assert_eq!(continued.run.command, "continue");
    assert_eq!(
        connection
            .query_row(
                "SELECT kind FROM pull_requests WHERE number = 42",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "recovery"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM tasks WHERE id = ?1",
                [&continued.task_id],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "in_flight"
    );
    fs::remove_dir_all(directory).unwrap();
}

fn seed_repository(path: &Path) {
    seed_repository_with_readme(path, "seed\n");
}

fn set_origin(path: &Path, origin: &str) {
    assert!(Command::new("git")
        .args(["remote", "add", "origin", origin])
        .current_dir(path)
        .status()
        .unwrap()
        .success());
}

#[test]
fn repository_identity_shares_leases_across_clones_and_keeps_tasks_distinct() {
    let path = database_path();
    let root = path.parent().unwrap();
    let first = root.join("first");
    let second = root.join("second");
    let other = root.join("other");
    for repository in [&first, &second, &other] {
        seed_repository(repository);
    }
    set_origin(&first, "https://github.com/example/widgets.git/");
    set_origin(&second, "git@github.com:Example/Widgets.git");
    set_origin(&other, "https://github.com/example/other.git");
    migrate(&path).unwrap();
    let issues: Vec<GithubTask> = serde_json::from_str(
        r#"[{"number":42,"title":"Shared","body":"","labels":[]},{"number":43,"title":"Other task","body":"","labels":[]}]"#,
    )
    .unwrap();
    let mut connection = open(&path).unwrap();
    import_github_tasks(&mut connection, &repository_id(&first), &issues).unwrap();
    import_github_tasks(&mut connection, &repository_id(&other), &issues[..1]).unwrap();
    let shared = task_id(&path, &repository_id(&second), "github", "", "42")
        .unwrap()
        .unwrap();
    let distinct = task_id(&path, &repository_id(&second), "github", "", "43")
        .unwrap()
        .unwrap();
    let other_task = task_id(&path, &repository_id(&other), "github", "", "42")
        .unwrap()
        .unwrap();
    let logs = root.join("logs");
    let cache = root.join("cache");
    let (first_run, _) = start_task_run(&path, &first, &logs, &cache, None, &shared).unwrap();
    assert_eq!(first_run.repository_id, repository_id(&second));
    assert!(matches!(
        start_task_run(&path, &second, &logs, &cache, None, &shared),
        Err(ClaimError::ClaimedBy { .. })
    ));
    start_task_run(&path, &second, &logs, &cache, None, &distinct).unwrap();
    start_task_run(&path, &other, &logs, &cache, None, &other_task).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_identity_shares_continue_merge_and_relative_local_tasks() {
    let path = database_path();
    let root = path.parent().unwrap();
    let first = root.join("first");
    let second = root.join("second");
    for repository in [&first, &second] {
        seed_repository(repository);
    }
    set_origin(&first, "https://github.com/example/widgets.git");
    set_origin(&second, "git@github.com:example/widgets.git");
    migrate(&path).unwrap();
    let task_contents = "## [ ] 1: Local task\n";
    let first_tasks = first.join("tasks.md");
    let second_tasks = second.join("tasks.md");
    fs::write(&first_tasks, task_contents).unwrap();
    fs::write(&second_tasks, task_contents).unwrap();
    let tasks = parse_tasks(task_contents).unwrap().tasks;
    let first_source_path = repository_source_path(&first, &first_tasks).unwrap();
    let second_source_path = repository_source_path(&second, &second_tasks).unwrap();
    assert_eq!(first_source_path, "tasks.md");
    assert_eq!(first_source_path, second_source_path);
    let mut connection = open(&path).unwrap();
    import_local_tasks(
        &mut connection,
        &repository_id(&first),
        Path::new(&first_source_path),
        &tasks,
    )
    .unwrap();
    let local = task_id(
        &path,
        &repository_id(&second),
        "local",
        &second_source_path,
        "1",
    )
    .unwrap();
    assert!(local.is_some());
    let logs = root.join("logs");
    let merged = start_merge_run(&path, &first, &logs).unwrap();
    acquire_pr(&path, &merged.repository_id, 9, &merged.id).unwrap();
    let competing_merge = start_merge_run(&path, &second, &logs).unwrap();
    assert!(matches!(
        acquire_pr(
            &path,
            &competing_merge.repository_id,
            9,
            &competing_merge.id
        ),
        Err(ClaimError::ClaimedBy { .. })
    ));
    let (continued, claim) = start_continue_run(
        &path, &first, &logs, "image", 10, "branch", "sha", "title", false,
    )
    .unwrap();
    assert!(matches!(
        start_continue_run(&path, &second, &logs, "image", 10, "branch", "sha", "title", false,),
        Err(ClaimError::ClaimedBy { .. })
    ));
    release(&path, &claim.id, &continued.run.id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_identity_migrates_legacy_rows_and_reports_live_conflicts() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("checkout");
    seed_repository(&repository);
    set_origin(&repository, "https://github.com/example/widgets.git");
    migrate(&path).unwrap();
    let legacy = "Example/Widgets";
    let stable = repository_id(&repository);
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, '', 'checkout', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('legacy-task', ?1, 'github', '42', 'legacy', 'in_flight', '[]', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES ('legacy-run', ?1, 'run', 'interrupted', 'now', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO recovery (id, task_id, original_run_id, branch, reason, created_at)
             VALUES ('recovery', 'legacy-task', 'legacy-run', 'branch', 'failed', 'now')",
            [],
        )
        .unwrap();
    drop(connection);
    assert_eq!(
        ensure_repository_identity(&path, &repository).unwrap(),
        stable
    );
    let connection = open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT repository_id FROM tasks WHERE id = 'legacy-task'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        stable
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM recovery WHERE task_id = 'legacy-task'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('conflict', ?1, 'github', '43', 'current', 'pending', '[]', 'now')",
            [&stable],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, '', 'checkout', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('legacy-conflict-task', ?1, 'github', '43', 'legacy', 'claimed', '[]', 'now')",
            [&legacy],
        )
        .unwrap();
    drop(connection);
    assert!(ensure_repository_identity(&path, &repository).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_identity_migrates_live_legacy_lease_from_equivalent_clone() {
    let path = database_path();
    let root = path.parent().unwrap();
    let first = root.join("first");
    let second = root.join("second");
    for repository in [&first, &second] {
        seed_repository(repository);
    }
    set_origin(&first, "https://github.com/example/widgets.git");
    set_origin(&second, "git@github.com:example/widgets.git");
    migrate(&path).unwrap();
    let legacy = first.canonicalize().unwrap().display().to_string();
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, '', 'first', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES ('legacy-run', ?1, 'merge', 'running', 'now', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('legacy-claim', ?1, 'pr', '42', 'legacy-run', 'active', 'now', 'now')",
            [&legacy],
        )
        .unwrap();
    drop(connection);

    let stable = ensure_repository_identity(&path, &second).unwrap();
    let competing = start_merge_run(&path, &second, &root.join("logs")).unwrap();
    assert!(matches!(
        acquire_pr(&path, &stable, 42, &competing.id),
        Err(ClaimError::ClaimedBy { .. })
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_identity_migrates_live_legacy_git_suffix_lease() {
    let path = database_path();
    let root = path.parent().unwrap();
    let first = root.join("first");
    let second = root.join("second");
    for repository in [&first, &second] {
        seed_repository(repository);
    }
    set_origin(&first, "https://github.com/example/widgets.git/");
    set_origin(&second, "git@github.com:example/widgets.git");
    migrate(&path).unwrap();
    let legacy = "example/widgets.git";
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, '', 'first', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES ('legacy-run', ?1, 'merge', 'running', 'now', 'now')",
            [&legacy],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('legacy-claim', ?1, 'pr', '42', 'legacy-run', 'active', 'now', 'now')",
            [&legacy],
        )
        .unwrap();
    drop(connection);

    let stable = ensure_repository_identity(&path, &second).unwrap();
    let competing = start_merge_run(&path, &second, &root.join("logs")).unwrap();
    assert!(matches!(
        acquire_pr(&path, &stable, 42, &competing.id),
        Err(ClaimError::ClaimedBy { .. })
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn repository_identity_discards_pending_legacy_local_task_matching_stable_task() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("checkout");
    seed_repository(&repository);
    set_origin(&repository, "https://github.com/example/widgets.git");
    migrate(&path).unwrap();
    let legacy = repository.canonicalize().unwrap().display().to_string();
    let stable = repository_id(&repository);
    let connection = open(&path).unwrap();
    for repository_id in [&legacy, &stable] {
        connection
            .execute(
                "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, '', 'checkout', 'now')",
                [repository_id],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, source_path, external_id, title, status, blockers_json, updated_at)
             VALUES ('legacy-task', ?1, 'local', ?2, '1', 'legacy', 'pending', '[]', 'now')",
            (&legacy, format!("{legacy}/tasks.md")),
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, source_path, external_id, title, status, blockers_json, updated_at)
             VALUES ('stable-task', ?1, 'local', 'tasks.md', '1', 'stable', 'pending', '[]', 'now')",
            [&stable],
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        ensure_repository_identity(&path, &repository).unwrap(),
        stable
    );
    let connection = open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE repository_id = ?1 AND source = 'local' AND source_path = 'tasks.md' AND external_id = '1'",
                [&stable],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    fs::remove_dir_all(root).unwrap();
}

fn seed_repository_with_readme(path: &Path, readme: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::create_dir(path).unwrap();
    Command::new("git")
        .args(["init", "-q"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "test@example.invalid"])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Test"])
        .current_dir(path)
        .status()
        .unwrap();
    fs::write(path.join("README.md"), readme).unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .status()
        .unwrap();
    Command::new("git")
        .args(["commit", "-qm", "seed"])
        .current_dir(path)
        .status()
        .unwrap();
}

#[cfg(unix)]
#[test]
fn timed_command_drains_output_before_termination() {
    let path = database_path();
    let root = path.parent().unwrap();
    fs::create_dir_all(root).unwrap();

    let output = run_command(
        "sh",
        &[
            "-c",
            "head -c 131072 /dev/zero; printf stderr >&2; while :; do :; done",
        ],
        root,
        Some(Duration::from_millis(200)),
    )
    .unwrap();

    assert!(!output.status.success());
    assert_eq!(output.stdout.len(), 131072);
    assert_eq!(output.stderr, b"stderr");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn retry_attempts_have_distinct_run_records_and_workspaces() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("source");
    seed_repository(&repository);
    let logs = root.join("logs");
    let cache = root.join("cache");
    migrate(&path).unwrap();

    let first = start_retry_run(&path, &repository, &logs, &cache, None).unwrap();
    let second =
        start_task_retry_run(&path, &repository, &logs, &cache, None, Some(&first.id)).unwrap();

    assert_ne!(first.id, second.id);
    assert_ne!(first.logs_path, second.logs_path);
    assert_eq!(
        open(&path)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM runs", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        2
    );
    let attempts = task_claim_attempts(
        &path,
        &AbandonedTaskClaim {
            lease_id: "claim".to_string(),
            run_id: first.id.clone(),
            task_id: "task".to_string(),
            repository_id: first.repository_id.clone(),
            branch: None,
        },
    )
    .unwrap();
    assert_eq!(
        attempts
            .iter()
            .map(|attempt| &attempt.run_id)
            .collect::<Vec<_>>(),
        vec![&first.id, &second.id]
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_runs_with_matching_repository_names_use_separate_caches() {
    let path = database_path();
    let root = path.parent().unwrap();
    let first_repository = root.join("team-a/api");
    let second_repository = root.join("team-b/api");
    seed_repository_with_readme(&first_repository, "team a\n");
    seed_repository_with_readme(&second_repository, "team b\n");
    let logs = root.join("logs");
    let cache = root.join("cache");

    let first = prepare_run(
        &path,
        RunOptions {
            repository: &first_repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();
    let second = prepare_run(
        &path,
        RunOptions {
            repository: &second_repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();

    assert_eq!(
        fs::read_to_string(first.clone.join("README.md")).unwrap(),
        "team a\n"
    );
    assert_eq!(
        fs::read_to_string(second.clone.join("README.md")).unwrap(),
        "team b\n"
    );
    let cache_paths = open(&path)
        .unwrap()
        .prepare("SELECT cache_path FROM repositories ORDER BY id")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(cache_paths.len(), 2);
    assert_ne!(cache_paths[0], cache_paths[1]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_runs_with_long_canonical_paths_use_a_bounded_cache_key() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root
        .join("a".repeat(40))
        .join("b".repeat(40))
        .join("c".repeat(40));
    seed_repository(&repository);
    let logs = root.join("logs");
    let cache = root.join("cache");

    let prepared = prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();

    assert!(prepared.clone.join(".git").is_dir());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_runs_have_isolated_clones_branches_and_directories() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("source");
    seed_repository(&repository);
    assert!(Command::new("git")
        .args(["branch", "release"])
        .current_dir(&repository)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .args([
            "-C",
            repository.to_str().unwrap(),
            "remote",
            "add",
            "origin",
            "git@github.com:example/source.git",
        ])
        .status()
        .unwrap()
        .success());
    let logs = root.join("logs");
    let cache = root.join("cache");

    let first = prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "release",
            image: None,
            task: Some("alpha"),
            issue: None,
            run_id: None,
        },
    )
    .unwrap();
    let second = prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "release",
            image: None,
            task: None,
            issue: Some(42),
            run_id: None,
        },
    )
    .unwrap();

    assert_ne!(first.directory, second.directory);
    assert_ne!(first.branch, second.branch);
    assert!(first.clone.join(".git").is_dir());
    assert!(second.clone.join(".git").is_dir());
    assert!(first.branch.ends_with("-task-alpha"));
    assert!(second.branch.ends_with("-issue-42"));
    assert_eq!(
        String::from_utf8(
            Command::new("git")
                .args([
                    "config",
                    "--get",
                    &format!("branch.{}.gh-merge-base", first.branch),
                ])
                .current_dir(&first.clone)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim(),
        "release"
    );
    assert_eq!(
        String::from_utf8(
            Command::new("git")
                .args(["remote", "get-url", "origin"])
                .current_dir(&first.clone)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim(),
        "https://github.com/example/source.git"
    );
    assert_eq!(
        open(&path)
            .unwrap()
            .query_row(
                "SELECT status FROM runs WHERE id = ?1",
                [&first.run.id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "running"
    );
    assert_eq!(
        open(&path)
            .unwrap()
            .query_row(
                "SELECT branch FROM runs WHERE id = ?1",
                [&first.run.id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        first.branch
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_run_uses_the_refreshed_remote_head() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("source");
    seed_repository(&repository);
    let logs = root.join("logs");
    let cache = root.join("cache");

    prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();
    fs::write(repository.join("README.md"), "updated\n").unwrap();
    Command::new("git")
        .args(["commit", "-am", "update"])
        .current_dir(&repository)
        .status()
        .unwrap();
    let updated_sha = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repository)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let refreshed = prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();

    assert_eq!(
        refreshed.run.base_sha.as_deref(),
        Some(updated_sha.as_str())
    );
    assert_eq!(
        fs::read_to_string(refreshed.clone.join("README.md")).unwrap(),
        "updated\n"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_preparation_keeps_working_clones_usable() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("source");
    seed_repository(&repository);
    migrate(&path).unwrap();
    let logs = root.join("logs");
    let cache = root.join("cache");
    let barrier = Arc::new(Barrier::new(2));
    let preparations = (0..2)
        .map(|_| {
            let path = path.clone();
            let repository = repository.clone();
            let logs = logs.clone();
            let cache = cache.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                prepare_run(
                    &path,
                    RunOptions {
                        repository: &repository,
                        logs_root: &logs,
                        cache_root: &cache,
                        base: "HEAD",
                        image: None,
                        task: None,
                        issue: None,
                        run_id: None,
                    },
                )
            })
        })
        .collect::<Vec<_>>();
    let runs = preparations
        .into_iter()
        .map(|preparation| preparation.join().unwrap().unwrap())
        .collect::<Vec<_>>();

    assert_ne!(runs[0].clone, runs[1].clone);
    assert_ne!(runs[0].branch, runs[1].branch);
    for run in runs {
        assert!(run.clone.join(".git").is_dir());
        assert!(Command::new("git")
            .args(["rev-parse", "--verify", &run.branch])
            .current_dir(run.clone)
            .status()
            .unwrap()
            .success());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn preparing_a_run_reconciles_abandoned_owners() {
    let path = database_path();
    let root = path.parent().unwrap();
    let repository = root.join("source");
    seed_repository(&repository);
    let logs = root.join("logs");
    let cache = root.join("cache");
    let first = prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();
    let connection = open(&path).unwrap();
    for (run_id, pid) in [
        ("stopped", i64::MAX),
        ("live", i64::from(std::process::id())),
    ] {
        connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, pid, hostname)
             VALUES (?1, ?2, 'run', 'running', 'now', 'now', ?3, ?4)",
            (
                run_id,
                &first.run.repository_id,
                pid,
                std::fs::read_to_string("/etc/hostname").unwrap().trim(),
            ),
        )
        .unwrap();
    }

    prepare_run(
        &path,
        RunOptions {
            repository: &repository,
            logs_root: &logs,
            cache_root: &cache,
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
            run_id: None,
        },
    )
    .unwrap();

    assert_eq!(
        open(&path)
            .unwrap()
            .query_row("SELECT status FROM runs WHERE id = 'stopped'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "interrupted"
    );
    assert_eq!(
        open(&path)
            .unwrap()
            .query_row("SELECT status FROM runs WHERE id = 'live'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "running"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn claim_process() {
    let Ok(path) = std::env::var("ALUCARD_CLAIM_TEST_PATH") else {
        return;
    };
    let resource = std::env::var("ALUCARD_CLAIM_TEST_RESOURCE").unwrap();
    let run_id = std::env::var("ALUCARD_CLAIM_TEST_RUN").unwrap();
    let path = PathBuf::from(path);
    let result = if let Some(pr_number) = resource.strip_prefix("pr:") {
        acquire_pr(&path, "repo", pr_number.parse().unwrap(), &run_id)
    } else {
        acquire_task(&path, &resource, &run_id)
    };
    match result {
        Ok(_) => println!("acquired"),
        Err(ClaimError::ClaimedBy { run_id }) => println!("claimed by run {run_id}"),
        Err(error) => panic!("claim failed: {error}"),
    }
}

fn run_claim_processes(path: &Path, resources: Vec<String>) -> Vec<String> {
    let executable = std::env::current_exe().unwrap();
    let barrier = Arc::new(Barrier::new(resources.len()));
    let handles = resources
        .into_iter()
        .enumerate()
        .map(|(number, task_id)| {
            let executable = executable.clone();
            let path = path.to_path_buf();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let output = Command::new(executable)
                    .args(["--exact", "claim_process", "--nocapture"])
                    .env("ALUCARD_CLAIM_TEST_PATH", path)
                    .env("ALUCARD_CLAIM_TEST_RESOURCE", task_id)
                    .env("ALUCARD_CLAIM_TEST_RUN", format!("run-{number}"))
                    .output()
                    .unwrap();
                assert!(output.status.success(), "{:?}", output);
                String::from_utf8(output.stdout).unwrap()
            })
        })
        .collect::<Vec<_>>();
    handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect()
}

#[test]
fn concurrent_processes_get_one_task_claim_without_lock_errors() {
    let path = database_path();
    prepare_claims(&path, &["task"]);
    for number in 0..8 {
        insert_run(&path, &format!("run-{number}"));
    }

    let output = run_claim_processes(&path, vec!["task".to_string(); 8]);
    assert_eq!(
        output
            .iter()
            .filter(|line| line.contains("acquired"))
            .count(),
        1
    );
    assert_eq!(
        output
            .iter()
            .filter(|line| line.contains("claimed by run run-"))
            .count(),
        7
    );
    assert!(!output
        .iter()
        .any(|line| line.contains("database is locked")));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn concurrent_processes_claim_different_tasks() {
    let path = database_path();
    prepare_claims(&path, &["first", "second"]);
    insert_run(&path, "run-0");
    insert_run(&path, "run-1");

    let output = run_claim_processes(&path, vec!["first".to_string(), "second".to_string()]);
    assert_eq!(
        output
            .iter()
            .filter(|line| line.contains("acquired"))
            .count(),
        2
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn pr_claims_are_exclusive_and_releases_require_the_owning_lease() {
    let path = database_path();
    prepare_claims(&path, &[]);
    insert_run(&path, "run-0");
    insert_run(&path, "run-1");

    let output = run_claim_processes(&path, vec!["pr:42".to_string(), "pr:42".to_string()]);
    assert_eq!(
        output
            .iter()
            .filter(|line| line.contains("acquired"))
            .count(),
        1
    );
    assert_eq!(
        output
            .iter()
            .filter(|line| line.contains("claimed by run"))
            .count(),
        1
    );

    let (claim, owner) = open(&path)
        .unwrap()
        .query_row(
            "SELECT id, run_id FROM claims WHERE state = 'active'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    heartbeat(&path, &claim, &owner).unwrap();
    let other_run = if owner == "run-0" { "run-1" } else { "run-0" };
    assert_eq!(
        release(&path, "other-lease", &owner),
        Err(ClaimError::LeaseNotActive)
    );
    assert_eq!(
        acquire_pr(&path, "repo", 42, other_run),
        Err(ClaimError::ClaimedBy {
            run_id: owner.clone()
        })
    );
    assert_eq!(
        release(&path, &claim, other_run),
        Err(ClaimError::LeaseNotActive)
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn task_run_pr_lease_blocks_continue_and_merge_without_overwriting_metadata() {
    let path = database_path();
    let directory = path.parent().unwrap();
    let repository = directory.join("repository");
    let logs = directory.join("logs");
    fs::create_dir_all(&repository).unwrap();
    migrate(&path).unwrap();
    let repository_id = repository_ledger_id(&repository).unwrap();
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, 'owner', 'name', 'now')",
            [&repository_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('task', ?1, 'github', 'task', 'task', 'pending', '[]', 'now')",
            [&repository_id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES ('run', ?1, 'run', 'running', 'now', 'now')",
            [&repository_id],
        )
        .unwrap();
    let task_claim = acquire_task(&path, "task", "run").unwrap();
    let pr_claim = record_task_pr(
        &path,
        "task",
        &task_claim.id,
        "run",
        42,
        "owner-branch",
        "owner-sha",
    )
    .unwrap();
    assert!(matches!(
        start_continue_run(
            &path,
            &repository,
            &logs,
            "image",
            42,
            "contender-branch",
            "contender-sha",
            "contender title",
            false,
        ),
        Err(ClaimError::ClaimedBy { .. })
    ));
    let metadata: (String, String) = open(&path)
        .unwrap()
        .query_row(
            "SELECT branch, head_sha FROM pull_requests WHERE number = 42",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        metadata,
        ("owner-branch".to_string(), "owner-sha".to_string())
    );

    let merge = start_merge_run(&path, &repository, &logs).unwrap();
    assert!(matches!(
        acquire_pr(&path, &merge.repository_id, 42, &merge.id),
        Err(ClaimError::ClaimedBy { .. })
    ));
    let (unrelated, unrelated_claim) = start_continue_run(
        &path,
        &repository,
        &logs,
        "image",
        43,
        "branch",
        "sha",
        "title",
        false,
    )
    .unwrap();
    release(&path, &unrelated_claim.id, &unrelated.run.id).unwrap();
    release(&path, &pr_claim.id, "run").unwrap();
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn runs_cannot_acquire_claims_for_another_repository() {
    let path = database_path();
    migrate(&path).unwrap();
    let connection = open(&path).unwrap();
    for repository_id in ["source", "target"] {
        connection
            .execute(
                "INSERT INTO repositories (id, owner, name, created_at) VALUES (?1, 'owner', ?1, 'now')",
                [repository_id],
            )
            .unwrap();
    }
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('target-task', 'target', 'github', 'target-task', 'task', 'pending', '[]', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES ('source-run', 'source', 'run', 'running', 'now', 'now')",
            [],
        )
        .unwrap();
    drop(connection);

    assert_eq!(
        acquire_task(&path, "target-task", "source-run"),
        Err(ClaimError::RunNotInRepository)
    );
    assert_eq!(
        acquire_pr(&path, "target", 42, "source-run"),
        Err(ClaimError::RunNotInRepository)
    );

    let connection = open(&path).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM tasks WHERE id = 'target-task'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "pending"
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM claims", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    drop(connection);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn pending_task_can_be_reacquired_after_its_lease_is_released() {
    let path = database_path();
    prepare_claims(&path, &["task"]);
    insert_run(&path, "run-0");
    insert_run(&path, "run-1");

    let claim = acquire_task(&path, "task", "run-0").unwrap();
    transition_task(&path, "task", &claim.id, "run-0", TaskStatus::Pending).unwrap();
    release(&path, &claim.id, "run-0").unwrap();

    let reacquired = acquire_task(&path, "task", "run-1").unwrap();
    assert_eq!(reacquired.run_id, "run-1");
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn opened_pr_moves_the_claimed_task_to_in_flight() {
    let path = database_path();
    prepare_claims(&path, &["task"]);
    insert_run(&path, "run");
    let claim = acquire_task(&path, "task", "run").unwrap();

    record_task_pr(&path, "task", &claim.id, "run", 42, "agent/task", "abc123").unwrap();

    let connection = open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT status FROM tasks WHERE id = 'task'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "in_flight"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT number FROM pull_requests WHERE task_id = 'task'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        42
    );
    record_task_pr_gate_state(&path, "task", 42, "ci_missing").unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT gate_state FROM pull_requests WHERE task_id = 'task'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "ci_missing"
    );
    record_task_pr_review(
        &path,
        "task",
        42,
        "APPROVED",
        "review_needs_human",
        Some("abc123"),
        "def456",
    )
    .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT review_verdict, gate_state, reviewer_approved_sha, head_sha FROM pull_requests WHERE task_id = 'task'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap(),
        (
            "APPROVED".to_string(),
            "review_needs_human".to_string(),
            Some("abc123".to_string()),
            "def456".to_string()
        )
    );
}

#[test]
fn abandoned_claims_only_include_dead_local_owners_and_can_be_released() {
    let path = database_path();
    prepare_claims(&path, &["dead", "live"]);
    let hostname = fs::read_to_string("/etc/hostname")
        .unwrap()
        .trim()
        .to_string();
    let connection = open(&path).unwrap();
    for (run, pid) in [
        ("dead-run", i64::MAX),
        ("live-run", i64::from(std::process::id())),
    ] {
        connection
            .execute(
                "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, pid, hostname)
                 VALUES (?1, 'repo', 'run', 'running', 'now', 'now', ?2, ?3)",
                (run, pid, &hostname),
            )
            .unwrap();
    }
    drop(connection);
    let dead = acquire_task(&path, "dead", "dead-run").unwrap();
    acquire_task(&path, "live", "live-run").unwrap();

    let abandoned = abandoned_task_claims(&path, "repo").unwrap();
    assert_eq!(abandoned.len(), 1);
    assert_eq!(abandoned[0].lease_id, dead.id);
    release_abandoned_task_claim(&path, &abandoned[0]).unwrap();

    let connection = open(&path).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT status FROM tasks WHERE id = 'dead'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "pending"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT state FROM claims WHERE id = ?1",
                [&dead.id],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "released"
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn remote_operations_are_durable_and_idempotent() {
    let path = database_path();
    migrate(&path).unwrap();
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES ('repo', 'owner', 'name', 'now')",
            [],
        )
        .unwrap();
    drop(connection);

    let first = begin_operation(&path, "repo", "recovery:branch", "push").unwrap();
    let second = begin_operation(&path, "repo", "recovery:branch", "push").unwrap();
    assert_eq!(first, second);
    finish_operation(&path, &first, Some("network unavailable")).unwrap();
    let connection = open(&path).unwrap();
    let operation: (String, i64, Option<String>) = connection
        .query_row(
            "SELECT phase, attempt_count, last_error FROM operations WHERE id = ?1",
            [&first],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(
        operation,
        (
            "failed".to_string(),
            1,
            Some("network unavailable".to_string())
        )
    );
    drop(connection);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn recovery_operations_replay_only_deferred_failures() {
    let path = database_path();
    migrate(&path).unwrap();
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES ('repo', 'owner', 'name', 'now')",
            [],
        )
        .unwrap();
    drop(connection);

    let operation = begin_operation_with_payload(
        &path,
        "repo",
        "recovery:branch",
        "create_recovery_pr",
        Some(r#"{"base":"main"}"#),
    )
    .unwrap();

    finish_operation(&path, &operation, Some("GitHub unavailable")).unwrap();
    assert!(pending_operations(&path, "repo").unwrap().is_empty());

    defer_operation(&path, &operation, "GitHub unavailable").unwrap();

    assert_eq!(
        pending_operations(&path, "repo").unwrap(),
        vec![alucard::PendingOperation {
            id: operation,
            target: "recovery:branch".to_string(),
            operation_type: "create_recovery_pr".to_string(),
            payload: Some(r#"{"base":"main"}"#.to_string()),
        }]
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn pending_recovery_operations_skip_a_live_retry_publisher() {
    let path = database_path();
    migrate(&path).unwrap();
    let hostname = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .unwrap()
        .trim()
        .to_string();
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES ('repo', 'owner', 'name', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('task', 'repo', 'local', 'task', 'task', 'claimed', '[]', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, branch, pid, hostname)
             VALUES ('owner', 'repo', 'run', 'running', 'now', 'now', 'owner-branch', 0, ?1)",
            [&hostname],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('claim', 'repo', 'task', 'task', 'owner', 'active', 'now', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, branch, pid, hostname, retry_owner_run_id)
             VALUES ('retry', 'repo', 'run', 'running', 'now', 'now', 'recovery-branch', ?1, ?2, 'owner')",
            (i64::from(std::process::id()), hostname),
        )
        .unwrap();
    drop(connection);
    begin_operation(&path, "repo", "recovery:recovery-branch", "push").unwrap();

    assert!(pending_recovery_operations(&path, "repo")
        .unwrap()
        .is_empty());

    open(&path)
        .unwrap()
        .execute("UPDATE runs SET pid = 0 WHERE id = 'retry'", [])
        .unwrap();
    assert_eq!(pending_recovery_operations(&path, "repo").unwrap().len(), 1);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn recovery_replay_lock_excludes_concurrent_recoverers() {
    let path = database_path();
    migrate(&path).unwrap();
    let (locked_sender, locked) = std::sync::mpsc::channel();
    let (release, release_receiver) = std::sync::mpsc::channel();
    let held_path = path.clone();
    let holder = thread::spawn(move || {
        let _lock = lock_recovery_operations(&held_path).unwrap();
        locked_sender.send(()).unwrap();
        release_receiver.recv().unwrap();
    });
    locked.recv().unwrap();

    let (acquired_sender, acquired) = std::sync::mpsc::channel();
    let (done, done_receiver) = std::sync::mpsc::channel();
    let waiting_path = path.clone();
    let waiter = thread::spawn(move || {
        let _lock = lock_recovery_operations(&waiting_path).unwrap();
        acquired_sender.send(()).unwrap();
        done_receiver.recv().unwrap();
    });
    assert!(acquired.recv_timeout(Duration::from_millis(50)).is_err());
    release.send(()).unwrap();
    acquired.recv_timeout(Duration::from_secs(1)).unwrap();
    done.send(()).unwrap();
    holder.join().unwrap();
    waiter.join().unwrap();
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn migration_is_idempotent_and_enforces_the_ledger_constraints() {
    let path = database_path();
    assert_eq!(status(&path).unwrap().pending_migrations, 9);
    assert_eq!(migrate(&path).unwrap().pending_migrations, 0);
    assert_eq!(migrate(&path).unwrap().pending_migrations, 0);

    let connection = open(&path).unwrap();
    assert!(connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('task', 'unknown', 'github', '1', 'task', 'pending', '[]', 'now')",
            [],
        )
        .is_err());
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES ('repo', 'owner', 'name', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at)
             VALUES ('run', 'repo', 'run', 'running', 'now', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('first', 'repo', 'task', 'task', 'run', 'active', 'now', 'now')",
            [],
        )
        .unwrap();
    assert!(connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('second', 'repo', 'task', 'task', 'run', 'active', 'now', 'now')",
            [],
        )
        .is_err());
    connection
        .execute(
            "UPDATE claims SET state = 'released' WHERE id = 'first'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('second', 'repo', 'task', 'task', 'run', 'active', 'now', 'now')",
            [],
        )
        .unwrap();

    #[cfg(unix)]
    {
        assert_eq!(
            fs::metadata(path.parent().unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(connection);
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn status_reports_pending_migrations_until_the_database_is_migrated() {
    let path = database_path();
    let binary = env!("CARGO_BIN_EXE_alucard");

    let status = Command::new(binary)
        .args(["--db-path", path.to_str().unwrap(), "db", "status"])
        .output()
        .unwrap();
    assert!(!status.status.success());
    assert!(String::from_utf8(status.stdout)
        .unwrap()
        .contains("pending migrations: 9"));

    let migrate = Command::new(binary)
        .args(["--db-path", path.to_str().unwrap(), "db", "migrate"])
        .status()
        .unwrap();
    assert!(migrate.success());

    let status = Command::new(binary)
        .args(["--db-path", path.to_str().unwrap(), "db", "status"])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert!(String::from_utf8(status.stdout)
        .unwrap()
        .contains("pending migrations: 0"));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn doctor_names_migration_command_when_ledger_is_pending() {
    let path = database_path();
    let env_file = path.with_file_name("doctor.env");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        &env_file,
        "GITHUB_TOKEN=test\nALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args([
            "--db-path",
            path.to_str().unwrap(),
            "doctor",
            ".",
            "--env-file",
            env_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("alucard db migrate"));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn runs_commands_list_recent_runs_and_show_claims_and_pull_requests() {
    let path = database_path();
    migrate(&path).unwrap();
    let connection = open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO repositories (id, owner, name, created_at) VALUES ('repo', 'owner', 'name', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, base_sha, image, logs_path)
             VALUES ('older', 'repo', 'run', 'completed', '2026-01-01', '2026-01-01', 'old', NULL, '/tmp/old'),
                    ('current', 'repo', 'continue', 'running', '2026-01-02', '2026-01-02', 'abc123', 'image:test', '/tmp/current')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO tasks (id, repository_id, source, external_id, title, status, blockers_json, updated_at)
             VALUES ('task-1', 'repo', 'github', '1', 'Task', 'claimed', '[]', 'now'),
                    ('task-2', 'repo', 'github', '2', 'Task', 'claimed', '[]', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
             VALUES ('claim-1', 'repo', 'task', 'task-1', 'current', 'active', 'now', 'now'),
                    ('claim-2', 'repo', 'pr', '42', 'current', 'active', 'now', 'now'),
                    ('claim-3', 'repo', 'pr', '43', 'current', 'active', 'now', 'now')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO pull_requests (id, task_id, repository_id, number, branch, head_sha, kind, remote_state, gate_state)
             VALUES ('pr-1', 'task-1', 'repo', 42, 'alucard/task-1', 'abc123', 'normal', 'open', 'pending'),
                    ('pr-2', 'task-2', 'repo', 43, 'alucard/task-2', 'def456', 'normal', 'open', 'pending')",
            [],
        )
        .unwrap();
    drop(connection);

    let binary = env!("CARGO_BIN_EXE_alucard");
    let list = Command::new(binary)
        .args(["--db-path", path.to_str().unwrap(), "runs"])
        .output()
        .unwrap();
    assert!(list.status.success());
    assert_eq!(
        String::from_utf8(list.stdout).unwrap(),
        "current\trunning\tcontinue\t/tmp/current\nolder\tcompleted\trun\t/tmp/old\n"
    );

    let show = Command::new(binary)
        .args([
            "--db-path",
            path.to_str().unwrap(),
            "runs",
            "show",
            "current",
        ])
        .output()
        .unwrap();
    assert!(show.status.success());
    let stdout = String::from_utf8(show.stdout).unwrap();
    assert!(stdout.contains("id: current\nrepository: repo\ncommand: continue\nstatus: running"));
    assert!(stdout.contains("base sha: abc123\nimage: image:test\nlogs: /tmp/current"));
    assert!(stdout.contains("claim: task task-1 active"));
    assert!(stdout.contains("pr: #42 alucard/task-1"));
    assert_eq!(stdout.matches("pr: #42 alucard/task-1").count(), 1);
    assert!(stdout.contains("pr: #43 alucard/task-2"));
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[cfg(unix)]
#[test]
fn status_preserves_existing_database_parent_permissions() {
    let path = database_path();
    let parent = path.parent().unwrap();
    fs::create_dir(parent).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(status(&path).unwrap().pending_migrations, 9);
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o755
    );
    fs::remove_dir_all(parent).unwrap();
}
