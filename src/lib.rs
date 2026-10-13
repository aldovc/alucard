//! SQLite ledger support for the Alucard orchestrator.

use rusqlite::{Connection, TransactionBehavior};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MIGRATIONS: &[(i64, &str)] = &[(1, include_str!("../migrations/0001_init.sql"))];

pub struct DatabaseStatus {
    pub schema_version: i64,
    pub pending_migrations: usize,
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
