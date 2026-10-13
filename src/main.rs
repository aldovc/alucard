//! Alucard orchestrator: the Rust port of the `alucard` Bash script.

use alucard::{
    database_path, github_queue, github_ticket_blockers, import_github_tasks, import_local_tasks,
    local_queue_with_open_issues, migrate, open, parse_tasks, status, GithubPullRequest,
    GithubTask,
};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::{collections::BTreeMap, fs};

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
        Some("doctor") => doctor_command(&commands[1..], db_path),
        Some("queue") => queue_command(&commands[1..], db_path),
        Some(cmd) => fail(&format!("alucard: unknown command: {cmd}")),
        None => fail("usage: alucard [--db-path PATH] {version|queue|doctor|db migrate|db status}"),
    }
}

const REQUIRED_TOOL_FILES: &str = include_str!("../required-tool-files.txt");

fn nonempty_environment(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

struct DoctorOptions {
    repository: PathBuf,
    env_file: PathBuf,
    image: String,
    tasks: Option<PathBuf>,
    github: bool,
    issue: bool,
}

impl DoctorOptions {
    fn parse(args: &[String], tools: &std::path::Path) -> Result<Self, String> {
        let mut options = Self {
            repository: nonempty_environment("ALUCARD_TARGET_REPO")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().expect("current directory")),
            env_file: nonempty_environment("ALUCARD_ENV_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| tools.join("alucard.env")),
            image: nonempty_environment("ALUCARD_IMAGE")
                .and_then(|value| value.into_string().ok())
                .unwrap_or_else(|| "ghcr.io/aldovc/alucard:latest".to_string()),
            tasks: nonempty_environment("ALUCARD_TASKS_FILE").map(PathBuf::from),
            github: false,
            issue: false,
        };
        let mut repository_set = false;
        let mut tasks_from_flag = false;
        let mut values = args.iter();
        while let Some(value) = values.next() {
            match value.as_str() {
                "-C" | "--repo" => {
                    options.repository =
                        PathBuf::from(values.next().ok_or("--repo requires a path")?);
                    repository_set = true;
                }
                "--env-file" => {
                    options.env_file =
                        PathBuf::from(values.next().ok_or("--env-file requires a path")?)
                }
                "--image" => {
                    options.image = values.next().ok_or("--image requires a name")?.clone()
                }
                "-t" | "--timeout-minutes" => {
                    let timeout = values
                        .next()
                        .ok_or_else(|| format!("{value} requires a number"))?;
                    if timeout.is_empty() || !timeout.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err("timeout must be a positive integer".to_string());
                    }
                }
                "-n" | "--iterations" => {
                    let iterations = values
                        .next()
                        .ok_or_else(|| format!("{value} requires a number"))?;
                    if iterations.is_empty()
                        || !iterations.bytes().all(|byte| byte.is_ascii_digit())
                    {
                        return Err("iterations must be a positive integer".to_string());
                    }
                }
                "--base" => {
                    let base = values.next().ok_or("--base requires a branch name")?;
                    if base.is_empty() {
                        return Err("--base requires a branch name".to_string());
                    }
                }
                "--max-review-cycles" => {
                    let cycles = values
                        .next()
                        .ok_or("--max-review-cycles requires a number")?;
                    if cycles.is_empty() || !cycles.bytes().all(|byte| byte.is_ascii_digit()) {
                        return Err("max-review-cycles must be a positive integer".to_string());
                    }
                }
                "--no-build" => {}
                "--logs-root" => {
                    values
                        .next()
                        .ok_or_else(|| format!("{value} requires a path"))?;
                }
                "--tasks" => {
                    options.tasks = Some(PathBuf::from(
                        values.next().ok_or("--tasks requires a path")?,
                    ));
                    tasks_from_flag = true;
                }
                "--github" => options.github = true,
                "--issue" => {
                    let issue = values.next().ok_or("--issue requires a number")?;
                    if issue
                        .parse::<i64>()
                        .ok()
                        .filter(|number| *number > 0)
                        .is_none()
                    {
                        return Err("--issue requires a positive integer".to_string());
                    }
                    options.issue = true;
                }
                value if value.starts_with('-') => return Err(format!("unknown option: {value}")),
                value if !repository_set => {
                    options.repository = PathBuf::from(value);
                    repository_set = true;
                }
                value => return Err(format!("unexpected argument: {value}")),
            }
        }
        if options.issue {
            return Err("--issue is only valid with alucard run".to_string());
        }
        if tasks_from_flag && options.github {
            return Err("--tasks and --github are mutually exclusive".to_string());
        }
        if !options.github {
            if let Some(tasks) = &options.tasks {
                if !tasks.is_file() {
                    return Err(format!("tasks file not found: {}", tasks.display()));
                }
            }
        }
        Ok(options)
    }
}

fn doctor_command(args: &[String], db_path: Option<PathBuf>) -> ExitCode {
    let Some(tools) = tool_dir() else {
        return fail("alucard: could not locate tool files");
    };
    let options = match DoctorOptions::parse(args, &tools) {
        Ok(options) => options,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    let mut ok = true;
    for command in ["git", "gh", "jq", "docker", "timeout"] {
        if command_exists(command) {
            println!("ok: {command}");
        } else {
            println!("missing: {command}");
            ok = false;
        }
    }
    let missing_files = REQUIRED_TOOL_FILES.lines().any(|file| {
        if tools.join(file).is_file() {
            false
        } else {
            println!("missing: {}", tools.join(file).display());
            true
        }
    });
    if missing_files {
        ok = false;
    } else {
        println!("ok: tool files in {}", tools.display());
    }
    let environment = match read_environment(&options.env_file) {
        Ok(environment) => {
            println!("ok: env file {}", options.env_file.display());
            environment
        }
        Err(_) => {
            println!("missing: env file {}", options.env_file.display());
            ok = false;
            BTreeMap::new()
        }
    };
    if options.env_file.is_file() {
        if environment.get("GITHUB_TOKEN").is_none_or(String::is_empty) {
            println!("missing: GITHUB_TOKEN value");
            ok = false;
        }
        let providers = ["WORKER", "CI_FIX", "REVIEWER", "FEEDBACK"]
            .iter()
            .filter_map(|role| match resolve_provider(&environment, role) {
                Ok(provider) => Some(provider),
                Err(provider) => {
                    println!("missing: unrecognized provider '{provider}' for role {role}: must be 'claude' or 'codex'");
                    ok = false;
                    None
                }
            })
            .collect::<Vec<_>>();
        for (provider, key) in [("claude", "ANTHROPIC_API_KEY"), ("codex", "OPENAI_API_KEY")] {
            if providers.contains(&provider) {
                if environment.get(key).is_some_and(|value| !value.is_empty()) {
                    println!("ok: {key}");
                } else {
                    println!("missing: {key} value");
                    ok = false;
                }
            }
        }
    }
    let repository = options
        .repository
        .canonicalize()
        .ok()
        .filter(|path| path.is_dir())
        .and_then(|path| git_repository_root(&path));
    if let Some(repository) = repository.as_ref() {
        println!("ok: target repo {}", repository.display());
        if command_success("git", repository, &["config", "--get", "remote.origin.url"]) {
            println!("ok: target repo has origin remote");
        } else {
            println!("missing: target repo origin remote");
            ok = false;
        }
    } else if let Some(github_ref) = github_ref(&options.repository) {
        if command_success(
            "gh",
            &tools,
            &["repo", "view", &github_ref, "--json", "name"],
        ) {
            println!(
                "ok: target repo is GitHub ref {} (cache: {})",
                github_ref,
                cache_path(&github_ref).display()
            );
        } else {
            println!(
                "missing: cannot reach GitHub repo {github_ref} (check GITHUB_TOKEN and network)"
            );
            ok = false;
        }
    } else {
        println!(
            "missing: valid target git repo at {} (or a GitHub ref like owner/repo)",
            options.repository.display()
        );
        ok = false;
    }
    if command_success("docker", &tools, &["image", "inspect", &options.image]) {
        println!("ok: Docker image {}", options.image);
    } else {
        println!("missing: Docker image {} (run: docker pull ghcr.io/aldovc/alucard:latest, or alucard build for a local image)", options.image);
    }
    if !options.github {
        let task_file = options.tasks.or_else(|| {
            repository
                .map(|path| path.join(".alucard/tasks.md"))
                .filter(|path| path.is_file())
        });
        if let Some(task_file) = task_file {
            if command_exists("jq") {
                doctor_tasks(&task_file, &mut ok);
            } else {
                println!(
                    "skipped: tasks file validation ({}) — jq missing",
                    task_file.display()
                );
            }
        }
    }
    let path = match database_path(db_path) {
        Ok(path) => path,
        Err(error) => {
            println!("missing: ledger database ({error})");
            return ExitCode::FAILURE;
        }
    };
    match status(&path) {
        Ok(database) if database.pending_migrations == 0 && database.journal_mode == "wal" => {
            println!(
                "ok: ledger {} (WAL enabled, migrations current)",
                path.display()
            )
        }
        Ok(database) if database.pending_migrations != 0 => {
            println!("missing: ledger migrations (run: alucard db migrate)");
            ok = false;
        }
        Ok(database) => {
            println!("missing: ledger WAL mode is {}", database.journal_mode);
            ok = false;
        }
        Err(error) => {
            println!("missing: ledger database {} ({error})", path.display());
            ok = false;
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn command_exists(command: &str) -> bool {
    Command::new(command).arg("--version").output().is_ok()
}

fn command_success(command: &str, directory: &std::path::Path, args: &[&str]) -> bool {
    Command::new(command)
        .current_dir(directory)
        .args(args)
        .output()
        .is_ok_and(|output| output.status.success())
}

fn git_repository_root(directory: &std::path::Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .current_dir(directory)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    output.status.success().then(|| {
        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!root.is_empty()).then(|| PathBuf::from(root))
    })?
}

fn read_environment(path: &std::path::Path) -> Result<BTreeMap<String, String>, std::io::Error> {
    let mut environment: BTreeMap<String, String> = [
        "GITHUB_TOKEN",
        "ANTHROPIC_API_KEY",
        "OPENAI_API_KEY",
        "ALUCARD_PROVIDER",
        "ALUCARD_WORKER_PROVIDER",
        "ALUCARD_CI_FIX_PROVIDER",
        "ALUCARD_REVIEWER_PROVIDER",
        "ALUCARD_FEEDBACK_PROVIDER",
    ]
    .into_iter()
    .filter_map(|name| {
        std::env::var(name)
            .ok()
            .map(|value| (name.to_string(), value))
    })
    .collect();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = line.strip_prefix("export ").unwrap_or(line).split_once('=') {
            environment.insert(
                name.trim().to_string(),
                value.trim_matches('"').trim_matches('\'').to_string(),
            );
        }
    }
    Ok(environment)
}

fn resolve_provider<'a>(
    environment: &'a BTreeMap<String, String>,
    role: &str,
) -> Result<&'a str, &'a str> {
    let provider = environment
        .get(&format!("ALUCARD_{role}_PROVIDER"))
        .filter(|value| !value.is_empty())
        .or_else(|| {
            environment
                .get("ALUCARD_PROVIDER")
                .filter(|value| !value.is_empty())
        })
        .map_or("claude", String::as_str);
    match provider {
        "claude" | "codex" => Ok(provider),
        _ => Err(provider),
    }
}

fn github_ref(path: &std::path::Path) -> Option<String> {
    let value = path
        .to_str()?
        .strip_suffix(".git")
        .unwrap_or(path.to_str()?);
    let value = value
        .strip_prefix("git@github.com:")
        .or_else(|| value.strip_prefix("https://github.com/"))
        .unwrap_or(value);
    let (owner, name) = value.split_once('/')?;
    if name.contains('/') || owner.is_empty() || name.is_empty() {
        return None;
    }
    [owner, name]
        .iter()
        .all(|part| {
            part.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        })
        .then(|| format!("{owner}/{name}"))
}

fn cache_path(repository: &str) -> PathBuf {
    nonempty_environment("ALUCARD_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CACHE_HOME").map(|path| PathBuf::from(path).join("alucard"))
        })
        .or_else(|| std::env::var_os("HOME").map(|path| PathBuf::from(path).join(".cache/alucard")))
        .unwrap_or_default()
        .join(repository)
}

fn doctor_tasks(path: &std::path::Path, ok: &mut bool) {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(_) => return,
    };
    let parsed = match parse_tasks(&contents) {
        Ok(parsed) => parsed,
        Err(problems) => {
            println!("invalid: tasks file {}", path.display());
            for problem in problems {
                println!("  - {problem}");
            }
            *ok = false;
            return;
        }
    };
    let mut problems = Vec::new();
    let mut seen = BTreeMap::new();
    for task in &parsed.tasks {
        if let Some(first) = seen.insert(&task.id, task.line) {
            problems.push(format!(
                "line {}: duplicate task id {} (first used at line {first})",
                task.line, task.id
            ));
        }
        for blocker in &task.blockers {
            if !parsed
                .tasks
                .iter()
                .any(|candidate| candidate.id == *blocker)
            {
                problems.push(format!(
                    "line {}: task {} blocked by unknown task id {blocker}",
                    task.line, task.id
                ));
            }
        }
    }
    if parsed.parent_context.split_whitespace().next().is_none() {
        problems.push(
            "line 1: empty header — no parent context above the first task heading".to_string(),
        );
    }
    if parsed.tasks.is_empty() {
        problems.push("line 1: no task headings found".to_string());
    }
    if problems.is_empty() {
        let todo = parsed
            .tasks
            .iter()
            .filter(|task| task.state == alucard::TaskState::Pending)
            .count();
        println!(
            "ok: tasks file {} (local task source, {todo} todo)",
            path.display()
        );
    } else {
        println!("invalid: tasks file {}", path.display());
        for problem in problems {
            println!("  - {problem}");
        }
        *ok = false;
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
            repository: nonempty_environment("ALUCARD_TARGET_REPO")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().expect("current directory")),
            tasks: nonempty_environment("ALUCARD_TASKS_FILE").map(PathBuf::from),
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
