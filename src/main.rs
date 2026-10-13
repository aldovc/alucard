//! Alucard orchestrator: the Rust port of the `alucard` Bash script.

use alucard::{
    database_path, import_local_tasks, local_queue_with_open_issues, migrate, open, parse_tasks,
    status,
};
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (db_path, commands) = match parse_db_path(&args) {
        Ok(parsed) => parsed,
        Err(message) => return fail(&message),
    };
    match commands.first().map(String::as_str) {
        Some("version" | "--version" | "-V") => {
            println!("{}", version());
            ExitCode::SUCCESS
        }
        Some("db") => match database_path(db_path) {
            Ok(path) => database_command(&commands[1..], path),
            Err(error) => fail(&format!("alucard: {error}")),
        },
        Some("queue") => queue_command(&commands[1..], db_path),
        Some(cmd) => fail(&format!("alucard: unknown command: {cmd}")),
        None => fail("usage: alucard [--db-path PATH] {version|queue|db migrate|db status}"),
    }
}

fn queue_command(args: &[String], db_path: Option<PathBuf>) -> ExitCode {
    let options = match QueueOptions::parse(args) {
        Ok(options) => options,
        Err(message) => return fail(&format!("alucard: {message}")),
    };
    let task_file = match options.task_file() {
        Ok(Some(path)) => path,
        Ok(None) => return fail("alucard: GitHub queue is not implemented"),
        Err(message) => return fail(&format!("alucard: {message}")),
    };
    let contents = match std::fs::read_to_string(&task_file) {
        Ok(contents) => contents,
        Err(error) => {
            return fail(&format!(
                "alucard: cannot read {}: {error}",
                task_file.display()
            ))
        }
    };
    let parsed = match parse_tasks(&contents) {
        Ok(parsed) => parsed,
        Err(errors) => {
            return fail(&format!(
                "alucard: invalid tasks file {}:\n{}",
                task_file.display(),
                errors.join("\n")
            ))
        }
    };
    let path = match database_path(db_path) {
        Ok(path) => path,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    if let Err(error) = migrate(&path).and_then(|_| {
        let mut connection = open(&path)?;
        import_local_tasks(
            &mut connection,
            &options.repository.display().to_string(),
            &task_file,
            &parsed.tasks,
        )
    }) {
        return fail(&format!("alucard: database error: {error}"));
    }
    let locally_eligible = local_queue_with_open_issues(&parsed.tasks, Some(&[]));
    let open_issues = locally_eligible
        .iter()
        .any(|task| !task.issue_blockers.is_empty())
        .then(|| fetch_open_issue_numbers(&options.repository))
        .flatten();
    match serde_json::to_string_pretty(&local_queue_with_open_issues(
        &parsed.tasks,
        open_issues.as_deref(),
    )) {
        Ok(queue) => {
            println!("{queue}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("alucard: cannot serialize queue: {error}")),
    }
}

fn fetch_open_issue_numbers(repository: &PathBuf) -> Option<Vec<i64>> {
    let output = Command::new("gh")
        .current_dir(repository)
        .args([
            "issue", "list", "--state", "open", "--json", "number", "--limit", "1000",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout)
        .ok()?
        .into_iter()
        .map(|issue| issue.get("number")?.as_i64())
        .collect()
}

struct QueueOptions {
    repository: PathBuf,
    repository_set: bool,
    tasks: Option<PathBuf>,
    tasks_from_flag: bool,
    github: bool,
}

impl QueueOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut options = Self {
            repository: std::env::var_os("ALUCARD_TARGET_REPO")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().expect("current directory")),
            tasks: std::env::var_os("ALUCARD_TASKS_FILE").map(PathBuf::from),
            repository_set: false,
            tasks_from_flag: false,
            github: false,
        };
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--tasks" => {
                    options.tasks =
                        Some(PathBuf::from(args.next().ok_or("--tasks requires a path")?));
                    options.tasks_from_flag = true;
                }
                "--github" => options.github = true,
                "-C" | "--repo" => {
                    options.repository =
                        PathBuf::from(args.next().ok_or("--repo requires a path")?);
                    options.repository_set = true;
                }
                value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
                value if !options.repository_set => {
                    options.repository = PathBuf::from(value);
                    options.repository_set = true;
                }
                value => return Err(format!("unexpected argument: {value}")),
            }
        }
        if options.github && options.tasks_from_flag {
            return Err("--tasks and --github are mutually exclusive".to_string());
        }
        options.repository = options
            .repository
            .canonicalize()
            .unwrap_or(options.repository);
        Ok(options)
    }

    fn task_file(&self) -> Result<Option<PathBuf>, String> {
        if self.github {
            return Ok(None);
        }
        if let Some(path) = &self.tasks {
            return path
                .canonicalize()
                .map(Some)
                .map_err(|_| format!("tasks file not found: {}", path.display()));
        }
        let detected = self.repository.join(".alucard/tasks.md");
        Ok(detected.is_file().then_some(detected))
    }
}

fn parse_db_path(args: &[String]) -> Result<(Option<PathBuf>, Vec<String>), String> {
    let mut path = None;
    let mut commands = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--db-path" {
            let value = args
                .next()
                .ok_or_else(|| "alucard: --db-path requires a path".to_string())?;
            path = Some(PathBuf::from(value));
        } else {
            commands.push(arg.clone());
        }
    }
    Ok((path, commands))
}

fn database_command(args: &[String], path: PathBuf) -> ExitCode {
    if args.len() != 1 {
        return fail("usage: alucard [--db-path PATH] db {migrate|status}");
    }
    let result = match args[0].as_str() {
        "migrate" => migrate(&path),
        "status" => status(&path),
        command => return fail(&format!("alucard: unknown db command: {command}")),
    };
    match result {
        Ok(status) => {
            println!("path: {}", path.display());
            println!("schema version: {}", status.schema_version);
            println!("pending migrations: {}", status.pending_migrations);
            if status.pending_migrations == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => fail(&format!("alucard: database error: {error}")),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("{message}");
    ExitCode::FAILURE
}

/// The checkout holding the prompts and Dockerfile: the nearest ancestor of the executable that has them.
fn tool_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .find(|d| {
            d.join("Dockerfile").is_file() && d.join("alucard-engineering-policy.md").is_file()
        })
        .map(PathBuf::from)
}

fn version() -> String {
    tool_dir()
        .and_then(|dir| {
            Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["describe", "--tags", "--always", "--dirty"])
                .output()
                .ok()
        })
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
