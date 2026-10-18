use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug)]
enum Implementation {
    Bash,
    Rust,
}

struct Fixture {
    root: PathBuf,
    local: PathBuf,
}

impl Fixture {
    fn new(order: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "alucard-implementation-guard-{order}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let local = root.join("local");
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main", local.to_str().unwrap()],
            vec![
                "-C",
                local.to_str().unwrap(),
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "seed",
            ],
            vec![
                "-C",
                local.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                "https://github.com/Owner/Repository.git",
            ],
        ] {
            assert!(Command::new("git")
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .status()
                .unwrap()
                .success());
        }
        // Every external call a run makes after the guard parks here and reports which process got that far.
        for tool in ["gh", "docker"] {
            let stub = root.join("bin").join(tool);
            fs::write(
                &stub,
                "#!/bin/sh\necho \"$*\" >> \"$ALUCARD_TEST_REACHED\"\nexec sleep 600\n",
            )
            .unwrap();
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(
            root.join("alucard.env"),
            "GITHUB_TOKEN=test\nALUCARD_PROVIDER=codex\nOPENAI_API_KEY=test\n",
        )
        .unwrap();
        Self { root, local }
    }

    fn command(&self, implementation: Implementation, target: &str, label: &str) -> Command {
        let mut command = match implementation {
            Implementation::Bash => {
                let mut command = Command::new("bash");
                command.arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("alucard"));
                command
            }
            Implementation::Rust => {
                let mut command = Command::new(env!("CARGO_BIN_EXE_alucard"));
                command
                    .arg("--db-path")
                    .arg(self.root.join(format!("{label}.db")));
                command
            }
        };
        command
            .args(["run", target, "--no-build", "--env-file"])
            .arg(self.root.join("alucard.env"))
            .args(["--logs-root"])
            .arg(self.root.join("logs"))
            .current_dir(&self.root)
            .env(
                "PATH",
                format!(
                    "{}:{}",
                    self.root.join("bin").display(),
                    std::env::var("PATH").unwrap()
                ),
            )
            .env("HOME", self.root.join("home"))
            .env("GIT_CONFIG_GLOBAL", self.root.join("home/.gitconfig"))
            .env("ALUCARD_CACHE_DIR", self.root.join("cache"))
            .env("ALUCARD_TEST_REACHED", self.reached(label))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    fn reached(&self, label: &str) -> PathBuf {
        self.root.join(format!("{label}.reached"))
    }

    /// Starts a run in its own process group and waits until it is past the guard.
    fn hold(&self, implementation: Implementation, label: &str) -> Child {
        let mut child = self
            .command(implementation, self.local.to_str().unwrap(), label)
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while !self.reached(label).exists() {
            if let Some(status) = child.try_wait().unwrap() {
                let mut stderr = String::new();
                std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
                panic!("{implementation:?} {label} exited ({status}) before its first tool call: {stderr}");
            }
            assert!(
                Instant::now() < deadline,
                "{implementation:?} {label} never reached a tool call"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        child
    }
}

fn kill_group(mut child: Child) {
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
    child.wait().unwrap();
}

#[test]
fn bash_and_rust_refuse_each_other_on_one_repository() {
    for (index, (holder, other, target)) in [
        (
            Implementation::Rust,
            Implementation::Bash,
            Some("owner/repository"),
        ),
        (
            Implementation::Bash,
            Implementation::Rust,
            Some("https://github.com/OWNER/Repository.git"),
        ),
        (Implementation::Bash, Implementation::Rust, None),
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = Fixture::new(&index.to_string());
        let key = match target {
            Some(_) => "github.com/owner/repository".to_string(),
            None => {
                assert!(Command::new("git")
                    .arg("-C")
                    .arg(&fixture.local)
                    .args(["remote", "remove", "origin"])
                    .status()
                    .unwrap()
                    .success());
                fixture.local.display().to_string()
            }
        };
        let target = target.unwrap_or(fixture.local.to_str().unwrap());

        let first = fixture.hold(holder, "holder");
        let second = fixture.hold(holder, "same-implementation");

        let mut refused = fixture
            .command(other, target, "refused")
            .process_group(0)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        while refused.try_wait().unwrap().is_none() {
            if fixture.reached("refused").exists() || Instant::now() > deadline {
                kill_group(refused);
                panic!("{other:?} started beside {holder:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let refused = refused.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            !refused.status.success(),
            "{other:?} started beside {holder:?}"
        );
        for expected in [
            format!("pid {}:", first.id()),
            format!("pid {}:", second.id()),
            format!("run {}", fixture.local.display()),
            format!("running on {key}:"),
        ] {
            assert!(
                stderr.contains(&expected),
                "{other:?} refusal lacks {expected:?}: {stderr}"
            );
        }
        assert!(
            !fixture.reached("refused").exists(),
            "the refused run called a tool"
        );
        assert!(
            !fixture.root.join("cache/owner").exists(),
            "the refused run fetched a clone"
        );

        kill_group(first);
        kill_group(second);
        kill_group(fixture.hold(other, "after-holder-died"));
        fs::remove_dir_all(&fixture.root).unwrap();
    }
}
