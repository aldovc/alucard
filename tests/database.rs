use alucard::{acquire_pr, acquire_task, heartbeat, migrate, open, release, status, ClaimError};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

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
        release(&path, &claim, other_run),
        Err(ClaimError::LeaseNotActive)
    );
    assert_eq!(
        acquire_pr(&path, "repo", 42, other_run),
        Err(ClaimError::ClaimedBy { run_id: owner })
    );
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
}

#[test]
fn migration_is_idempotent_and_enforces_the_ledger_constraints() {
    let path = database_path();
    assert_eq!(status(&path).unwrap().pending_migrations, 1);
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
        .contains("pending migrations: 1"));

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

#[cfg(unix)]
#[test]
fn status_preserves_existing_database_parent_permissions() {
    let path = database_path();
    let parent = path.parent().unwrap();
    fs::create_dir(parent).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(status(&path).unwrap().pending_migrations, 1);
    assert_eq!(
        fs::metadata(parent).unwrap().permissions().mode() & 0o777,
        0o755
    );
    fs::remove_dir_all(parent).unwrap();
}
