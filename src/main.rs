//! Alucard orchestrator: the Rust port of the `alucard` Bash script.

use alucard::{
    begin_operation, build_image, compose_role_prompt, database_path, find_run, finish_operation,
    finish_run, github_queue, github_ticket_blockers, import_github_tasks, import_local_tasks,
    local_queue_with_open_issues, migrate, open, parse_tasks, prepare_run, recent_runs,
    record_recovery_failure, record_recovery_pr, record_task_pr, release, run_claims,
    run_command as execute_command, run_pull_requests, run_recovery, start_retry_run,
    start_task_run, status, task_id, task_is_pending, toolchain_preflight, transition_task,
    warn_if_stale_image, AgentInvocation, AgentRole, AgentSettings, GithubPullRequest, GithubTask,
    Provider, RecoveryPullRequest, RunOptions, TaskStatus,
};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
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
        Some("run") => run_command(&commands[1..], db_path),
        Some("build") => build_command(&commands[1..]),
        Some("runs") => match database_path(db_path) {
            Ok(path) => runs_command(&commands[1..], &path),
            Err(error) => fail(&format!("alucard: {error}")),
        },
        Some(cmd) => fail(&format!("alucard: unknown command: {cmd}")),
        None => {
            fail("usage: alucard [--db-path PATH] {version|build|queue|runs|doctor|db migrate|db status}")
        }
    }
}

fn build_command(args: &[String]) -> ExitCode {
    let mut image = nonempty_environment("ALUCARD_IMAGE")
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "ghcr.io/aldovc/alucard:latest".to_string());
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--image" => match args.next() {
                Some(value) if !value.is_empty() => image = value.clone(),
                None => return fail("alucard: --image requires an image name"),
                Some(_) => return fail("alucard: --image requires an image name"),
            },
            "-h" | "--help" => {
                println!("usage: alucard build [--image IMAGE]");
                return ExitCode::SUCCESS;
            }
            _ => return fail(&format!("alucard: unknown build option: {arg}")),
        }
    }
    let Some(dir) = tool_dir().or_else(|| std::env::current_dir().ok()) else {
        return fail("alucard: cannot find tool directory");
    };
    match build_image(&dir, &image) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => fail(&format!("alucard: {error}")),
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
    let settings = match AgentSettings::from_environment(Some(&options.env_file)) {
        Ok(settings) => {
            println!("ok: env file {}", options.env_file.display());
            Some(settings)
        }
        Err(_) => {
            println!("missing: env file {}", options.env_file.display());
            ok = false;
            None
        }
    };
    if let Some(settings) = settings {
        if !settings.has_value("GITHUB_TOKEN") {
            println!("missing: GITHUB_TOKEN value");
            ok = false;
        }
        let providers = [
            ("WORKER", AgentRole::Worker),
            ("CI_FIX", AgentRole::CiFix),
            ("REVIEWER", AgentRole::Reviewer),
            ("FEEDBACK", AgentRole::Feedback),
        ]
        .into_iter()
            .filter_map(|(role_name, role)| match settings.provider(role) {
                Ok(provider) => Some(provider),
                Err(_) => {
                    println!("missing: unrecognized provider '{}' for role {role_name}: must be 'claude' or 'codex'", settings.provider_name(role));
                    ok = false;
                    None
                }
            })
            .collect::<Vec<_>>();
        for (provider, key) in [
            (Provider::Claude, "ANTHROPIC_API_KEY"),
            (Provider::Codex, "OPENAI_API_KEY"),
        ] {
            if providers.contains(&provider) {
                if settings.has_value(key) {
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

struct RunCommandOptions {
    queue: QueueOptions,
    iterations: usize,
    timeout_minutes: u64,
    base: String,
    image: String,
    no_build: bool,
    logs_root: PathBuf,
    env_file: PathBuf,
    transport_attempts: usize,
}

impl RunCommandOptions {
    fn parse(args: &[String], tools: &Path) -> Result<Self, String> {
        let mut queue_args = Vec::new();
        let mut iterations = 20;
        let mut iterations_from_flag = false;
        let mut timeout_minutes = 30;
        let mut base = nonempty_environment("ALUCARD_BASE_BRANCH")
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| "main".to_string());
        let mut image = nonempty_environment("ALUCARD_IMAGE")
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| "ghcr.io/aldovc/alucard:latest".to_string());
        let mut no_build = false;
        let mut logs_root = nonempty_environment("ALUCARD_LOG_ROOT")
            .map(PathBuf::from)
            .or_else(|| {
                nonempty_environment("XDG_STATE_HOME")
                    .map(|root| PathBuf::from(root).join("alucard/logs"))
            })
            .or_else(|| {
                nonempty_environment("HOME")
                    .map(|root| PathBuf::from(root).join(".local/state/alucard/logs"))
            })
            .ok_or("log root is not set; use --logs-root or ALUCARD_LOG_ROOT")?;
        let mut env_file = nonempty_environment("ALUCARD_ENV_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| tools.join("alucard.env"));
        let mut values = args.iter();
        while let Some(value) = values.next() {
            match value.as_str() {
                "-n" | "--iterations" => {
                    iterations_from_flag = true;
                    iterations = values
                        .next()
                        .ok_or("--iterations requires a number")?
                        .parse()
                        .ok()
                        .filter(|value: &usize| *value > 0)
                        .ok_or("iterations must be greater than zero")?
                }
                "-t" | "--timeout-minutes" => {
                    timeout_minutes = values
                        .next()
                        .ok_or("--timeout-minutes requires a number")?
                        .parse()
                        .ok()
                        .filter(|value: &u64| *value > 0)
                        .ok_or("timeout must be greater than zero")?
                }
                "--base" => base = values.next().ok_or("--base requires a branch")?.clone(),
                "--image" => {
                    image = values
                        .next()
                        .ok_or("--image requires an image name")?
                        .clone()
                }
                "--no-build" => no_build = true,
                "--logs-root" => {
                    logs_root = PathBuf::from(values.next().ok_or("--logs-root requires a path")?)
                }
                "--env-file" => {
                    env_file = PathBuf::from(values.next().ok_or("--env-file requires a path")?)
                }
                _ => queue_args.push(value.clone()),
            }
        }
        let queue = QueueOptions::parse(&queue_args)?;
        if queue.issue.is_some() {
            if iterations_from_flag && iterations != 1 {
                return Err("--issue runs exactly one iteration".to_string());
            }
            iterations = 1;
        }
        let transport_attempts = nonempty_environment("ALUCARD_TRANSPORT_RETRY_ATTEMPTS")
            .and_then(|value| value.into_string().ok())
            .map(|value| {
                value
                    .parse::<usize>()
                    .map_err(|_| "ALUCARD_TRANSPORT_RETRY_ATTEMPTS must be a non-negative integer")
            })
            .transpose()?
            .unwrap_or(2)
            .saturating_add(1);
        Ok(Self {
            queue,
            iterations,
            timeout_minutes,
            base,
            image,
            no_build,
            logs_root,
            env_file,
            transport_attempts,
        })
    }
}

fn run_command(args: &[String], db_path: Option<PathBuf>) -> ExitCode {
    let Some(tools) = tool_dir() else {
        return fail("alucard: could not locate tool files");
    };
    let options = match RunCommandOptions::parse(args, &tools) {
        Ok(options) => options,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    let database = match database_path(db_path) {
        Ok(path) => path,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    if !options.env_file.is_file() {
        return fail(&format!(
            "alucard: env file not found: {}",
            options.env_file.display()
        ));
    }
    let settings = match AgentSettings::from_environment(Some(&options.env_file)) {
        Ok(settings) => settings,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    let cache = cache_root();
    let playwright_cache = fs::create_dir_all(cache.join("playwright-browsers"))
        .ok()
        .map(|()| cache.join("playwright-browsers"));
    let github_repo = github_repo_slug(&options.queue.repository);
    if !options.no_build {
        if let Err(error) = build_image(&tools, &options.image) {
            return fail(&format!("alucard: {error}"));
        }
    } else if let Err(error) = warn_if_stale_image(&tools, &options.image) {
        return fail(&format!("alucard: {error}"));
    }
    let task_file = match options.queue.task_file() {
        Ok(file) => file,
        Err(error) => return fail(&format!("alucard: {error}")),
    };
    let mut iteration = 1;
    while iteration <= options.iterations {
        let selected = match select_run_task(&options.queue, task_file.as_deref(), &database) {
            Ok(Some(task)) => task,
            Ok(None) => break,
            Err(error) => return fail(&format!("alucard: {error}")),
        };
        let (run, claim) = match start_task_run(
            &database,
            &options.queue.repository,
            &options.logs_root,
            &cache,
            Some(&options.image),
            &selected.ledger_id,
        ) {
            Ok(run) => run,
            Err(alucard::ClaimError::ClaimedBy { .. }) => continue,
            Err(error) => return fail(&format!("alucard: {error}")),
        };
        let owner_run_id = run.id.clone();
        let mut attempt_run = run;
        let mut attempt = 1;
        let mut prepared =
            match prepare_attempt(&database, &options, &cache, &selected, &attempt_run) {
                Ok(prepared) => prepared,
                Err(error) => {
                    release_without_pr(
                        &database,
                        &selected,
                        &claim.id,
                        &owner_run_id,
                        &options.queue.repository,
                    );
                    let _ = finish_run(&database, &attempt_run.id, "failed");
                    return fail(&format!("alucard: {error}"));
                }
            };
        if let Some(issue) = selected.issue.as_ref() {
            if let Err(error) = gh_output(
                &options.queue.repository,
                &[
                    "issue",
                    "edit",
                    &issue.number.to_string(),
                    "--add-label",
                    "in-progress",
                ],
                "could not label GitHub issue",
            ) {
                let _ = transition_task(
                    &database,
                    &selected.ledger_id,
                    &claim.id,
                    &owner_run_id,
                    TaskStatus::Pending,
                );
                let _ = release(&database, &claim.id, &owner_run_id);
                let _ = finish_run(&database, &attempt_run.id, "failed");
                return fail(&format!("alucard: {error}"));
            }
        }
        let commits = match recent_base_commits(&prepared.clone, &options.base) {
            Ok(commits) => commits,
            Err(error) => {
                release_without_pr(
                    &database,
                    &selected,
                    &claim.id,
                    &owner_run_id,
                    &options.queue.repository,
                );
                let _ = finish_run(&database, &attempt_run.id, "failed");
                return fail(&format!("alucard: {error}"));
            }
        };
        let prompt = match worker_prompt(
            &tools,
            &selected,
            &options.base,
            &commits,
            &toolchain_preflight(
                &options.queue.repository,
                &options.base,
                &options.image,
                &options.env_file,
                options.timeout_minutes,
            )
            .unwrap_or_else(|error| format!("Preflight unavailable: {error}")),
        ) {
            Ok(prompt) => prompt,
            Err(error) => {
                release_without_pr(
                    &database,
                    &selected,
                    &claim.id,
                    &owner_run_id,
                    &options.queue.repository,
                );
                let _ = finish_run(&database, &attempt_run.id, "failed");
                return fail(&format!("alucard: {error}"));
            }
        };
        loop {
            let name = format!("alucard-worker-{}-{attempt}", attempt_run.id);
            let log = prepared
                .directory
                .join(format!("iter-{iteration}-attempt-{attempt}.jsonl"));
            let result = AgentInvocation {
                role: AgentRole::Worker,
                name: &name,
                image: &options.image,
                env_file: &options.env_file,
                worktree: &prepared.clone,
                prompt: &prompt,
                timeout_minutes: options.timeout_minutes,
                network: None,
                playwright_cache: playwright_cache.as_deref(),
                database_url: None,
                github_repo: github_repo.as_deref(),
                worktree_read_only: false,
                output_dir: None,
                extra_env: &[],
            }
            .invoke(&settings, &log);
            let classification = result
                .as_ref()
                .map(|result| result.classification)
                .unwrap_or(alucard::AgentExit::Failed);
            match find_attempt_pr(&options.queue.repository, &prepared) {
                Ok(Some((number, branch, sha))) => {
                    if let Err(error) = record_task_pr(
                        &database,
                        &selected.ledger_id,
                        &claim.id,
                        &owner_run_id,
                        number,
                        &branch,
                        &sha,
                    ) {
                        let _ = transition_task(
                            &database,
                            &selected.ledger_id,
                            &claim.id,
                            &owner_run_id,
                            TaskStatus::InFlight,
                        );
                        let _ = release(&database, &claim.id, &owner_run_id);
                        let _ = finish_run(&database, &attempt_run.id, "failed");
                        return fail(&format!("alucard: {error}"));
                    }
                    let _ = release(&database, &claim.id, &owner_run_id);
                    let _ = finish_run(&database, &attempt_run.id, "completed");
                    break;
                }
                Ok(None) => {
                    if classification == alucard::AgentExit::Billing {
                        release_without_pr(
                            &database,
                            &selected,
                            &claim.id,
                            &owner_run_id,
                            &options.queue.repository,
                        );
                        let _ = finish_run(&database, &attempt_run.id, "failed");
                        return fail(
                            "alucard: provider returned billing_error (credit balance too low)",
                        );
                    }
                    if classification == alucard::AgentExit::Transport
                        && attempt < options.transport_attempts
                    {
                        let _ = finish_run(&database, &attempt_run.id, "failed");
                        attempt += 1;
                        attempt_run = match start_retry_run(
                            &database,
                            &options.queue.repository,
                            &options.logs_root,
                            &cache,
                            Some(&options.image),
                        ) {
                            Ok(run) => run,
                            Err(error) => {
                                release_without_pr(
                                    &database,
                                    &selected,
                                    &claim.id,
                                    &owner_run_id,
                                    &options.queue.repository,
                                );
                                return fail(&format!("alucard: {error}"));
                            }
                        };
                        prepared = match prepare_attempt(
                            &database,
                            &options,
                            &cache,
                            &selected,
                            &attempt_run,
                        ) {
                            Ok(prepared) => prepared,
                            Err(error) => {
                                release_without_pr(
                                    &database,
                                    &selected,
                                    &claim.id,
                                    &owner_run_id,
                                    &options.queue.repository,
                                );
                                let _ = finish_run(&database, &attempt_run.id, "failed");
                                return fail(&format!("alucard: {error}"));
                            }
                        };
                        continue;
                    }
                    if let Err(error) = recover_worker_changes(
                        &database,
                        &options,
                        &selected,
                        &claim.id,
                        &owner_run_id,
                        &prepared,
                        classification,
                        result.as_ref().ok().and_then(|value| value.status),
                    ) {
                        eprintln!("alucard: recovery requires human attention: {error}");
                        let _ = transition_task(
                            &database,
                            &selected.ledger_id,
                            &claim.id,
                            &owner_run_id,
                            TaskStatus::Human,
                        );
                    }
                    let _ = release(&database, &claim.id, &owner_run_id);
                    if let Some(issue) = selected.issue.as_ref() {
                        let _ = gh_output(
                            &options.queue.repository,
                            &[
                                "issue",
                                "edit",
                                &issue.number.to_string(),
                                "--remove-label",
                                "in-progress",
                            ],
                            "could not remove GitHub issue label",
                        );
                    }
                    let _ = finish_run(
                        &database,
                        &attempt_run.id,
                        if result.is_ok() {
                            "completed"
                        } else {
                            "failed"
                        },
                    );
                    break;
                }
                Err(error) => {
                    release_without_pr(
                        &database,
                        &selected,
                        &claim.id,
                        &owner_run_id,
                        &options.queue.repository,
                    );
                    let _ = finish_run(&database, &attempt_run.id, "failed");
                    return fail(&format!("alucard: {error}"));
                }
            }
        }
        iteration += 1;
    }
    ExitCode::SUCCESS
}

#[allow(clippy::too_many_arguments)]
fn recover_worker_changes(
    database: &Path,
    options: &RunCommandOptions,
    task: &SelectedRunTask,
    lease_id: &str,
    run_id: &str,
    prepared: &alucard::PreparedRun,
    classification: alucard::AgentExit,
    status: Option<i32>,
) -> Result<(), String> {
    if classification == alucard::AgentExit::Clean
        || (classification == alucard::AgentExit::Transport && status == Some(0))
    {
        return release_pending(database, task, lease_id, run_id, &options.queue.repository);
    }
    let base = prepared.run.base_sha.as_deref().unwrap_or("HEAD");
    let commits = execute_command(
        "git",
        &["rev-list", "--count", &format!("{base}..HEAD")],
        &prepared.clone,
        None,
    )
    .map_err(|error| error.to_string())?;
    let commit_count: usize = String::from_utf8_lossy(&commits.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    let dirty = execute_command("git", &["status", "--porcelain"], &prepared.clone, None)
        .map_err(|error| error.to_string())?;
    let dirty = !dirty.stdout.is_empty();
    if !dirty && commit_count == 0 {
        if classification == alucard::AgentExit::Transport {
            return Ok(());
        }
        return release_pending(database, task, lease_id, run_id, &options.queue.repository);
    }
    if dirty {
        let add = execute_command("git", &["add", "-A"], &prepared.clone, None)
            .map_err(|error| error.to_string())?;
        if !add.status.success() {
            return Err("could not stage worker changes".to_string());
        }
        let commit = execute_command(
            "git",
            &[
                "commit",
                "-m",
                "wip: alucard recovery — uncommitted worker state",
            ],
            &prepared.clone,
            None,
        )
        .map_err(|error| error.to_string())?;
        if !commit.status.success() {
            return Err(String::from_utf8_lossy(&commit.stderr).trim().to_string());
        }
    }
    let target = format!("recovery:{}", prepared.branch);
    let push = begin_operation(database, &prepared.run.repository_id, &target, "push")
        .map_err(|error| error.to_string())?;
    let pushed = execute_command(
        "git",
        &["push", "-u", "origin", &prepared.branch],
        &prepared.clone,
        None,
    )
    .map_err(|error| error.to_string())?;
    if !pushed.status.success() {
        let error = String::from_utf8_lossy(&pushed.stderr).trim().to_string();
        let _ = finish_operation(database, &push, Some(&error));
        record_recovery_failure(
            database,
            &task.ledger_id,
            run_id,
            &prepared.branch,
            &prepared.clone,
            &error,
        )
        .map_err(|error| error.to_string())?;
        return Err(format!(
            "push failed; checkout retained at {}: {error}",
            prepared.clone.display()
        ));
    }
    finish_operation(database, &push, None).map_err(|error| error.to_string())?;
    if classification == alucard::AgentExit::Transport && commit_count == 0 {
        return release_pending(database, task, lease_id, run_id, &options.queue.repository);
    }
    if let Some((number, branch, sha)) = find_open_pr(&options.queue.repository, &prepared.branch)?
    {
        return record_recovery(
            database,
            task,
            lease_id,
            run_id,
            prepared,
            number,
            &branch,
            &sha,
            classification,
            status,
        );
    }
    let title = format!(
        "wip: alucard recovery — worker stopped: {:?}",
        classification
    )
    .to_lowercase();
    let body = recovery_body(task, classification, status, commit_count);
    let pr = begin_operation(
        database,
        &prepared.run.repository_id,
        &target,
        "create_recovery_pr",
    )
    .map_err(|error| error.to_string())?;
    let output = execute_command(
        "gh",
        &[
            "pr",
            "create",
            "--head",
            &prepared.branch,
            "--base",
            &options.base,
            "--label",
            "alucard",
            "--draft",
            "--title",
            &title,
            "--body",
            &body,
        ],
        &options.queue.repository,
        None,
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let _ = finish_operation(database, &pr, Some(&error));
        record_recovery_failure(
            database,
            &task.ledger_id,
            run_id,
            &prepared.branch,
            &prepared.clone,
            &error,
        )
        .map_err(|error| error.to_string())?;
        return Err(format!(
            "recovery PR creation failed; checkout retained at {}: {error}",
            prepared.clone.display()
        ));
    }
    finish_operation(database, &pr, None).map_err(|error| error.to_string())?;
    let (number, branch, sha) = find_open_pr(&options.queue.repository, &prepared.branch)?
        .ok_or("recovery PR was created but cannot be discovered")?;
    record_recovery(
        database,
        task,
        lease_id,
        run_id,
        prepared,
        number,
        &branch,
        &sha,
        classification,
        status,
    )
}

fn release_pending(
    database: &Path,
    task: &SelectedRunTask,
    lease_id: &str,
    run_id: &str,
    repository: &Path,
) -> Result<(), String> {
    transition_task(
        database,
        &task.ledger_id,
        lease_id,
        run_id,
        TaskStatus::Pending,
    )
    .map_err(|error| error.to_string())?;
    if let Some(issue) = &task.issue {
        let _ = gh_output(
            repository,
            &[
                "issue",
                "edit",
                &issue.number.to_string(),
                "--remove-label",
                "in-progress",
            ],
            "could not remove GitHub issue label",
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn record_recovery(
    database: &Path,
    task: &SelectedRunTask,
    lease_id: &str,
    run_id: &str,
    prepared: &alucard::PreparedRun,
    number: i64,
    branch: &str,
    sha: &str,
    classification: alucard::AgentExit,
    status: Option<i32>,
) -> Result<(), String> {
    record_recovery_pr(
        database,
        &task.ledger_id,
        lease_id,
        run_id,
        RecoveryPullRequest {
            number,
            branch,
            head_sha: sha,
            reason: &format!("{classification:?}"),
            checkout_path: &prepared.clone,
        },
    )
    .map_err(|error| error.to_string())?;
    let target = format!("recovery-pr:{number}");
    let marker = begin_operation(
        database,
        &prepared.run.repository_id,
        &target,
        "recovery_marker",
    )
    .map_err(|error| error.to_string())?;
    let marker_body = "**🤖 Alucard recovery PR parked** — opened as a draft and labeled `needs-human` by the harness.";
    let comment = gh_output(
        &prepared.clone,
        &["pr", "comment", &number.to_string(), "--body", marker_body],
        "could not post recovery marker",
    );
    let marker_error = comment.err();
    finish_operation(database, &marker, marker_error.as_deref())
        .map_err(|error| error.to_string())?;
    let label = begin_operation(
        database,
        &prepared.run.repository_id,
        &target,
        "needs-human-label",
    )
    .map_err(|error| error.to_string())?;
    let labelled = gh_output(
        &prepared.clone,
        &[
            "pr",
            "edit",
            &number.to_string(),
            "--add-label",
            "needs-human",
        ],
        "could not label recovery PR",
    );
    let label_error = labelled.err();
    finish_operation(database, &label, label_error.as_deref())
        .map_err(|error| error.to_string())?;
    if marker_error.is_some() || label_error.is_some() {
        return Err([marker_error, label_error]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("; "));
    }
    if let Some(issue) = &task.issue {
        let comment = format!("**🤖 Alucard recovery** — the worker on this ticket stopped ({classification:?}, rc={}). Its unfinished work is on draft PR #{number}.", status.unwrap_or(1));
        let _ = gh_output(
            &prepared.clone,
            &[
                "issue",
                "comment",
                &issue.number.to_string(),
                "--body",
                &comment,
            ],
            "could not comment on recovery issue",
        );
        let _ = gh_output(
            &prepared.clone,
            &[
                "issue",
                "edit",
                &issue.number.to_string(),
                "--remove-label",
                "in-progress",
            ],
            "could not remove GitHub issue label",
        );
    }
    Ok(())
}

fn recovery_body(
    task: &SelectedRunTask,
    classification: alucard::AgentExit,
    status: Option<i32>,
    commits: usize,
) -> String {
    let (reference, finish, handback) = match (&task.issue, &task.local_id) {
        (Some(issue), _) => (
            format!("Refs #{}", issue.number),
            "when every acceptance criterion is met, change the `Refs` line above to `Closes` so the ticket closes on merge.",
            "close this PR and the ticket rejoins the queue on the next run — its `in-progress` label is already off, and this attempt is noted in its comments. `alucard run --issue <N>` starts a fresh attempt on it right away.",
        ),
        (None, Some(id)) => (
            format!("Task: {id}"),
            "the harness flips the task to `[x]` when this PR merges.",
            "close this PR and the task flips back to `[ ]` at the next queue build, with this attempt noted in its heading.",
        ),
        (None, None) => (String::new(), "", ""),
    };
    let stop = match classification {
        alucard::AgentExit::Exhausted | alucard::AgentExit::Timeout => {
            "ran out of turns or hit its time limit"
        }
        alucard::AgentExit::Wedged => {
            "wedged its shell (every tool call failed after /work went missing)"
        }
        alucard::AgentExit::Transport => "lost its provider connection",
        alucard::AgentExit::NoWorktree => {
            "never started: its checkout was empty when the container mounted it"
        }
        _ => "failed",
    };
    let reference = if reference.is_empty() {
        String::new()
    } else {
        format!("{reference}\n\n")
    };
    format!(
        "{reference}**🤖 Alucard recovery PR** — the worker {stop} (rc={}) before opening a PR. Its {commits} commit(s), plus whatever was still uncommitted, are pushed here so the work isn't lost. Nothing on this branch has been through CI or review, and the last commit may be mid-edit.\n\nThis is not a finished change. What each next step does:\n\n- **Finish it here** — {finish}\n- **Hand it back** — {handback}\n- **`alucard continue <PR_NUM>`** runs the CI and review gates on what is here. It does not resume implementation.",
        status.unwrap_or(1)
    )
}

fn release_without_pr(
    database: &Path,
    task: &SelectedRunTask,
    lease_id: &str,
    run_id: &str,
    repository: &Path,
) {
    if let Err(error) = transition_task(
        database,
        &task.ledger_id,
        lease_id,
        run_id,
        TaskStatus::Pending,
    ) {
        eprintln!("alucard: could not return task to pending: {error}");
    }
    if let Err(error) = release(database, lease_id, run_id) {
        eprintln!("alucard: could not release task claim: {error}");
    }
    if let Some(issue) = task.issue.as_ref() {
        if let Err(error) = gh_output(
            repository,
            &[
                "issue",
                "edit",
                &issue.number.to_string(),
                "--remove-label",
                "in-progress",
            ],
            "could not remove GitHub issue label",
        ) {
            eprintln!("alucard: {error}");
        }
    }
}

struct SelectedRunTask {
    ledger_id: String,
    issue: Option<GithubTask>,
    local_id: Option<String>,
    local_title: String,
    local_body: String,
    parent_context: String,
}

#[derive(serde::Serialize)]
struct WorkerIssuePrompt<'a> {
    number: i64,
    title: &'a str,
    labels: Vec<&'a str>,
}

#[derive(serde::Serialize)]
struct WorkerLocalTaskPrompt<'a> {
    id: Option<&'a str>,
    title: &'a str,
    body: &'a str,
}

fn select_run_task(
    options: &QueueOptions,
    task_file: Option<&Path>,
    database: &Path,
) -> Result<Option<SelectedRunTask>, String> {
    if let Some(file) = task_file {
        let parsed = parse_tasks(&fs::read_to_string(file).map_err(|error| error.to_string())?)
            .map_err(|errors| errors.join("\n"))?;
        let tasks = local_queue_with_open_issues(
            &parsed.tasks,
            fetch_open_issue_numbers(&options.repository).as_deref(),
        );
        migrate(database).map_err(|error| error.to_string())?;
        let mut connection = open(database).map_err(|error| error.to_string())?;
        import_local_tasks(
            &mut connection,
            &options.repository.display().to_string(),
            file,
            &parsed.tasks,
        )
        .map_err(|error| error.to_string())?;
        let task = tasks.into_iter().find(|task| {
            task_id(
                database,
                &options.repository.display().to_string(),
                "local",
                &file.display().to_string(),
                &task.id,
            )
            .and_then(|id| id.map(|id| task_is_pending(database, &id)).transpose())
            .ok()
            .flatten()
            .unwrap_or(false)
        });
        let Some(task) = task else {
            return Ok(None);
        };
        let id = task_id(
            database,
            &options.repository.display().to_string(),
            "local",
            &file.display().to_string(),
            &task.id,
        )
        .map_err(|error| error.to_string())?
        .ok_or("imported local task is missing")?;
        return Ok(Some(SelectedRunTask {
            ledger_id: id,
            issue: None,
            local_id: Some(task.id),
            local_title: task.title,
            local_body: task.body,
            parent_context: parsed.parent_context,
        }));
    }
    let (issues, pinned_is_pr) = if let Some(number) = options.issue {
        let (issue, is_pr) = fetch_pinned_issue(&options.repository, number)?;
        (vec![issue], is_pr)
    } else {
        (fetch_github_issues(&options.repository)?, false)
    };
    let open_issues = fetch_open_issue_numbers_required(&options.repository)?;
    let prs = fetch_open_prs(&options.repository)?;
    let issues = if options.issue.is_some() {
        vec![alucard::resolve_pinned_issue(
            issues.into_iter().next().unwrap(),
            pinned_is_pr,
            &open_issues,
            &prs,
        )?]
    } else {
        github_queue(issues, &open_issues, &prs)
    };
    migrate(database).map_err(|error| error.to_string())?;
    let mut connection = open(database).map_err(|error| error.to_string())?;
    import_github_tasks(
        &mut connection,
        &options.repository.display().to_string(),
        &issues,
    )
    .map_err(|error| error.to_string())?;
    let issue = issues.into_iter().find(|issue| {
        task_id(
            database,
            &options.repository.display().to_string(),
            "github",
            "",
            &issue.number.to_string(),
        )
        .and_then(|id| id.map(|id| task_is_pending(database, &id)).transpose())
        .ok()
        .flatten()
        .unwrap_or(false)
    });
    let Some(issue) = issue else {
        return Ok(None);
    };
    let id = task_id(
        database,
        &options.repository.display().to_string(),
        "github",
        "",
        &issue.number.to_string(),
    )
    .map_err(|error| error.to_string())?
    .ok_or("imported GitHub task is missing")?;
    Ok(Some(SelectedRunTask {
        ledger_id: id,
        issue: Some(issue),
        local_id: None,
        local_title: String::new(),
        local_body: String::new(),
        parent_context: String::new(),
    }))
}

fn worker_prompt(
    tools: &Path,
    task: &SelectedRunTask,
    base: &str,
    commits: &str,
    status: &str,
) -> Result<String, String> {
    let trailer = if task.issue.is_some() {
        "alucard-worker-github-issue-prompt.md"
    } else {
        "alucard-worker-local-prompt.md"
    };
    let instructions = compose_role_prompt(tools, "alucard-worker-prompt.md", Some(trailer))
        .map_err(|error| error.to_string())?;
    let task_data = if let Some(issue) = &task.issue {
        let issue = WorkerIssuePrompt {
            number: issue.number,
            title: &issue.title,
            labels: issue
                .labels
                .iter()
                .map(|label| label.name.as_str())
                .collect(),
        };
        format!(
            "<issue>{}</issue>",
            xml_escape(
                &serde_json::to_string(&issue).expect("worker issue prompt is serializable")
            )
        )
    } else {
        let local_task = WorkerLocalTaskPrompt {
            id: task.local_id.as_deref(),
            title: &task.local_title,
            body: &task.local_body,
        };
        format!(
            "<parent_context>{}</parent_context>\n<task>{}</task>",
            xml_escape(&task.parent_context),
            xml_escape(
                &serde_json::to_string(&local_task)
                    .expect("worker local task prompt is serializable")
            )
        )
    };
    Ok(format!(
        "<instructions>{instructions}</instructions>\n{}<base_branch>{}</base_branch>\n<commits>{}</commits>\n<toolchain_status>\n{}\n</toolchain_status>\n{task_data}",
        if task.issue.is_some() {
            ""
        } else {
            "<task_source>file</task_source>\n"
        },
        xml_escape(base),
        xml_escape(commits),
        xml_escape(status),
    ))
}

fn recent_base_commits(repository: &Path, base: &str) -> Result<String, String> {
    let output = execute_command(
        "git",
        &[
            "log",
            "-n",
            "5",
            "--format=%H%n%ad%n%B---",
            "--date=short",
            &format!("origin/{base}"),
        ],
        repository,
        None,
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_end_matches('\n')
        .to_string())
}

fn prepare_attempt(
    database: &Path,
    options: &RunCommandOptions,
    cache: &Path,
    selected: &SelectedRunTask,
    run: &alucard::Run,
) -> Result<alucard::PreparedRun, alucard::RunError> {
    prepare_run(
        database,
        RunOptions {
            repository: &options.queue.repository,
            logs_root: &options.logs_root,
            cache_root: cache,
            base: &options.base,
            image: Some(&options.image),
            task: selected.local_id.as_deref(),
            issue: selected.issue.as_ref().map(|issue| issue.number),
            run_id: Some(&run.id),
        },
    )
}

fn find_attempt_pr(
    repository: &Path,
    prepared: &alucard::PreparedRun,
) -> Result<Option<(i64, String, String)>, String> {
    if let Some(pull) = find_open_pr(repository, &prepared.branch)? {
        return Ok(Some(pull));
    }

    #[derive(serde::Deserialize)]
    struct Pull {
        number: i64,
        #[serde(rename = "headRefName")]
        branch: String,
        #[serde(rename = "headRefOid")]
        sha: String,
    }
    let pulls: Vec<Pull> = serde_json::from_slice(&gh_output(
        repository,
        &[
            "pr",
            "list",
            "--label",
            "alucard",
            "--state",
            "open",
            "--json",
            "number,headRefName,headRefOid",
        ],
        "could not discover pull request",
    )?)
    .map_err(|error| format!("could not parse pull requests: {error}"))?;
    let output = execute_command(
        "git",
        &[
            "rev-list",
            &format!(
                "{}..HEAD",
                prepared.run.base_sha.as_deref().unwrap_or("HEAD")
            ),
        ],
        &prepared.clone,
        None,
    )
    .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let commits_output = String::from_utf8_lossy(&output.stdout);
    let commits = commits_output
        .lines()
        .collect::<std::collections::BTreeSet<_>>();
    Ok(pulls
        .into_iter()
        .find(|pull| commits.contains(pull.sha.as_str()))
        .map(|pull| (pull.number, pull.branch, pull.sha)))
}

fn github_repo_slug(repository: &Path) -> Option<String> {
    let output = execute_command("git", &["remote", "get-url", "origin"], repository, None).ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout);
    let url = url.trim().strip_suffix(".git").unwrap_or(url.trim());
    let slug = url
        .strip_prefix("git@github.com:")
        .or_else(|| url.strip_prefix("https://github.com/"))?;
    let mut segments = slug.split('/');
    let owner = segments.next()?;
    let name = segments.next()?;
    if segments.next().is_some()
        || owner.is_empty()
        || name.is_empty()
        || !owner
            .bytes()
            .chain(name.bytes())
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

fn fetch_pinned_issue(repository: &Path, number: i64) -> Result<(GithubTask, bool), String> {
    #[derive(serde::Deserialize)]
    struct PinnedIssue {
        #[serde(flatten)]
        task: GithubTask,
    }
    let issue: PinnedIssue = serde_json::from_slice(&gh_output(
        repository,
        &[
            "issue",
            "view",
            &number.to_string(),
            "--json",
            "number,title,body,labels,state",
        ],
        "could not view GitHub issue",
    )?)
    .map_err(|error| format!("could not parse GitHub issue: {error}"))?;
    let is_pull_request = pinned_item_is_pull_request(repository, number)?;
    Ok((issue.task, is_pull_request))
}

fn pinned_item_is_pull_request(repository: &Path, number: i64) -> Result<bool, String> {
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
        "could not check whether GitHub issue is a pull request",
    )?)
    .map_err(|error| format!("could not parse repository name: {error}"))?;
    let (owner, name) = owner
        .trim()
        .split_once('/')
        .ok_or_else(|| "could not check whether GitHub issue is a pull request".to_string())?;
    let query = "query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){issueOrPullRequest(number:$number){__typename}}}";
    let item_type = String::from_utf8(gh_output(
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
            "-F",
            &format!("number={number}"),
            "--jq",
            ".data.repository.issueOrPullRequest.__typename",
        ],
        "could not check whether GitHub issue is a pull request",
    )?)
    .map_err(|error| format!("could not parse GitHub item type: {error}"))?;
    match item_type.trim() {
        "Issue" => Ok(false),
        "PullRequest" => Ok(true),
        _ => Err("could not determine whether GitHub issue is a pull request".to_string()),
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn find_open_pr(repository: &Path, branch: &str) -> Result<Option<(i64, String, String)>, String> {
    #[derive(serde::Deserialize)]
    struct Pull {
        number: i64,
        #[serde(rename = "headRefName")]
        branch: String,
        #[serde(rename = "headRefOid")]
        sha: String,
    }
    let pulls: Vec<Pull> = serde_json::from_slice(&gh_output(
        repository,
        &[
            "pr",
            "list",
            "--state",
            "open",
            "--head",
            branch,
            "--json",
            "number,headRefName,headRefOid",
        ],
        "could not discover pull request",
    )?)
    .map_err(|error| format!("could not parse pull requests: {error}"))?;
    Ok(pulls
        .into_iter()
        .next()
        .map(|pull| (pull.number, pull.branch, pull.sha)))
}

fn cache_root() -> PathBuf {
    nonempty_environment("ALUCARD_CACHE_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            nonempty_environment("XDG_CACHE_HOME").map(|root| PathBuf::from(root).join("alucard"))
        })
        .or_else(|| {
            nonempty_environment("HOME").map(|root| PathBuf::from(root).join(".cache/alucard"))
        })
        .unwrap_or_else(|| PathBuf::from(".alucard-cache"))
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

fn gh_output(repository: &Path, args: &[&str], context: &str) -> Result<Vec<u8>, String> {
    let output = execute_command("gh", args, repository, None)
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

fn fetch_github_issues(repository: &Path) -> Result<Vec<GithubTask>, String> {
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

fn fetch_open_issue_numbers_required(repository: &Path) -> Result<Vec<i64>, String> {
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

fn fetch_open_prs(repository: &Path) -> Result<Vec<GithubPullRequest>, String> {
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

fn fetch_open_issue_numbers(repository: &Path) -> Option<Vec<i64>> {
    let output = execute_command(
        "gh",
        &[
            "issue", "list", "--state", "open", "--json", "number", "--limit", "1000",
        ],
        repository,
        None,
    )
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
                let recovery = match run_recovery(path, &run.id) {
                    Ok(recovery) => recovery,
                    Err(error) => return fail(&format!("alucard: database error: {error}")),
                };
                if let Some(recovery) = recovery {
                    println!("recovery branch: {}", recovery.branch);
                    if let Some(number) = recovery.pull_request_number {
                        println!("recovery pr: #{number}");
                    }
                    if let Some(checkout_path) = recovery.checkout_path {
                        println!("recovery checkout: {checkout_path}");
                    }
                    if let Some(failure_reason) = recovery.failure_reason {
                        println!("recovery failure: {failure_reason}");
                    }
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

/// The checkout or installed tool directory containing the executable's supporting files.
fn tool_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?.canonicalize().ok()?;
    exe.ancestors()
        .find(|d| d.join("Cargo.toml").is_file())
        .map(PathBuf::from)
        .or_else(|| exe.parent()?.parent().map(PathBuf::from))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_prompt_matches_bash_for_pinned_issues_and_local_tasks() {
        let bash_prompt =
            |mode: &str, queue: &str, commits: &str, instructions: &str, tasks: &Path| {
                let output = Command::new("bash")
                    .args([
                        "-c",
                        r#"source "$5/alucard"
TASK_SOURCE="$4"
PIN_ISSUE=""
BASE_BRANCH="main"
TOOLCHAIN_STATUS="No Python or Node manifest found — dependency preflight skipped."
if [ "$TASK_SOURCE" = file ]; then
  TASKS_FILE_ABS="$6"
else
  PIN_ISSUE=42
fi
build_worker_prompt "$1" "$2" "$3"
printf '%s' "$FULL_PROMPT""#,
                        "bash",
                        queue,
                        commits,
                        instructions,
                        mode,
                        env!("CARGO_MANIFEST_DIR"),
                        tasks.to_str().unwrap(),
                    ])
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap()
            };

        let commits = "123\n2026-10-13\nsubject\nbody---";
        let issue = GithubTask {
            number: 42,
            title: "Pinned <issue>".to_string(),
            body: "ignored by the worker prompt".to_string(),
            labels: vec![alucard::GithubLabel {
                name: "ready-for-agent".to_string(),
            }],
            state: None,
        };
        let issue_task = SelectedRunTask {
            ledger_id: "task".to_string(),
            issue: Some(issue.clone()),
            local_id: None,
            local_title: String::new(),
            local_body: String::new(),
            parent_context: String::new(),
        };
        let issue_instructions = compose_role_prompt(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "alucard-worker-prompt.md",
            Some("alucard-worker-github-issue-prompt.md"),
        )
        .unwrap();
        let issue_queue = serde_json::json!([issue]).to_string();
        assert_eq!(
            worker_prompt(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &issue_task,
                "main",
                commits,
                "No Python or Node manifest found — dependency preflight skipped.",
            )
            .unwrap(),
            bash_prompt(
                "github",
                &issue_queue,
                commits,
                &issue_instructions,
                Path::new(env!("CARGO_MANIFEST_DIR")),
            )
        );

        let tasks = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures/tasks-basic.md");
        let parsed = parse_tasks(&fs::read_to_string(&tasks).unwrap()).unwrap();
        let task = &parsed.tasks[0];
        let local_task = SelectedRunTask {
            ledger_id: "local".to_string(),
            issue: None,
            local_id: Some(task.id.clone()),
            local_title: task.title.clone(),
            local_body: task.body.clone(),
            parent_context: parsed.parent_context,
        };
        let local_instructions = compose_role_prompt(
            Path::new(env!("CARGO_MANIFEST_DIR")),
            "alucard-worker-prompt.md",
            Some("alucard-worker-local-prompt.md"),
        )
        .unwrap();
        let local_queue = format!(
            "[{}]",
            serde_json::to_string(&WorkerLocalTaskPrompt {
                id: local_task.local_id.as_deref(),
                title: &local_task.local_title,
                body: &local_task.local_body,
            })
            .unwrap()
        );
        assert_eq!(
            worker_prompt(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                &local_task,
                "main",
                commits,
                "No Python or Node manifest found — dependency preflight skipped.",
            )
            .unwrap(),
            bash_prompt("file", &local_queue, commits, &local_instructions, &tasks)
        );
    }

    #[test]
    fn recovery_body_attributes_tasks_by_source() {
        let github_task = SelectedRunTask {
            ledger_id: "github".to_string(),
            issue: Some(GithubTask {
                number: 42,
                title: String::new(),
                body: String::new(),
                labels: Vec::new(),
                state: None,
            }),
            local_id: None,
            local_title: String::new(),
            local_body: String::new(),
            parent_context: String::new(),
        };
        let local_task = SelectedRunTask {
            ledger_id: "local".to_string(),
            issue: None,
            local_id: Some("7".to_string()),
            local_title: String::new(),
            local_body: String::new(),
            parent_context: String::new(),
        };

        let github = recovery_body(&github_task, alucard::AgentExit::Exhausted, Some(1), 2);
        assert!(github.starts_with("Refs #42\n\n"));
        assert!(github.contains("change the `Refs` line above to `Closes`"));
        assert!(github.contains("`alucard run --issue <N>`"));

        let local = recovery_body(&local_task, alucard::AgentExit::Exhausted, Some(1), 2);
        assert!(local.starts_with("Task: 7\n\n"));
        assert!(local.contains("the harness flips the task to `[x]` when this PR merges."));
        assert!(local.contains("task flips back to `[ ]` at the next queue build"));
    }
}
