use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REQUIRED_TOOL_FILES: &str = include_str!("../required-tool-files.txt");

fn run(command: &mut Command) {
    assert!(command.status().unwrap().success());
}

fn temp_dir() -> PathBuf {
    let directory = std::env::temp_dir().join(format!(
        "alucard-doctor-test-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    directory
}

fn copy_tool_installation(directory: &Path) -> PathBuf {
    let tools = directory.join("tools");
    let binary_dir = tools.join("bin");
    fs::create_dir_all(&binary_dir).unwrap();
    for file in REQUIRED_TOOL_FILES.lines() {
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join(file),
            tools.join(file),
        )
        .unwrap();
    }
    let binary = binary_dir.join("alucard");
    fs::copy(env!("CARGO_BIN_EXE_alucard"), &binary).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    binary
}

fn doctor(binary: &Path, repository: &Path, bin: &Path) -> std::process::Output {
    Command::new(binary)
        .args([
            "doctor",
            repository.to_str().unwrap(),
            "-t",
            "1",
            "--timeout-minutes",
            "1",
            "--logs-root",
            "/tmp/logs",
        ])
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap()
}

#[test]
fn doctor_uses_checkout_root_and_checks_installed_manifest() {
    let directory = temp_dir();
    let binary = copy_tool_installation(&directory);
    let bin = directory.join("bin");
    fs::create_dir(&bin).unwrap();
    let jq = bin.join("jq");
    fs::write(&jq, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&jq, fs::Permissions::from_mode(0o755)).unwrap();

    let repository = directory.join("repository");
    let nested = repository.join("nested");
    fs::create_dir_all(repository.join(".alucard")).unwrap();
    fs::create_dir(&nested).unwrap();
    fs::write(
        repository.join(".alucard/tasks.md"),
        include_str!("../test/fixtures/tasks-basic.md"),
    )
    .unwrap();
    run(Command::new("git").args(["init", "-q", repository.to_str().unwrap()]));
    run(Command::new("git").args([
        "-C",
        repository.to_str().unwrap(),
        "remote",
        "add",
        "origin",
        "https://example.test/repository.git",
    ]));

    let output = doctor(&binary, &nested, &bin);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(&format!("ok: target repo {}", repository.display())));
    assert!(stdout.contains(&format!(
        "ok: tasks file {} (local task source, 4 todo)",
        repository.join(".alucard/tasks.md").display()
    )));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("unknown option"));

    fs::remove_file(directory.join("tools/required-tool-files.txt")).unwrap();
    let output = doctor(&binary, &nested, &bin);
    assert!(String::from_utf8_lossy(&output.stdout).contains(&format!(
        "missing: {}",
        directory.join("tools/required-tool-files.txt").display()
    )));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn doctor_github_mode_ignores_environment_tasks_file_and_accepts_zero_timeout() {
    let directory = temp_dir();
    let binary = copy_tool_installation(&directory);
    let repository = directory.join("repository");
    fs::create_dir_all(&repository).unwrap();

    let output = Command::new(binary)
        .args([
            "doctor",
            repository.to_str().unwrap(),
            "--github",
            "--timeout-minutes",
            "0",
        ])
        .env("ALUCARD_TASKS_FILE", directory.join("missing-tasks.md"))
        .output()
        .unwrap();

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("tasks file not found"));
    assert!(!stderr.contains("timeout must be a positive integer"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("missing: valid target git repo"));

    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn doctor_accepts_shared_run_options() {
    let directory = temp_dir();
    let binary = copy_tool_installation(&directory);
    let repository = directory.join("repository");
    fs::create_dir_all(&repository).unwrap();

    let output = Command::new(binary)
        .args([
            "doctor",
            repository.to_str().unwrap(),
            "-n",
            "0",
            "--iterations",
            "0",
            "--base",
            "main",
            "--max-review-cycles",
            "0",
            "--no-build",
        ])
        .output()
        .unwrap();

    assert!(!String::from_utf8_lossy(&output.stderr).contains("unknown option"));

    fs::remove_dir_all(directory).unwrap();
}
