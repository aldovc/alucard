use std::process::Command;

#[test]
fn merge_contract_matches_the_bash_workflow() {
    let output = Command::new("bash")
        .arg("test/test_merge.sh")
        .env("ALUCARD", env!("CARGO_BIN_EXE_alucard"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
