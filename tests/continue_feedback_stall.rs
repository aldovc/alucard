mod test_support;

use alucard::open;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TemporaryDirectory(PathBuf);

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn continue_feedback_stall_contract_matches_the_bash_workflow() {
    let directory = TemporaryDirectory(std::env::temp_dir().join(format!(
        "alucard-continue-feedback-stall-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )));
    fs::create_dir_all(&directory.0).unwrap();
    let ledger = directory.0.join("ledger.db");
    let output = Command::new("bash")
        .arg("test/test_continue_feedback_stall.sh")
        .env("ALUCARD", env!("CARGO_BIN_EXE_alucard"))
        .env("CONTINUE_TEST_LEDGER", &ledger)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        open(&ledger)
            .unwrap()
            .query_row(
                "SELECT kind FROM pull_requests WHERE number = 88",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "recovery"
    );
}
