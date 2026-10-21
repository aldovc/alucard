mod test_support;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use test_support::copy_tool_installation;

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
fn version_matches_git_describe() {
    let root = env!("CARGO_MANIFEST_DIR");
    let run = |cmd: &mut Command| {
        let out = cmd.arg("version").current_dir(root).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let rust = run(&mut Command::new(env!("CARGO_BIN_EXE_alucard")));
    let describe = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(describe.status.success());
    let expected = String::from_utf8(describe.stdout).unwrap();
    assert_eq!(rust, expected);
    assert_ne!(rust.trim(), "unknown");
}

#[test]
fn top_level_help_matches_committed_text() {
    for arguments in [Vec::<&str>::new(), vec!["help"], vec!["-h"], vec!["--help"]] {
        let rust = Command::new(env!("CARGO_BIN_EXE_alucard"))
            .args(&arguments)
            .output()
            .unwrap();
        assert!(rust.status.success());
        assert_eq!(rust.stdout, include_bytes!("fixtures/top-level-help.txt"));
    }
}

#[test]
fn cli_error_messages_match_bash() {
    let root = env!("CARGO_MANIFEST_DIR");
    for (arguments, stderr) in [
        (
            vec!["run", root, "-n", "x"],
            "alucard: iterations must be a positive integer\n",
        ),
        (
            vec!["continue"],
            "alucard: usage: alucard continue <PR_NUM> [options] — PR_NUM must be a positive integer\n",
        ),
        (
            vec!["merge"],
            "alucard: usage: alucard merge <PR_NUM> [options] — PR_NUM must be a positive integer\n",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
            .args(arguments)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(String::from_utf8(output.stderr).unwrap(), stderr);
    }
}

#[test]
fn legacy_run_shorthand_and_directory_command_dispatch_to_run() {
    let root = env!("CARGO_MANIFEST_DIR");
    let directory = std::env::temp_dir().join(format!(
        "alucard-cli-dispatch-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let missing_env = directory.join("missing.env");
    let target = directory.to_str().unwrap();

    for arguments in [
        vec!["1", target, "--env-file", missing_env.to_str().unwrap()],
        vec![
            "1",
            "2",
            target,
            "--env-file",
            missing_env.to_str().unwrap(),
        ],
        vec![target, "--env-file", missing_env.to_str().unwrap()],
        vec![
            target,
            "-n",
            "1",
            "--env-file",
            missing_env.to_str().unwrap(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_alucard"))
            .args(&arguments)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("missing env file"));
    }
    fs::remove_dir_all(directory).unwrap();
}

/// A copied tool installation, a target checkout with an origin, a migrated ledger and stubbed gh, jq and docker.
struct DoctorFixture {
    directory: PathBuf,
    binary: PathBuf,
    tools: PathBuf,
    repository: PathBuf,
}

impl DoctorFixture {
    fn new(name: &str) -> Self {
        let directory = std::env::temp_dir().join(format!(
            "alucard-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let binary = copy_tool_installation(&directory);
        let bin = directory.join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (tool, status) in [("gh", 0), ("jq", 0), ("docker", 1)] {
            let stub = bin.join(tool);
            fs::write(&stub, format!("#!/bin/sh\nexit {status}\n")).unwrap();
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let repository = directory.join("repository");
        for args in [
            vec!["init", "-q", repository.to_str().unwrap()],
            vec![
                "-C",
                repository.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                "https://example.test/repository.git",
            ],
        ] {
            assert!(Command::new("git").args(args).status().unwrap().success());
        }
        assert!(Command::new(&binary)
            .arg("--db-path")
            .arg(directory.join("ledger.db"))
            .args(["db", "migrate"])
            .status()
            .unwrap()
            .success());
        Self {
            tools: directory.join("tools").canonicalize().unwrap(),
            repository: repository.canonicalize().unwrap(),
            directory,
            binary,
        }
    }

    fn doctor(&self) -> Command {
        let mut command = Command::new(&self.binary);
        command
            .arg("--db-path")
            .arg(self.directory.join("ledger.db"))
            .arg("doctor")
            .current_dir(&self.repository)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.directory.join("bin").display()),
            );
        command
    }

    fn expected(&self, lines: &[String]) -> String {
        [
            "ok: git".to_string(),
            "ok: gh".to_string(),
            "ok: jq".to_string(),
            "ok: docker".to_string(),
            "ok: timeout".to_string(),
            format!("ok: tool files in {}", self.tools.display()),
        ]
        .iter()
        .chain(lines)
        .chain([&"missing: Docker image ghcr.io/aldovc/alucard:latest (run: docker pull ghcr.io/aldovc/alucard:latest, or alucard build for a local image)".to_string()])
        .cloned()
        .collect::<Vec<_>>()
        .join("\n")
    }
}

impl Drop for DoctorFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

#[test]
fn doctor_reports_prerequisites_before_ledger_checks() {
    let fixture = DoctorFixture::new("doctor");
    let env_file = fixture.directory.join("doctor.env");
    fs::write(
        &env_file,
        "GITHUB_TOKEN=test\nALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
    )
    .unwrap();
    let expected = fixture.expected(&[
        format!("ok: env file {}", env_file.display()),
        "ok: OPENAI_API_KEY".to_string(),
        format!("ok: target repo {}", fixture.repository.display()),
        "ok: target repo has origin remote".to_string(),
    ]);
    let rust = fixture
        .doctor()
        .args([".", "--env-file", env_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(without_ledger_check(rust), expected);
}

#[test]
fn doctor_falls_back_to_defaults_for_an_empty_environment() {
    let fixture = DoctorFixture::new("doctor-empty-defaults");
    let expected = fixture.expected(&[
        format!(
            "missing: env file {}",
            fixture.tools.join("alucard.env").display()
        ),
        format!("ok: target repo {}", fixture.repository.display()),
        "ok: target repo has origin remote".to_string(),
    ]);
    let rust = empty_environment_defaults(&mut fixture.doctor())
        .output()
        .unwrap();

    assert!(!String::from_utf8_lossy(&rust.stderr).contains("tasks file not found"));
    assert_eq!(without_ledger_check(rust), expected);
}

#[test]
fn doctor_falls_back_to_xdg_cache_for_an_empty_cache_directory() {
    let fixture = DoctorFixture::new("doctor-empty-cache");
    let cache = fixture.directory.join("cache");
    let expected = fixture.expected(&[
        format!(
            "missing: env file {}",
            fixture.tools.join("alucard.env").display()
        ),
        format!(
            "ok: target repo is GitHub ref owner/repo (cache: {}/alucard/owner/repo)",
            cache.display()
        ),
    ]);
    let rust = fixture
        .doctor()
        .arg("owner/repo")
        .env("ALUCARD_CACHE_DIR", "")
        .env("XDG_CACHE_HOME", &cache)
        .output()
        .unwrap();

    assert_eq!(without_ledger_check(rust), expected);
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
