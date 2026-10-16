mod test_support;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn empty_environment_defaults(command: &mut Command) -> &mut Command {
    command
        .env("ALUCARD_TARGET_REPO", "")
        .env("ALUCARD_ENV_FILE", "")
        .env("ALUCARD_IMAGE", "")
        .env("ALUCARD_TASKS_FILE", "")
        .env("ALUCARD_CACHE_DIR", "")
}

#[cfg(unix)]
#[test]
fn build_uses_default_image_for_a_blank_environment_and_rejects_a_blank_flag() {
    let root = env!("CARGO_MANIFEST_DIR");
    let directory = std::env::temp_dir().join(format!(
        "alucard-build-command-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let bin = directory.join("bin");
    let trace = directory.join("docker.trace");
    fs::create_dir_all(&bin).unwrap();
    let docker = bin.join("docker");
    fs::write(
        &docker,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ALUCARD_DOCKER_TRACE\"\ncase \"$1 $2\" in\n  'image inspect') exit 1 ;;\nesac\n",
    )
    .unwrap();
    fs::set_permissions(&docker, fs::Permissions::from_mode(0o755)).unwrap();

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .arg("build")
        .current_dir(root)
        .env("ALUCARD_IMAGE", "")
        .env("ALUCARD_DOCKER_TRACE", &trace)
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(fs::read_to_string(&trace)
        .unwrap()
        .contains("-t ghcr.io/aldovc/alucard:latest"));

    let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args(["build", "--image", ""])
        .current_dir(root)
        .env("ALUCARD_DOCKER_TRACE", &trace)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--image requires an image name"));

    fs::remove_dir_all(directory).unwrap();
}

fn without_ledger_check(output: std::process::Output) -> String {
    String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("ok: ledger "))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn version_matches_bash() {
    let root = env!("CARGO_MANIFEST_DIR");
    let run = |cmd: &mut Command| {
        let out = cmd.arg("version").current_dir(root).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let rust = run(&mut Command::new(env!("CARGO_BIN_EXE_alucard")));
    let bash = run(Command::new("bash").arg(format!("{root}/alucard")));
    assert_eq!(rust, bash);
    assert_ne!(rust.trim(), "unknown");
}

#[test]
fn doctor_matches_bash_before_ledger_checks() {
    let root = env!("CARGO_MANIFEST_DIR");
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory =
        std::env::temp_dir().join(format!("alucard-doctor-{}-{unique}", std::process::id()));
    fs::create_dir(&directory).unwrap();
    let env_file = directory.join("doctor.env");
    let database = directory.join("ledger.db");
    fs::write(
        &env_file,
        "GITHUB_TOKEN=test\nALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
    )
    .unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args(["--db-path", database.to_str().unwrap(), "db", "migrate"])
        .status()
        .unwrap()
        .success());
    let bash = Command::new("bash")
        .args([
            "alucard",
            "doctor",
            ".",
            "--env-file",
            env_file.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let rust = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args([
            "--db-path",
            database.to_str().unwrap(),
            "doctor",
            ".",
            "--env-file",
            env_file.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let rust = without_ledger_check(rust);
    assert_eq!(String::from_utf8(bash.stdout).unwrap().trim(), rust);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn doctor_empty_environment_defaults_match_bash() {
    let root = env!("CARGO_MANIFEST_DIR");
    let directory = std::env::temp_dir().join(format!(
        "alucard-doctor-empty-defaults-{}",
        std::process::id()
    ));
    fs::create_dir_all(&directory).unwrap();
    let database = directory.join("ledger.db");
    assert!(Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args(["--db-path", database.to_str().unwrap(), "db", "migrate"])
        .status()
        .unwrap()
        .success());

    let mut bash = Command::new("bash");
    bash.args(["alucard", "doctor"]).current_dir(root);
    let bash = empty_environment_defaults(&mut bash).output().unwrap();
    let mut rust = Command::new(env!("CARGO_BIN_EXE_alucard"));
    rust.args(["--db-path", database.to_str().unwrap(), "doctor"])
        .current_dir(root);
    let rust = empty_environment_defaults(&mut rust).output().unwrap();

    assert!(!String::from_utf8_lossy(&rust.stderr).contains("tasks file not found"));
    assert_eq!(
        String::from_utf8(bash.stdout).unwrap().trim(),
        without_ledger_check(rust)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn doctor_empty_cache_directory_matches_bash() {
    let root = env!("CARGO_MANIFEST_DIR");
    let directory =
        std::env::temp_dir().join(format!("alucard-doctor-empty-cache-{}", std::process::id()));
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let gh = bin.join("gh");
    fs::write(&gh, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let database = directory.join("ledger.db");
    assert!(Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args(["--db-path", database.to_str().unwrap(), "db", "migrate"])
        .status()
        .unwrap()
        .success());
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let bash = Command::new("bash")
        .args(["alucard", "doctor", "owner/repo"])
        .current_dir(root)
        .env("ALUCARD_CACHE_DIR", "")
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .env("PATH", &path)
        .output()
        .unwrap();
    let rust = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args([
            "--db-path",
            database.to_str().unwrap(),
            "doctor",
            "owner/repo",
        ])
        .current_dir(root)
        .env("ALUCARD_CACHE_DIR", "")
        .env("XDG_CACHE_HOME", directory.join("cache"))
        .env("PATH", path)
        .output()
        .unwrap();

    assert_eq!(
        String::from_utf8(bash.stdout).unwrap().trim(),
        without_ledger_check(rust)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn doctor_rejects_invalid_provider_settings() {
    let root = env!("CARGO_MANIFEST_DIR");
    let directory =
        std::env::temp_dir().join(format!("alucard-doctor-invalid-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let env_file = directory.join("doctor.env");
    let database = directory.join("ledger.db");
    fs::write(&env_file, "GITHUB_TOKEN=test\nALUCARD_PROVIDER=invalid\n").unwrap();
    assert!(Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args(["--db-path", database.to_str().unwrap(), "db", "migrate"])
        .status()
        .unwrap()
        .success());
    let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
        .args([
            "--db-path",
            database.to_str().unwrap(),
            "doctor",
            ".",
            "--env-file",
            env_file.to_str().unwrap(),
        ])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("missing: unrecognized provider 'invalid' for role WORKER"));
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn doctor_normalizes_github_url_references() {
    let root = env!("CARGO_MANIFEST_DIR");
    let directory =
        std::env::temp_dir().join(format!("alucard-doctor-github-{}", std::process::id()));
    let bin = directory.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let trace = directory.join("gh.args");
    let gh = bin.join("gh");
    fs::write(
        &gh,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$ALUCARD_GH_ARGS\"\n",
    )
    .unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    let env_file = directory.join("doctor.env");
    fs::write(
        &env_file,
        "GITHUB_TOKEN=test\nALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
    )
    .unwrap();

    for (index, reference) in [
        "https://github.com/owner/repo.git",
        "git@github.com:owner/repo.git",
    ]
    .iter()
    .enumerate()
    {
        let database = directory.join(format!("ledger-{index}.db"));
        assert!(Command::new(env!("CARGO_BIN_EXE_alucard"))
            .args(["--db-path", database.to_str().unwrap(), "db", "migrate"])
            .status()
            .unwrap()
            .success());
        let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
            .args([
                "--db-path",
                database.to_str().unwrap(),
                "doctor",
                reference,
                "--env-file",
                env_file.to_str().unwrap(),
            ])
            .current_dir(root)
            .env("ALUCARD_GH_ARGS", &trace)
            .env("ALUCARD_CACHE_DIR", directory.join("cache"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .output()
            .unwrap();
        let output = String::from_utf8(output.stdout).unwrap();
        assert!(output.contains("ok: target repo is GitHub ref owner/repo (cache:"));
    }
    let trace = fs::read_to_string(&trace).unwrap();
    let views = trace
        .lines()
        .filter(|line| line.starts_with("repo view"))
        .collect::<Vec<_>>();
    assert_eq!(views, ["repo view owner/repo --json name"; 2]);
    fs::remove_dir_all(directory).unwrap();
}
