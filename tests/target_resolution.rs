use alucard::repository_ledger_id;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

fn temporary_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "alucard-target-resolution-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

fn git(directory: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn run_target(root: &Path, cache: &Path, database: &Path, target: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alucard"))
        .current_dir(root)
        .args([
            "--db-path",
            database.to_str().unwrap(),
            "run",
            target,
            "--no-build",
            "--env-file",
            root.join("missing.env").to_str().unwrap(),
        ])
        .env("HOME", root.join("home"))
        .env("GIT_CONFIG_GLOBAL", root.join("home/.gitconfig"))
        .env("ALUCARD_CACHE_DIR", cache)
        .output()
        .unwrap()
}

fn assert_resolved(output: Output) {
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("env file not found"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn target_resolution_cli_matches_bash_and_reuses_the_cache() {
    let root = temporary_path();
    let remote = root.join("remotes/owner/repository.git");
    let seed = root.join("seed");
    let local = root.join("local");
    fs::create_dir_all(remote.parent().unwrap()).unwrap();
    git(&root, &["init", "--bare", remote.to_str().unwrap()]);
    git(&root, &["init", seed.to_str().unwrap()]);
    git(&seed, &["config", "user.email", "test@example.com"]);
    git(&seed, &["config", "user.name", "Test"]);
    fs::write(seed.join("tracked"), "first\n").unwrap();
    git(&seed, &["add", "tracked"]);
    git(&seed, &["commit", "-m", "first"]);
    git(&seed, &["branch", "-M", "main"]);
    git(
        &seed,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    git(&seed, &["push", "-u", "origin", "main"]);
    git(&remote, &["symbolic-ref", "HEAD", "refs/heads/main"]);
    git(
        &root,
        &["clone", remote.to_str().unwrap(), local.to_str().unwrap()],
    );

    let home = root.join("home");
    fs::create_dir_all(&home).unwrap();
    let config = home.join(".gitconfig");
    let remote_url = format!("file://{}", remote.display());
    for source in [
        "https://github.com/owner/repository.git",
        "git@github.com:owner/repository.git",
    ] {
        git(
            &root,
            &[
                "config",
                "--file",
                config.to_str().unwrap(),
                "--add",
                &format!("url.{remote_url}.insteadOf"),
                source,
            ],
        );
    }

    for (index, target) in [
        "owner/repository",
        "https://github.com/owner/repository.git",
        "git@github.com:owner/repository.git",
    ]
    .iter()
    .enumerate()
    {
        let cache = root.join(format!("cache-{index}"));
        assert_resolved(run_target(
            &root,
            &cache,
            &root.join(format!("{index}.db")),
            target,
        ));
        assert_eq!(
            repository_ledger_id(&cache.join("owner/repository")).unwrap(),
            repository_ledger_id(&local).unwrap()
        );
    }

    let cache = root.join("cache-reuse");
    let database = root.join("reuse.db");
    assert_resolved(run_target(&root, &cache, &database, "owner/repository"));
    assert_resolved(run_target(
        &root,
        &root.join("cache-local"),
        &database,
        local.to_str().unwrap(),
    ));
    assert_eq!(
        repository_ledger_id(&cache.join("owner/repository")).unwrap(),
        repository_ledger_id(&local).unwrap()
    );
    fs::write(seed.join("tracked"), "second\n").unwrap();
    git(&seed, &["commit", "-am", "second"]);
    git(&seed, &["push"]);
    let head = Command::new("git")
        .current_dir(&seed)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert_resolved(run_target(&root, &cache, &database, "owner/repository"));
    let cached_head = Command::new("git")
        .current_dir(cache.join("owner/repository"))
        .args(["rev-parse", "origin/main"])
        .output()
        .unwrap();
    assert_eq!(head.stdout, cached_head.stdout);

    let concurrent_cache = root.join("cache-concurrent");
    let root_for_thread = root.clone();
    let cache_for_thread = concurrent_cache.clone();
    let first = thread::spawn(move || {
        run_target(
            &root_for_thread,
            &cache_for_thread,
            &root_for_thread.join("first.db"),
            "owner/repository",
        )
    });
    let root_for_thread = root.clone();
    let cache_for_thread = concurrent_cache.clone();
    let second = thread::spawn(move || {
        run_target(
            &root_for_thread,
            &cache_for_thread,
            &root_for_thread.join("second.db"),
            "owner/repository",
        )
    });
    assert_resolved(first.join().unwrap());
    assert_resolved(second.join().unwrap());
    git(
        &concurrent_cache.join("owner/repository"),
        &["rev-parse", "--is-inside-work-tree"],
    );

    let non_git = root.join("not-a-repository");
    fs::create_dir(&non_git).unwrap();
    let output = run_target(
        &root,
        &root.join("cache-errors"),
        &root.join("errors.db"),
        non_git.to_str().unwrap(),
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "alucard: target path is not inside a git repo: {}",
        non_git.display()
    )));
    let output = run_target(
        &root,
        &root.join("cache-errors"),
        &root.join("errors.db"),
        "./not-a-repository",
    );
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("alucard: target path is not inside a git repo: ./not-a-repository",));
    let file_target = root.join("not-a-repository-file");
    fs::write(&file_target, "not a repository\n").unwrap();
    let output = run_target(
        &root,
        &root.join("cache-errors"),
        &root.join("errors.db"),
        file_target.to_str().unwrap(),
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!(
        "alucard: target repo does not exist: {}",
        file_target.display()
    )));
    let output = run_target(
        &root,
        &root.join("cache-errors"),
        &root.join("errors.db"),
        "example.invalid/owner/repository",
    );
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("alucard: target repo does not exist: example.invalid/owner/repository"));

    for (index, target) in ["./repository", "../repository", "owner/.", "owner/.."]
        .iter()
        .enumerate()
    {
        let cache = root.join(format!("cache-dot-segment-{index}"));
        let output = run_target(
            &root,
            &cache,
            &root.join(format!("dot-segment-{index}.db")),
            target,
        );
        assert!(String::from_utf8_lossy(&output.stderr)
            .contains(&format!("alucard: target repo does not exist: {target}")));
        assert!(!cache.exists());
    }

    let local_shorthand = root.join("owner/repository");
    fs::create_dir_all(local_shorthand.parent().unwrap()).unwrap();
    git(&root, &["init", local_shorthand.to_str().unwrap()]);
    let local_precedence_cache = root.join("cache-local-precedence");
    assert_resolved(run_target(
        &root,
        &local_precedence_cache,
        &root.join("local-precedence.db"),
        "owner/repository",
    ));
    assert!(!local_precedence_cache.exists());

    fs::remove_dir_all(root).unwrap();
}
