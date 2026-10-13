use std::fs;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
    let rust = String::from_utf8(rust.stdout)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with("ok: ledger "))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(String::from_utf8(bash.stdout).unwrap().trim(), rust);
    fs::remove_dir_all(directory).unwrap();
}
