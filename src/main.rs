//! Alucard orchestrator: the Rust port of the `alucard` Bash script.

use alucard::{
    database_path, find_run, interrupt_running_runs, migrate, prepare_run, recent_runs, run_claims,
    run_command as execute_command, run_pull_requests, status, RunOptions,
};
use std::path::PathBuf;
use std::process::ExitCode;

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
        Some("runs") => match database_path(db_path) {
            Ok(path) => runs_command(&commands[1..], &path),
            Err(error) => fail(&format!("alucard: {error}")),
        },
        Some("run") => match database_path(db_path) {
            Ok(path) => run_command(&commands[1..], &path),
            Err(error) => fail(&format!("alucard: {error}")),
        },
        Some(cmd) => fail(&format!("alucard: unknown command: {cmd}")),
        None => fail(
            "usage: alucard [--db-path PATH] {version|db migrate|db status|runs|run REPOSITORY}",
        ),
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

fn run_command(args: &[String], path: &std::path::Path) -> ExitCode {
    let Some(repository) = args.first() else {
        return fail("usage: alucard run REPOSITORY");
    };
    if args.len() != 1 {
        return fail("usage: alucard run REPOSITORY");
    }
    if let Err(error) = migrate(path).and_then(|_| interrupt_running_runs(path)) {
        return fail(&format!("alucard: database error: {error}"));
    }
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("."));
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .unwrap_or_else(|| PathBuf::from("."));
    match prepare_run(
        path,
        RunOptions {
            repository: std::path::Path::new(repository),
            logs_root: &state.join("alucard/logs"),
            cache_root: &cache.join("alucard/repos"),
            base: "HEAD",
            image: None,
            task: None,
            issue: None,
        },
    ) {
        Ok(run) => {
            println!(
                "run: {}\nbranch: {}\nclone: {}",
                run.run.id,
                run.branch,
                run.clone.display()
            );
            ExitCode::SUCCESS
        }
        Err(error) => fail(&format!("alucard: run preparation failed: {error}")),
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
