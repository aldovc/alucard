//! SQLite ledger support for the Alucard orchestrator.

use fs2::FileExt;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::env;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};
use uuid::Uuid;

const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

pub struct DatabaseStatus {
    pub schema_version: i64,
    pub pending_migrations: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Run {
    pub id: String,
    pub repository_id: String,
    pub command: String,
    pub status: String,
    pub base_sha: Option<String>,
    pub image: Option<String>,
    pub logs_path: Option<String>,
}

pub struct PreparedRun {
    pub run: Run,
    pub directory: PathBuf,
    pub clone: PathBuf,
    pub branch: String,
}

pub struct RunOptions<'a> {
    pub repository: &'a Path,
    pub logs_root: &'a Path,
    pub cache_root: &'a Path,
    pub base: &'a str,
    pub image: Option<&'a str>,
    pub task: Option<&'a str>,
    pub issue: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Claim {
    pub id: String,
    pub repository_id: String,
    pub resource_kind: ClaimResource,
    pub resource_key: String,
    pub run_id: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimResource {
    Task,
    PullRequest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Claimed,
    InFlight,
    Done,
    Human,
}

impl TaskStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::InFlight => "in_flight",
            Self::Done => "done",
            Self::Human => "human",
        }
    }
}

impl ClaimResource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::PullRequest => "pr",
        }
    }
}

#[derive(Debug)]
pub enum ClaimError {
    Database(DatabaseError),
    ClaimedBy { run_id: String },
    RunNotInRepository,
    TaskNotEligible,
    LeaseNotActive,
}

impl std::fmt::Display for ClaimError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::ClaimedBy { run_id } => write!(formatter, "claimed by run {run_id}"),
            Self::RunNotInRepository => write!(formatter, "run does not belong to the repository"),
            Self::TaskNotEligible => write!(formatter, "task is not eligible for a claim"),
            Self::LeaseNotActive => write!(formatter, "lease is not active"),
        }
    }
}

impl std::error::Error for ClaimError {}

impl PartialEq for ClaimError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ClaimedBy { run_id: left }, Self::ClaimedBy { run_id: right }) => left == right,
            (Self::RunNotInRepository, Self::RunNotInRepository)
            | (Self::TaskNotEligible, Self::TaskNotEligible)
            | (Self::LeaseNotActive, Self::LeaseNotActive) => true,
            _ => false,
        }
    }
}

impl Eq for ClaimError {}

impl From<DatabaseError> for ClaimError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}

impl From<rusqlite::Error> for ClaimError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.into())
    }
}

pub fn acquire_task(path: &Path, task_id: &str, run_id: &str) -> Result<Claim, ClaimError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let (repository_id, status): (String, String) = transaction
        .query_row(
            "SELECT repository_id, status FROM tasks WHERE id = ?1",
            [task_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| match error {
            rusqlite::Error::QueryReturnedNoRows => ClaimError::TaskNotEligible,
            other => ClaimError::Database(other.into()),
        })?;
    if let Some(run_id) = active_claim(&transaction, &repository_id, ClaimResource::Task, task_id)?
    {
        return Err(ClaimError::ClaimedBy { run_id });
    }
    if status != "pending" {
        return Err(ClaimError::TaskNotEligible);
    }
    let claim = acquire(
        &transaction,
        &repository_id,
        ClaimResource::Task,
        task_id,
        run_id,
    )?;
    let changed = transaction.execute(
        "UPDATE tasks SET status = 'claimed', updated_at = datetime('now') WHERE id = ?1",
        (task_id,),
    )?;
    if changed != 1 {
        return Err(ClaimError::TaskNotEligible);
    }
    transaction.commit()?;
    Ok(claim)
}

pub fn acquire_pr(
    path: &Path,
    repository_id: &str,
    pr_number: i64,
    run_id: &str,
) -> Result<Claim, ClaimError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let claim = acquire(
        &transaction,
        repository_id,
        ClaimResource::PullRequest,
        &pr_number.to_string(),
        run_id,
    )?;
    transaction.commit()?;
    Ok(claim)
}

pub fn heartbeat(path: &Path, lease_id: &str, run_id: &str) -> Result<(), ClaimError> {
    update_active_claim(path, lease_id, run_id, "heartbeat_at = datetime('now')")
}

pub fn release(path: &Path, lease_id: &str, run_id: &str) -> Result<(), ClaimError> {
    update_active_claim(
        path,
        lease_id,
        run_id,
        "state = 'released', heartbeat_at = datetime('now')",
    )
}

pub fn transition_task(
    path: &Path,
    task_id: &str,
    lease_id: &str,
    run_id: &str,
    status: TaskStatus,
) -> Result<(), ClaimError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE tasks SET status = ?1, updated_at = datetime('now')
         WHERE id = ?2 AND EXISTS (
             SELECT 1 FROM claims
             WHERE id = ?3 AND run_id = ?4 AND resource_kind = 'task'
               AND resource_key = ?2 AND state = 'active'
         )",
        (status.as_str(), task_id, lease_id, run_id),
    )?;
    if changed != 1 {
        return Err(ClaimError::LeaseNotActive);
    }
    transaction.commit()?;
    Ok(())
}

fn acquire(
    transaction: &rusqlite::Transaction<'_>,
    repository_id: &str,
    resource_kind: ClaimResource,
    resource_key: &str,
    run_id: &str,
) -> Result<Claim, ClaimError> {
    let run_belongs_to_repository: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id = ?1 AND repository_id = ?2)",
        (run_id, repository_id),
        |row| row.get(0),
    )?;
    if !run_belongs_to_repository {
        return Err(ClaimError::RunNotInRepository);
    }
    if let Some(run_id) = active_claim(transaction, repository_id, resource_kind, resource_key)? {
        return Err(ClaimError::ClaimedBy { run_id });
    }
    let claim = Claim {
        id: Uuid::new_v4().to_string(),
        repository_id: repository_id.to_string(),
        resource_kind,
        resource_key: resource_key.to_string(),
        run_id: run_id.to_string(),
    };
    transaction.execute(
        "INSERT INTO claims (id, repository_id, resource_kind, resource_key, run_id, state, acquired_at, heartbeat_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'active', datetime('now'), datetime('now'))",
        (&claim.id, &claim.repository_id, resource_kind.as_str(), &claim.resource_key, &claim.run_id),
    )?;
    Ok(claim)
}

fn active_claim(
    transaction: &rusqlite::Transaction<'_>,
    repository_id: &str,
    resource_kind: ClaimResource,
    resource_key: &str,
) -> Result<Option<String>, ClaimError> {
    transaction
        .query_row(
            "SELECT run_id FROM claims
             WHERE repository_id = ?1 AND resource_kind = ?2 AND resource_key = ?3 AND state = 'active'",
            (repository_id, resource_kind.as_str(), resource_key),
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

fn update_active_claim(
    path: &Path,
    lease_id: &str,
    run_id: &str,
    assignment: &str,
) -> Result<(), ClaimError> {
    let connection = open(path)?;
    let changed = connection.execute(
        &format!(
            "UPDATE claims SET {assignment} WHERE id = ?1 AND run_id = ?2 AND state = 'active'"
        ),
        (lease_id, run_id),
    )?;
    if changed == 1 {
        Ok(())
    } else {
        Err(ClaimError::LeaseNotActive)
    }
}

/// Executes an external command without involving a shell.
pub fn run_command(
    program: &str,
    arguments: &[&str],
    cwd: &Path,
    timeout: Option<Duration>,
) -> io::Result<Output> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    if let Some(timeout) = timeout {
        let started = Instant::now();
        while child.try_wait()?.is_none() {
            if started.elapsed() >= timeout {
                child.kill()?;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    child.wait_with_output()
}

fn run_checked(program: &str, arguments: &[&str], cwd: &Path) -> Result<Output, RunError> {
    let output = run_command(program, arguments, cwd, None)?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(RunError::Command {
            program: program.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

pub fn prepare_run(path: &Path, options: RunOptions<'_>) -> Result<PreparedRun, RunError> {
    migrate(path)?;

    let repository = options.repository.canonicalize()?;
    let repository_name = repository
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "repository has no name"))?;
    let repository_id = repository.to_string_lossy().to_string();
    let cache_key = repository_cache_key(&repository);
    let cache = options.cache_root.join(&cache_key);
    fs::create_dir_all(options.cache_root)?;
    let lock = File::create(options.cache_root.join(format!("{cache_key}.lock")))?;
    lock.lock_exclusive()?;
    if cache.exists() {
        run_checked("git", &["fetch", "--quiet", "origin"], &cache)?;
    } else {
        run_checked(
            "git",
            &[
                "clone",
                "--quiet",
                repository.to_str().unwrap_or_default(),
                cache.to_str().unwrap_or_default(),
            ],
            options.cache_root,
        )?;
    }
    let remote_base = format!("origin/{}", options.base);
    let base_sha =
        String::from_utf8_lossy(&run_checked("git", &["rev-parse", &remote_base], &cache)?.stdout)
            .trim()
            .to_string();

    let run_id = Uuid::new_v4().to_string();
    let directory = options.logs_root.join(&run_id);
    fs::create_dir_all(&directory)?;
    let clone = directory.join("repository");
    run_checked(
        "git",
        &[
            "clone",
            "--quiet",
            cache.to_str().unwrap_or_default(),
            clone.to_str().unwrap_or_default(),
        ],
        &directory,
    )?;
    FileExt::unlock(&lock)?;
    if !clone.join(".git").is_dir() {
        return Err(RunError::InvalidClone);
    }
    let suffix = options
        .issue
        .map(|issue| format!("issue-{issue}"))
        .or_else(|| options.task.map(|task| format!("task-{task}")));
    let branch = format!(
        "alucard/{run_id}/iter-1{}",
        suffix.map(|value| format!("-{value}")).unwrap_or_default()
    );
    run_checked(
        "git",
        &["switch", "--quiet", "-c", &branch, &base_sha],
        &clone,
    )?;

    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR IGNORE INTO repositories (id, owner, name, cache_path, created_at)
         VALUES (?1, '', ?2, ?3, datetime('now'))",
        params![repository_id, repository_name, cache.to_string_lossy()],
    )?;
    transaction.execute(
        "INSERT INTO runs (id, repository_id, command, status, started_at, heartbeat_at, base_sha, image, logs_path)
         VALUES (?1, ?2, 'run', 'running', datetime('now'), datetime('now'), ?3, ?4, ?5)",
        params![run_id, repository_id, base_sha, options.image, directory.to_string_lossy()],
    )?;
    transaction.commit()?;
    Ok(PreparedRun {
        run: Run {
            id: run_id,
            repository_id,
            command: "run".to_string(),
            status: "running".to_string(),
            base_sha: Some(base_sha),
            image: options.image.map(str::to_string),
            logs_path: Some(directory.to_string_lossy().to_string()),
        },
        directory,
        clone,
        branch,
    })
}

fn repository_cache_key(repository: &Path) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = repository.as_os_str().as_encoded_bytes();
    let mut key = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        key.push(HEX[(byte >> 4) as usize] as char);
        key.push(HEX[(byte & 0x0f) as usize] as char);
    }
    key
}

pub fn interrupt_running_runs(path: &Path) -> Result<usize, DatabaseError> {
    let connection = open(path)?;
    Ok(connection.execute(
        "UPDATE runs SET status = 'interrupted', ended_at = datetime('now') WHERE status = 'running'",
        [],
    )?)
}

pub fn recent_runs(path: &Path) -> Result<Vec<Run>, DatabaseError> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT id, repository_id, command, status, base_sha, image, logs_path
         FROM runs ORDER BY started_at DESC, rowid DESC LIMIT 20",
    )?;
    let runs = statement
        .query_map([], run_from_row)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(DatabaseError::from)?;
    Ok(runs)
}

pub fn find_run(path: &Path, run_id: &str) -> Result<Option<Run>, DatabaseError> {
    let connection = open(path)?;
    connection
        .query_row(
            "SELECT id, repository_id, command, status, base_sha, image, logs_path FROM runs WHERE id = ?1",
            [run_id],
            run_from_row,
        )
        .optional()
        .map_err(Into::into)
}

pub fn run_claims(
    path: &Path,
    run_id: &str,
) -> Result<Vec<(String, String, String)>, DatabaseError> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT resource_kind, resource_key, state FROM claims WHERE run_id = ?1 ORDER BY acquired_at",
    )?;
    let claims = statement
        .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(claims)
}

pub fn run_pull_requests(path: &Path, run_id: &str) -> Result<Vec<(i64, String)>, DatabaseError> {
    let connection = open(path)?;
    let mut statement = connection.prepare(
        "SELECT pull_requests.number, pull_requests.branch
         FROM pull_requests JOIN claims ON claims.resource_key = pull_requests.task_id
         WHERE claims.run_id = ?1 AND claims.resource_kind = 'task' ORDER BY pull_requests.number",
    )?;
    let pull_requests = statement
        .query_map([run_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(pull_requests)
}

fn run_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Run> {
    Ok(Run {
        id: row.get(0)?,
        repository_id: row.get(1)?,
        command: row.get(2)?,
        status: row.get(3)?,
        base_sha: row.get(4)?,
        image: row.get(5)?,
        logs_path: row.get(6)?,
    })
}

#[derive(Debug)]
pub enum RunError {
    Database(DatabaseError),
    Io(io::Error),
    Command { program: String, stderr: String },
    InvalidClone,
}

impl From<DatabaseError> for RunError {
    fn from(error: DatabaseError) -> Self {
        Self::Database(error)
    }
}
impl From<io::Error> for RunError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<rusqlite::Error> for RunError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Database(error.into())
    }
}
impl std::fmt::Display for RunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Database(error) => error.fmt(formatter),
            Self::Io(error) => error.fmt(formatter),
            Self::Command { program, stderr } => write!(formatter, "{program} failed: {stderr}"),
            Self::InvalidClone => write!(
                formatter,
                "git clone did not create a self-contained repository"
            ),
        }
    }
}
impl std::error::Error for RunError {}

pub fn database_path(override_path: Option<PathBuf>) -> io::Result<PathBuf> {
    override_path
        .or_else(|| env::var_os("ALUCARD_DB_PATH").map(PathBuf::from))
        .or_else(|| {
            env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .map(|path| path.join("alucard/alucard.db"))
        })
        .or_else(|| {
            env::var_os("HOME")
                .map(PathBuf::from)
                .map(|path| path.join(".local/state/alucard/alucard.db"))
        })
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "database path is not set; use --db-path or ALUCARD_DB_PATH",
            )
        })
}

pub fn migrate(path: &Path) -> Result<DatabaseStatus, DatabaseError> {
    let mut connection = open(path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY
        );",
    )?;

    for &(version, sql) in MIGRATIONS {
        if !is_applied(&transaction, version)? {
            transaction.execute_batch(sql)?;
            transaction.execute(
                "INSERT INTO schema_migrations (version) VALUES (?1)",
                [version],
            )?;
        }
    }
    transaction.commit()?;
    status(path)
}

pub fn status(path: &Path) -> Result<DatabaseStatus, DatabaseError> {
    let connection = open(path)?;
    let applied = applied_versions(&connection)?;
    let schema_version = applied.iter().copied().max().unwrap_or(0);
    let pending_migrations = MIGRATIONS
        .iter()
        .filter(|(version, _)| !applied.contains(version))
        .count();
    Ok(DatabaseStatus {
        schema_version,
        pending_migrations,
    })
}

pub fn open(path: &Path) -> Result<Connection, DatabaseError> {
    prepare_directory(path)?;
    let connection = Connection::open(path)?;
    restrict_file_permissions(path)?;
    connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

fn is_applied(connection: &Connection, version: i64) -> Result<bool, rusqlite::Error> {
    connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM schema_migrations WHERE version = ?1)",
        [version],
        |row| row.get(0),
    )
}

fn applied_versions(connection: &Connection) -> Result<Vec<i64>, rusqlite::Error> {
    let table_exists: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations')",
        [],
        |row| row.get(0),
    )?;
    if !table_exists {
        return Ok(Vec::new());
    }
    let mut statement = connection.prepare("SELECT version FROM schema_migrations")?;
    let versions = statement
        .query_map([], |row| row.get(0))?
        .collect::<Result<Vec<i64>, _>>()?;
    Ok(versions)
}

fn prepare_directory(path: &Path) -> io::Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    if let Some(grandparent) = parent
        .parent()
        .filter(|grandparent| !grandparent.as_os_str().is_empty())
    {
        fs::create_dir_all(grandparent)?;
    }
    match fs::create_dir(parent) {
        Ok(()) => restrict_directory_permissions(parent)?,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    Ok(())
}

fn restrict_directory_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

fn restrict_file_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[derive(Debug)]
pub enum DatabaseError {
    Io(io::Error),
    Sql(rusqlite::Error),
}

impl From<io::Error> for DatabaseError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for DatabaseError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sql(error)
    }
}

impl std::fmt::Display for DatabaseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::Sql(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for DatabaseError {}
