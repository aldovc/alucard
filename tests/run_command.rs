use alucard::{
    interrupt_active_command, reset_command_interrupt, run_command, ActiveCommand,
    ACTIVE_COMMAND_PID,
};
use std::fs;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
#[test]
fn run_command_interrupts_the_child_while_waiting() {
    reset_command_interrupt();
    let directory = std::env::temp_dir().join(format!(
        "alucard-command-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&directory).unwrap();
    let working_directory = directory.clone();
    let command = thread::spawn(move || {
        run_command(
            "sleep",
            &["60"],
            &working_directory,
            Some(Duration::from_secs(60)),
        )
    });
    let deadline = Instant::now() + Duration::from_secs(1);
    while ACTIVE_COMMAND_PID.load(Ordering::SeqCst) == 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ACTIVE_COMMAND_PID.load(Ordering::SeqCst) > 0);

    interrupt_active_command();
    assert!(command.join().unwrap().is_ok());
    assert_eq!(ACTIVE_COMMAND_PID.load(Ordering::SeqCst), 0);

    reset_command_interrupt();
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn registration_terminates_a_child_when_already_interrupted() {
    reset_command_interrupt();
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();

    interrupt_active_command();
    let active_command = ActiveCommand::register(child.id());

    assert!(!child.wait().unwrap().success());
    drop(active_command);
    assert_eq!(ACTIVE_COMMAND_PID.load(Ordering::SeqCst), 0);
    reset_command_interrupt();
}
