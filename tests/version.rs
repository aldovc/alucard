use std::process::Command;

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
