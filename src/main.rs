//! Alucard orchestrator: the Rust port of the `alucard` Bash script.

use alucard::{
    database_path, find_run, github_queue, github_ticket_blockers, import_github_tasks,
    import_local_tasks, local_queue_with_open_issues, migrate, open, parse_tasks, recent_runs,
    run_claims, run_command as execute_command, run_pull_requests, status, GithubPullRequest,
    GithubTask,
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
        Some("runs") => match database_path(db_path) {
            Ok(path) => runs_command(&commands[1..], &path),
            Err(error) => fail(&format!("alucard: {error}")),
        },
        Some(cmd) => fail(&format!("alucard: unknown command: {cmd}")),
        None => fail("usage: alucard [--db-path PATH] {version|queue|runs|db migrate|db status}"),
    }
}

fn queue_command(args: &[String], db_path: Option<PathBuf>) -> ExitCode {
    let options = match QueueOptions::parse(args) {
        Ok(options) => options,
        Err(message) => return fail(&format!("alucard: {message}")),
    };
    if options.issue.is_some() {
        return fail("alucard: --issue is only valid with alucard run");
    }
    let task_file = match options.task_file() {
        Ok(Some(path)) => path,
        Ok(None) => return github_queue_command(&options, db_path),
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

fn github_queue_command(options: &QueueOptions, db_path: Option<PathBuf>) -> ExitCode {
    let issues = match fetch_github_issues(&options.repository) {
        Ok(issues) => issues,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    let needs_blockers = issues
        .iter()
        .any(|issue| !github_ticket_blockers(&issue.body).is_empty());
    let open_issues = if needs_blockers {
        match fetch_open_issue_numbers_required(&options.repository) {
            Ok(numbers) => numbers,
            Err(error) => return fail(&format!("alucard: {error}")),
        }
    } else {
        Vec::new()
    };
    let open_prs = match fetch_open_prs(&options.repository) {
        Ok(prs) => prs,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    let imported_tasks = issues.clone();
    let queue = github_queue(issues, &open_issues, &open_prs);
    let path = match database_path(db_path) {
        Ok(path) => path,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    if let Err(error) = migrate(&path).and_then(|_| {
        let mut connection = open(&path)?;
        import_github_tasks(
            &mut connection,
            &options.repository.display().to_string(),
            &imported_tasks,
        )
    }) {
        return fail(&format!("alucard: database error: {error}"));
    }
    match serde_json::to_string_pretty(&queue) {
        Ok(queue) => {
            println!("{queue}");
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("alucard: cannot serialize queue: {error}")),
    }
}

fn gh_output(repository: &PathBuf, args: &[&str], context: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("gh")
        .current_dir(repository)
        .args(args)
        .output()
        .map_err(|error| format!("{context}: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if error.is_empty() {
            context.to_string()
        } else {
            format!("{context}: {error}")
        })
    }
}

fn fetch_github_issues(repository: &PathBuf) -> Result<Vec<GithubTask>, String> {
    serde_json::from_slice(&gh_output(
        repository,
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--label",
            "ready-for-agent",
            "--limit",
            "500",
            "--json",
            "number,title,body,labels",
        ],
        "could not list GitHub issues",
    )?)
    .map_err(|error| format!("could not parse GitHub issues: {error}"))
}

fn fetch_open_issue_numbers_required(repository: &PathBuf) -> Result<Vec<i64>, String> {
    let values: Vec<serde_json::Value> = serde_json::from_slice(&gh_output(
        repository,
        &[
            "issue", "list", "--state", "open", "--json", "number", "--limit", "1000",
        ],
        "could not list open issues while checking blockers",
    )?)
    .map_err(|error| format!("could not parse open issues: {error}"))?;
    values
        .into_iter()
        .map(|issue| {
            issue
                .get("number")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| "could not parse open issue number".to_string())
        })
        .collect()
}

fn fetch_open_prs(repository: &PathBuf) -> Result<Vec<GithubPullRequest>, String> {
    let owner = String::from_utf8(gh_output(
        repository,
        &[
            "repo",
            "view",
            "--json",
            "nameWithOwner",
            "--jq",
            ".nameWithOwner",
        ],
        "could not list open pull requests",
    )?)
    .map_err(|error| format!("could not parse repository name: {error}"))?;
    let owner = owner.trim();
    let (owner, name) = owner
        .split_once('/')
        .ok_or_else(|| "could not list open pull requests".to_string())?;
    let query = "query($owner:String!,$repo:String!){repository(owner:$owner,name:$repo){pullRequests(states:OPEN,first:100){nodes{number,title,body,closingIssuesReferences(first:25){nodes{number}}}}}}}";
    let output = gh_output(
        repository,
        &[
            "api",
            "graphql",
            "-f",
            &format!("query={query}"),
            "-f",
            &format!("owner={owner}"),
            "-f",
            &format!("repo={name}"),
            "--jq",
            ".data.repository.pullRequests.nodes",
        ],
        "could not list open pull requests",
    )?;
    serde_json::from_slice(&output)
        .map_err(|error| format!("could not parse open pull requests: {error}"))
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
    issue: Option<i64>,
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
            issue: None,
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
                "--issue" => {
                    let value = args.next().ok_or("--issue requires a number")?;
                    let number = value
                        .parse::<i64>()
                        .ok()
                        .filter(|number| *number > 0)
                        .ok_or("--issue requires a positive integer")?;
                    options.issue = Some(number);
                }
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
        if options.tasks_from_flag && (options.github || options.issue.is_some()) {
            return Err(if options.issue.is_some() {
                "--tasks and --issue are mutually exclusive"
            } else {
                "--tasks and --github are mutually exclusive"
            }
            .to_string());
        }
        options.repository = options
            .repository
            .canonicalize()
            .unwrap_or(options.repository);
        Ok(options)
    }

    fn task_file(&self) -> Result<Option<PathBuf>, String> {
        if self.github || self.issue.is_some() {
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

fn runs_command(args: &[String], path: &std::path::Path) -> ExitCode {
    if args.is_empty() {
        return match recent_runs(path) {
            Ok(runs) => {
                for run in runs {
                    println!(
                        "{}\t{}\t{}\t{}",
                        run.id,
                        run.status,
                        run.command,
                        run.logs_path.unwrap_or_default()
                    );
                }
                ExitCode::SUCCESS
            }
            Err(error) => fail(&format!("alucard: database error: {error}")),
        };
    }
    if args.len() == 2 && args[0] == "show" {
        return match find_run(path, &args[1]) {
            Ok(Some(run)) => {
                println!("id: {}", run.id);
                println!("repository: {}", run.repository_id);
                println!("command: {}", run.command);
                println!("status: {}", run.status);
                println!("base sha: {}", run.base_sha.unwrap_or_default());
                println!("image: {}", run.image.unwrap_or_default());
                println!("logs: {}", run.logs_path.unwrap_or_default());
                let claims = match run_claims(path, &run.id) {
                    Ok(claims) => claims,
                    Err(error) => return fail(&format!("alucard: database error: {error}")),
                };
                for (kind, key, state) in claims {
                    println!("claim: {kind} {key} {state}");
                }
                let pull_requests = match run_pull_requests(path, &run.id) {
                    Ok(pull_requests) => pull_requests,
                    Err(error) => return fail(&format!("alucard: database error: {error}")),
                };
                for (number, branch) in pull_requests {
                    println!("pr: #{number} {branch}");
                }
                ExitCode::SUCCESS
            }
            Ok(None) => fail(&format!("alucard: run not found: {}", args[1])),
            Err(error) => fail(&format!("alucard: database error: {error}")),
        };
    }
    fail("usage: alucard [--db-path PATH] runs [show RUN_ID]")
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
            execute_command(
                "git",
                &["describe", "--tags", "--always", "--dirty"],
                &dir,
                None,
            )
            .ok()
        })
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}
