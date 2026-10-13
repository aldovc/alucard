//! SQLite ledger support for the Alucard orchestrator.

use rusqlite::{Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LocalTask {
    pub id: String,
    pub title: String,
    pub body: String,
    #[serde(skip)]
    pub state: TaskState,
    #[serde(skip)]
    pub line: usize,
    #[serde(skip)]
    pub pr_number: Option<i64>,
    #[serde(skip)]
    pub previous_pr_number: Option<i64>,
    #[serde(skip)]
    pub blockers: Vec<String>,
    #[serde(skip)]
    pub issue_blockers: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct GithubTask {
    pub number: i64,
    pub title: String,
    pub body: String,
    pub labels: Vec<GithubLabel>,
    #[serde(default, skip_serializing)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct GithubLabel {
    pub name: String,
}

pub fn github_ticket_blockers(body: &str) -> Vec<i64> {
    let mut blockers = Vec::new();
    for line in body.lines() {
        let mut rest = line;
        while let Some(index) = rest.find("Blocked by #") {
            rest = &rest[index + "Blocked by #".len()..];
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(number) = rest[..digits].parse() {
                blockers.push(number);
            }
            rest = &rest[digits..];
        }
    }
    let mut in_section = false;
    for line in body.lines() {
        if let Some(heading) = line.strip_prefix("##") {
            in_section = heading
                .trim_start_matches(char::is_whitespace)
                .strip_prefix("Blocked by")
                .is_some_and(|rest| {
                    rest.is_empty() || rest.chars().next().is_some_and(char::is_whitespace)
                });
            continue;
        }
        if in_section {
            let mut rest = line;
            while let Some(index) = rest.find('#') {
                rest = &rest[index + 1..];
                let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
                if let Ok(number) = rest[..digits].parse() {
                    blockers.push(number);
                }
                rest = &rest[digits..];
            }
        }
    }
    blockers.sort_unstable();
    blockers.dedup();
    blockers
}

pub fn github_queue(
    issues: Vec<GithubTask>,
    open_issue_numbers: &[i64],
    open_prs: &[GithubPullRequest],
) -> Vec<GithubTask> {
    let mut queue: Vec<_> = issues
        .into_iter()
        .filter(|issue| {
            let labels: Vec<_> = issue
                .labels
                .iter()
                .map(|label| label.name.as_str())
                .collect();
            labels.contains(&"ready-for-agent")
                && !labels
                    .iter()
                    .any(|label| matches!(*label, "ready-for-human" | "in-progress" | "wip"))
                && github_ticket_blockers(&issue.body)
                    .iter()
                    .all(|blocker| !open_issue_numbers.contains(blocker))
                && !open_prs.iter().any(|pr| pr.covers(issue.number))
        })
        .collect();
    queue.sort_by_key(|issue| {
        (
            !issue
                .labels
                .iter()
                .any(|label| label.name == "priority:high"),
            issue.number,
        )
    });
    queue
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct GithubPullRequest {
    pub number: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default, rename = "closingIssuesReferences")]
    pub closing_issues_references: ClosingIssuesReferences,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize)]
pub struct ClosingIssuesReferences {
    #[serde(default)]
    pub nodes: Vec<IssueReference>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct IssueReference {
    pub number: i64,
}

impl GithubPullRequest {
    pub fn covers(&self, issue_number: i64) -> bool {
        self.closing_issues_references
            .nodes
            .iter()
            .any(|issue| issue.number == issue_number)
            || [self.title.as_str(), self.body.as_str()]
                .iter()
                .any(|text| references_issue(text, issue_number))
    }
}

fn references_issue(text: &str, issue_number: i64) -> bool {
    let reference = format!("#{issue_number}");
    text.match_indices(&reference).any(|(index, _)| {
        !text[index + reference.len()..]
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Pending,
    InFlight,
    Done,
    Human,
}

impl TaskState {
    fn from_marker(marker: char) -> Self {
        match marker {
            ' ' => Self::Pending,
            '>' => Self::InFlight,
            'x' => Self::Done,
            'h' => Self::Human,
            _ => unreachable!("validated task state"),
        }
    }

    fn ledger_status(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InFlight => "in_flight",
            Self::Done => "done",
            Self::Human => "human",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ParsedTasks {
    pub parent_context: String,
    pub tasks: Vec<LocalTask>,
}

pub fn parse_tasks(contents: &str) -> Result<ParsedTasks, Vec<String>> {
    let contents = contents.replace('\r', "");
    let lines: Vec<_> = contents.lines().collect();
    let mut parent = Vec::new();
    let mut tasks = Vec::new();
    let mut current: Option<LocalTask> = None;
    let mut errors = Vec::new();

    for (index, line) in lines.iter().enumerate() {
        if line.strip_prefix("##").is_some_and(|rest| {
            rest.trim_start_matches(|character: char| character.is_ascii_whitespace())
                .starts_with('[')
        }) {
            let heading = line.strip_prefix("## ").unwrap_or("");
            let Some((marker, rest)) = heading
                .strip_prefix('[')
                .and_then(|value| {
                    value
                        .chars()
                        .next()
                        .map(|marker| (marker, &value[marker.len_utf8()..]))
                })
                .and_then(|(marker, value)| value.strip_prefix("] ").map(|rest| (marker, rest)))
            else {
                errors.push(format!(
                    "line {}: malformed task heading: {line}",
                    index + 1
                ));
                continue;
            };
            let Some((id, title)) = rest.split_once(": ") else {
                errors.push(format!(
                    "line {}: malformed task heading: {line}",
                    index + 1
                ));
                continue;
            };
            if !matches!(marker, ' ' | '>' | 'x' | 'h')
                || id.is_empty()
                || title.is_empty()
                || !id.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
            {
                errors.push(format!(
                    "line {}: malformed task heading: {line}",
                    index + 1
                ));
                continue;
            }
            if let Some(task) = current.take() {
                tasks.push(task);
            }
            let (title, pr_number) = split_pr_suffix(title, "(PR #");
            let (title, previous_pr_number) = split_pr_suffix(title, "(previous attempt: PR #");
            current = Some(LocalTask {
                id: id.to_string(),
                title: title.to_string(),
                body: String::new(),
                state: TaskState::from_marker(marker),
                line: index + 1,
                pr_number,
                previous_pr_number,
                blockers: Vec::new(),
                issue_blockers: Vec::new(),
            });
        } else if let Some(task) = current.as_mut() {
            if !task.body.is_empty() {
                task.body.push('\n');
            }
            task.body.push_str(line);
        } else {
            parent.push(*line);
        }
    }
    if let Some(task) = current {
        tasks.push(task);
    }
    for task in &mut tasks {
        task.body = task.body.trim_matches('\n').to_string();
        for line in task
            .body
            .lines()
            .filter_map(|line| line.strip_prefix("Blocked by:"))
        {
            for token in line.split(|character: char| character == ',' || character.is_whitespace())
            {
                if let Some(number) = token
                    .strip_prefix('#')
                    .and_then(|number| number.parse().ok())
                {
                    task.issue_blockers.push(number);
                } else if !token.is_empty() && token != "none" {
                    task.blockers.push(token.to_string());
                }
            }
        }
        let mut body = task.body.as_str();
        while let Some(index) = body.find("Blocked by #") {
            let number = &body[index + "Blocked by #".len()..];
            let digits = number.bytes().take_while(u8::is_ascii_digit).count();
            if let Ok(number) = number[..digits].parse() {
                task.issue_blockers.push(number);
            }
            body = if digits == 0 {
                &number[number.chars().next().map_or(0, char::len_utf8)..]
            } else {
                &number[digits..]
            };
        }
    }
    if errors.is_empty() {
        Ok(ParsedTasks {
            parent_context: parent.join("\n").trim_end_matches('\n').to_string(),
            tasks,
        })
    } else {
        Err(errors)
    }
}

fn split_pr_suffix<'a>(title: &'a str, prefix: &str) -> (&'a str, Option<i64>) {
    let Some(start) = title.rfind(prefix) else {
        return (title, None);
    };
    let number = &title[start + prefix.len()..];
    let Some(number) = number.strip_suffix(')') else {
        return (title, None);
    };
    if !number.bytes().all(|character| character.is_ascii_digit()) {
        return (title, None);
    }
    let Ok(number) = number.parse() else {
        return (title, None);
    };
    (title[..start].trim_end(), Some(number))
}

pub fn local_queue(tasks: &[LocalTask]) -> Vec<LocalTask> {
    local_queue_with_open_issues(tasks, None)
}

pub fn local_queue_with_open_issues(
    tasks: &[LocalTask],
    open_issues: Option<&[i64]>,
) -> Vec<LocalTask> {
    let states: std::collections::HashMap<_, _> = tasks
        .iter()
        .map(|task| (task.id.as_str(), task.state))
        .collect();
    tasks
        .iter()
        .filter(|task| {
            task.state == TaskState::Pending
                && task.issue_blockers.iter().all(|blocker| {
                    open_issues
                        .map(|open| !open.contains(blocker))
                        .unwrap_or(false)
                })
                && task
                    .blockers
                    .iter()
                    .all(|blocker| states.get(blocker.as_str()) == Some(&TaskState::Done))
        })
        .cloned()
        .collect()
}

pub fn import_local_tasks(
    connection: &mut Connection,
    repository: &str,
    source_path: &Path,
    tasks: &[LocalTask],
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO repositories (id, owner, name, cache_path, created_at) VALUES (?1, ?1, ?1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        [repository],
    )?;
    let source_path = source_path.to_string_lossy();
    for task in tasks {
        let blockers = serde_json::to_string(&task.blockers).expect("string blockers serialize");
        transaction.execute(
            "INSERT INTO tasks (id, repository_id, source, source_path, external_id, title, status, blockers_json, pr_number, previous_pr_number, updated_at)
             VALUES (lower(hex(randomblob(16))), ?1, 'local', ?2, ?3, ?4, ?5, ?6, ?7, ?8, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(repository_id, source, source_path, external_id) DO UPDATE SET
               title = excluded.title, status = excluded.status, blockers_json = excluded.blockers_json,
               pr_number = excluded.pr_number, previous_pr_number = excluded.previous_pr_number,
               updated_at = excluded.updated_at
             WHERE title IS NOT excluded.title OR status IS NOT excluded.status OR blockers_json IS NOT excluded.blockers_json
               OR pr_number IS NOT excluded.pr_number OR previous_pr_number IS NOT excluded.previous_pr_number",
            rusqlite::params![repository, source_path, task.id, task.title, task.state.ledger_status(), blockers, task.pr_number, task.previous_pr_number],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn import_github_tasks(
    connection: &mut Connection,
    repository: &str,
    tasks: &[GithubTask],
) -> Result<(), DatabaseError> {
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR IGNORE INTO repositories (id, owner, name, cache_path, created_at) VALUES (?1, ?1, ?1, ?1, strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
        [repository],
    )?;
    for task in tasks {
        let blockers = serde_json::to_string(&github_ticket_blockers(&task.body))
            .expect("issue blockers serialize");
        transaction.execute(
            "INSERT INTO tasks (id, repository_id, source, source_path, external_id, title, status, blockers_json, updated_at)
             VALUES (lower(hex(randomblob(16))), ?1, 'github', '', ?2, ?3, 'pending', ?4, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
             ON CONFLICT(repository_id, source, source_path, external_id) DO UPDATE SET
               title = excluded.title, status = excluded.status, blockers_json = excluded.blockers_json,
               updated_at = excluded.updated_at
             WHERE title IS NOT excluded.title OR status IS NOT excluded.status OR blockers_json IS NOT excluded.blockers_json",
            rusqlite::params![repository, task.number.to_string(), task.title, blockers],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

pub struct DatabaseStatus {
    pub schema_version: i64,
    pub pending_migrations: usize,
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
